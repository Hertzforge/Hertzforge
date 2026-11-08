/**
 * What a classical shock run offers now, and why a verb waits, from the run's state, this
 * screen's role and the setup. One pure function, so the run bar, the module's own verbs and the
 * tests agree.
 *
 * The run bar's verbs (Apply Setup, Arm, Arm Past Feasibility, Start, Proceed, New Run, Stop,
 * Abort) follow single-output Random's rules. The module's verbs (Fire, a level step, Go to a
 * level, Train, Burst) are offered in the manual ladder only: an automatic schedule fires itself.
 * Stop and Abort need no control; everything else does.
 */

import type { ShockState } from '$lib/protocol/shock';
import type { ActionId, ActionState, SetupStatus } from '../actions';

export type ShockVerbId = 'fire' | 'levelDown' | 'levelUp' | 'rung' | 'train' | 'burst';

export interface ShockVerbContext {
	connected: boolean;
	controller: boolean;
	run: ShockState | null;
	setup: SetupStatus;
	/** Errors in the setup and the channel table. */
	setupErrors: number;
	/** A request in flight (its verb), or null. */
	busy: string | null;
	/** Edits on this screen the service has not been sent, in words, or null. */
	pending: string | null;
	/** The configuration the run holds has a gunfire burst. */
	gunfire: boolean;
}

export const SHOCK_LIVE_STATES: ShockState['state'][] = ['armed', 'running', 'review', 'stopping'];

export function shockIsLive(run: { state: ShockState['state'] } | null | undefined): boolean {
	return !!run && SHOCK_LIVE_STATES.includes(run.state);
}

const hidden: ActionState = { visible: false, enabled: false, reason: null };

function gateWith(ctx: ShockVerbContext) {
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
export function shockBarActions(ctx: ShockVerbContext): Record<ActionId, ActionState> {
	const gate = gateWith(ctx);
	const run = ctx.run;
	const state = run?.state ?? 'unconfigured';
	const live = SHOCK_LIVE_STATES.includes(state);
	const setupBlocks = ctx.pending ?? (ctx.setup === 'changed' ? 'The setup changed since it was applied. Apply it first.' : null);
	const errors = ctx.setupErrors > 0 ? `Fix the ${ctx.setupErrors === 1 ? 'error' : `${ctx.setupErrors} errors`} in the setup first.` : null;
	const apply = gate(!live, 'apply the setup', ctx.pending ?? errors ?? (ctx.setup === 'applied' ? 'The service already holds this setup.' : null));
	const arm = gate(state === 'configured', 'arm', setupBlocks);
	const override = gate(state === 'blocked', 'arm', setupBlocks);
	const start = gate(state === 'configured' || state === 'armed', 'start', setupBlocks);
	const proceed = gate(state === 'review', 'proceed', null);
	const stop = gate(live, 'stop', state === 'stopping' ? 'The run is already stopping after the pulse in flight.' : null, false);
	const abort = gate(live, 'abort', null, false);
	const reset = gate(state === 'finished', 'start a new run', ctx.setup === 'changed' ? 'The setup changed; apply it to start a new run from it.' : null);
	return { apply, arm, override, start, proceed, level: hidden, mode: hidden, stop, abort, reset };
}

export function shockPrimary(ctx: ShockVerbContext): ActionId | null {
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
		case 'finished':
			return ctx.setup === 'changed' ? 'apply' : 'reset';
		default:
			return null;
	}
}

/** The manual ladder's verbs, with why each waits. */
export function shockModuleVerbs(ctx: ShockVerbContext): Record<ShockVerbId, ActionState> {
	const gate = gateWith(ctx);
	const run = ctx.run;
	const manual = run?.mode === 'manual';
	const running = run?.state === 'running';
	const steer = manual && (running || run?.state === 'review');
	const levels = run?.levels_db ?? [];
	const level = run?.level_db ?? null;

	const fireWhy = (): string | null => {
		if (!run) return null;
		if (run.phase === 'sysid') return 'Identification is running.';
		if (run.phase === 'firing') return 'A pulse is playing.';
		if (run.next_pulse_s !== null && run.next_pulse_s > 0.05) return `The next pulse may fire in ${run.next_pulse_s.toFixed(1)} s.`;
		return null;
	};
	const levelWhy = (dir: -1 | 1): string | null => {
		if (run?.phase === 'sysid') return 'The level moves once identification is done.';
		if (level === null || !levels.length) return null;
		if (dir > 0 && level >= levels[levels.length - 1] - 1e-9) return 'At the last level of the table.';
		if (dir < 0 && level <= levels[0] + 1e-9) return 'At the first level of the table.';
		return null;
	};

	return {
		fire: gate(manual && running, 'fire', fireWhy()),
		levelDown: gate(steer, 'change the level', levelWhy(-1)),
		levelUp: gate(steer, 'change the level', levelWhy(1)),
		rung: gate(steer, 'change the level', run?.phase === 'sysid' ? 'The level moves once identification is done.' : null),
		train: gate(manual && running, 'fire a train', fireWhy()),
		burst: gate(manual && running && ctx.gunfire, 'fire the burst', fireWhy())
	};
}
