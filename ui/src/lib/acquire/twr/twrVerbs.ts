/**
 * What a time waveform replication (TWR) run offers now, and why a verb waits, from the run's
 * state, this screen's role and the setup. One pure function, so the run bar, the module's own
 * verbs and the tests agree.
 *
 * The run bar's verbs (Apply Setup, Arm, Arm Past Feasibility, Start, Proceed, New Run, Stop,
 * Abort) follow single-output Random's rules, with two differences: the coherence hold is a live
 * state that only New Run (`twr.reset`) releases, and the bar's own level and mode verbs are not
 * offered (the ladder mode is fixed for the run). The module's verbs: Iterate, Play, a level step
 * down and up and Go to a level, between plays on the manual ladder (and at a level that failed its
 * gate under the hold policy); Freeze and Resume the law's corrections while the run is running or
 * holding for review. Stop and Abort need no control; everything else does.
 */

import type { TwrPlayRow, TwrState } from '$lib/protocol/twr';
import type { ActionId, ActionState, SetupStatus } from '../actions';

export type TwrVerbId = 'iterate' | 'play' | 'levelDown' | 'levelUp' | 'rung' | 'freeze' | 'unfreeze';

/** What the verbs read of a run: the state, with the live frame laid over it. */
export type TwrVerbRun = Pick<TwrState, 'state' | 'phase' | 'mode' | 'levels_db' | 'level_db' | 'frozen' | 'last_play'>;

export interface TwrVerbContext {
	connected: boolean;
	controller: boolean;
	run: TwrVerbRun | null;
	setup: SetupStatus;
	/** Errors in the setup and the channel table. */
	setupErrors: number;
	/** A request in flight (its verb), or null. */
	busy: string | null;
	/** Edits on this screen the service has not been sent, in words, or null. */
	pending: string | null;
}

/** A run is live while armed, running, holding for review, held by the coherence hold or stopping. */
export const TWR_LIVE_STATES: TwrState['state'][] = ['armed', 'running', 'review', 'held', 'stopping'];

export function twrIsLive(run: { state: TwrState['state'] } | null | undefined): boolean {
	return !!run && TWR_LIVE_STATES.includes(run.state);
}

/** The phases between plays, where a verb may start the next one or move the level. */
const BETWEEN: TwrState['phase'][] = ['waiting', 'holding', 'awaiting_review'];

const hidden: ActionState = { visible: false, enabled: false, reason: null };

function gateWith(ctx: TwrVerbContext) {
	return (visible: boolean, verb: string, own: string | null, needsControl = true): ActionState => {
		if (!visible) return hidden;
		let reason: string | null = null;
		if (!ctx.connected) reason = 'The control service is not connected.';
		else if (needsControl && !ctx.controller) reason = `Take control to ${verb}.`;
		else if (ctx.busy && needsControl) reason = `Waiting for ${ctx.busy}.`;
		else reason = own;
		return { visible, enabled: reason === null, reason };
	};
}

/** The run bar's verbs, by its names. The bar's own level and mode verbs are not offered. */
export function twrBarActions(ctx: TwrVerbContext): Record<ActionId, ActionState> {
	const gate = gateWith(ctx);
	const state = ctx.run?.state ?? 'unconfigured';
	const live = TWR_LIVE_STATES.includes(state);
	const setupBlocks = ctx.pending ?? (ctx.setup === 'changed' ? 'The setup changed since it was applied. Apply it first.' : null);
	const errors = ctx.setupErrors > 0 ? `Fix the ${ctx.setupErrors === 1 ? 'error' : `${ctx.setupErrors} errors`} in the setup first.` : null;
	const apply = gate(!live, 'apply the setup', ctx.pending ?? errors ?? (ctx.setup === 'applied' ? 'The service already holds this setup.' : null));
	const arm = gate(state === 'configured', 'arm', setupBlocks);
	const override = gate(state === 'blocked', 'arm', setupBlocks);
	const start = gate(state === 'configured' || state === 'armed', 'start', setupBlocks);
	const proceed = gate(state === 'review', 'proceed', null);
	const stop = gate(live, 'stop', state === 'stopping' ? 'The run is already stopping.' : null, false);
	const abort = gate(live, 'abort', null, false);
	const reset = gate(state === 'finished' || state === 'held', 'start a new run', ctx.setup === 'changed' && state === 'finished' ? 'The setup changed; apply it to start a new run from it.' : null);
	return { apply, arm, override, start, proceed, level: hidden, mode: hidden, stop, abort, reset };
}

export function twrPrimary(ctx: TwrVerbContext): ActionId | null {
	switch (ctx.run?.state ?? 'unconfigured') {
		case 'unconfigured':
			return 'apply';
		case 'configured':
			return ctx.setup === 'changed' ? 'apply' : 'start';
		case 'blocked':
			return ctx.setup === 'changed' ? 'apply' : 'override';
		case 'armed':
			return 'start';
		case 'review':
			return 'proceed';
		case 'held':
			return 'reset';
		case 'finished':
			return ctx.setup === 'changed' ? 'apply' : 'reset';
		default:
			return null;
	}
}

/** The last play gated the level the run is at now: playback may begin from it. */
export function levelGated(run: Pick<TwrState, 'level_db' | 'last_play'> | null): boolean {
	const last: TwrPlayRow | null = run?.last_play ?? null;
	if (!last || run?.level_db === null || run?.level_db === undefined) return false;
	return last.kind === 'iteration' && last.gated && Math.abs(last.level_db - run.level_db) < 1e-9;
}

/** The module's verbs, with why each waits. */
export function twrModuleVerbs(ctx: TwrVerbContext): Record<TwrVerbId, ActionState> {
	const gate = gateWith(ctx);
	const run = ctx.run;
	const manual = run?.mode === 'manual';
	const running = run?.state === 'running';
	const phase = run?.phase ?? 'idle';
	const levels = run?.levels_db ?? [];
	const level = run?.level_db ?? null;
	// The level moves on the manual ladder, or at a level that failed its gate under the hold policy.
	const steer = (manual && (running || run?.state === 'review')) || (running && phase === 'holding');

	const playWhy = (): string | null => {
		if (phase === 'sysid') return 'Identification is running.';
		if (phase === 'iterating' || phase === 'playing') return 'A play is in flight.';
		if (phase === 'holding') return 'The level did not meet its gate; change the level or abort.';
		return null;
	};
	const levelWhy = (dir: -1 | 1): string | null => {
		if (phase === 'sysid') return 'The level moves once identification is done.';
		if (!BETWEEN.includes(phase)) return 'The level moves between plays.';
		if (level === null || !levels.length) return null;
		if (dir > 0 && level >= levels[levels.length - 1] - 1e-9) return 'At the last level of the table.';
		if (dir < 0 && level <= levels[0] + 1e-9) return 'At the first level of the table.';
		return null;
	};
	const iterateWhy = playWhy();
	const play = playWhy() ?? (levelGated(run) ? null : 'Once a play at this level meets the gate.');
	const frozen = !!run?.frozen;
	const adapting = running || run?.state === 'review';

	return {
		iterate: gate(manual && running, 'iterate', iterateWhy),
		play: gate(manual && running, 'begin playback', play),
		levelDown: gate(steer, 'change the level', levelWhy(-1)),
		levelUp: gate(steer, 'change the level', levelWhy(1)),
		rung: gate(steer, 'change the level', phase === 'sysid' ? 'The level moves once identification is done.' : BETWEEN.includes(phase) ? null : 'The level moves between plays.'),
		freeze: gate(adapting && !frozen, 'freeze the corrections', null),
		unfreeze: gate(adapting && frozen, 'resume the corrections', null)
	};
}
