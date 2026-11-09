/**
 * What an Explorer run offers now, and why a verb waits, from the run's state and phase, this
 * screen's role and the setup. One pure function, so the run bar, the steering panel, the keys and
 * the tests agree.
 *
 * The run bar's verbs: Apply Setup, Start (the station opens and arms in the same step, so there
 * is no Arm), New Run, Stop and Abort. There is no review hold, no level ladder and no ladder mode.
 * The steering verbs (the frequency, the level, a sweep, Hold, clearing the survey) apply while the
 * run is running; once a stop has begun nothing may raise or move the drive, so they wait with the
 * reason said. A dB step needs a level to step from. Stop and Abort need no control; everything
 * else does.
 */

import type { ExplorerState } from '$lib/protocol/explorer';
import type { ActionId, ActionState, SetupStatus } from '../actions';

export type ExplorerVerbId = 'frequency' | 'levelVolts' | 'levelStep' | 'sweep' | 'hold' | 'clearSurvey';

/** What the verbs read of a run: the state, with the live frame laid over it. */
export type ExplorerVerbRun = Pick<ExplorerState, 'state' | 'phase' | 'target_vpk' | 'sweep'>;

export interface ExplorerVerbContext {
	connected: boolean;
	controller: boolean;
	run: ExplorerVerbRun | null;
	setup: SetupStatus;
	/** Errors in the setup and the channel table. */
	setupErrors: number;
	/** A request in flight (its verb), or null. */
	busy: string | null;
	/** Edits on this screen the service has not been sent, in words, or null. */
	pending: string | null;
}

/** A run is live while the station opens, while it runs and while it stops. */
export const EXPLORER_LIVE_STATES: ExplorerState['state'][] = ['starting', 'running', 'stopping'];

export function explorerIsLive(run: { state: ExplorerState['state'] } | null | undefined): boolean {
	return !!run && EXPLORER_LIVE_STATES.includes(run.state);
}

const hidden: ActionState = { visible: false, enabled: false, reason: null };

function gateWith(ctx: ExplorerVerbContext) {
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

/** The run bar's verbs, by its names. Arm, Proceed, the level step and the ladder mode are not offered. */
export function explorerBarActions(ctx: ExplorerVerbContext): Record<ActionId, ActionState> {
	const gate = gateWith(ctx);
	const state = ctx.run?.state ?? 'unconfigured';
	const live = EXPLORER_LIVE_STATES.includes(state);
	const setupBlocks = ctx.pending ?? (ctx.setup === 'changed' ? 'The setup changed since it was applied. Apply it first.' : null);
	const errors = ctx.setupErrors > 0 ? `Fix the ${ctx.setupErrors === 1 ? 'error' : `${ctx.setupErrors} errors`} in the setup first.` : null;
	const apply = gate(!live, 'apply the setup', ctx.pending ?? errors ?? (ctx.setup === 'applied' ? 'The service already holds this setup.' : null));
	const start = gate(state === 'configured', 'start', setupBlocks);
	const stop = gate(live, 'stop', state === 'stopping' ? 'The drive is already ramping down.' : null, false);
	const abort = gate(live, 'abort', null, false);
	const reset = gate(state === 'finished', 'start a new run', ctx.setup === 'changed' ? 'The setup changed; apply it to start a new run from it.' : null);
	return { apply, arm: hidden, override: hidden, start, proceed: hidden, level: hidden, mode: hidden, stop, abort, reset };
}

export function explorerPrimary(ctx: ExplorerVerbContext): ActionId | null {
	switch (ctx.run?.state ?? 'unconfigured') {
		case 'unconfigured':
			return 'apply';
		case 'configured':
			return ctx.setup === 'changed' ? 'apply' : 'start';
		case 'finished':
			return ctx.setup === 'changed' ? 'apply' : 'reset';
		default:
			return null;
	}
}

/** The steering verbs, with why each waits. Shown while the run is starting, running or stopping. */
export function explorerSteerVerbs(ctx: ExplorerVerbContext): Record<ExplorerVerbId, ActionState> {
	const gate = gateWith(ctx);
	const run = ctx.run;
	const state = run?.state ?? 'unconfigured';
	const shown = EXPLORER_LIVE_STATES.includes(state);
	const notNow = state === 'starting' ? 'Once the station has opened and the tone runs at 0 V.' : state === 'stopping' ? 'The run is stopping: nothing raises or moves the drive now.' : null;
	const fromZero = run && run.target_vpk === 0 ? 'The level is 0 V: set a level in volts first, then step it in dB.' : null;
	const moving = !!run?.sweep && !run.sweep.halted;
	return {
		frequency: gate(shown, 'steer the tone', notNow),
		levelVolts: gate(shown, 'set the level', notNow),
		levelStep: gate(shown, 'step the level', notNow ?? fromZero),
		sweep: gate(shown, 'sweep the tone', notNow),
		hold: gate(shown, 'hold the tone', notNow ?? (moving ? null : 'No sweep is moving the tone.')),
		clearSurvey: gate(shown, 'clear the survey', state === 'starting' ? 'Once the run is running.' : null)
	};
}

/** A key press as the steering panel reads it. */
export interface SteerKey {
	key: string;
	shiftKey?: boolean;
	altKey?: boolean;
	ctrlKey?: boolean;
	metaKey?: boolean;
	/** Held down: the browser's auto-repeat. */
	repeat?: boolean;
}

/** What one press asks for: a frequency step in octaves or a level step in dB, or a hold. */
export type SteerKeyAction = { step_oct: number } | { step_db: number } | { hold: true };

/**
 * The steering keys, read only while the steering panel holds focus: Left and Right step the
 * frequency, Up and Down the level, by the fine step, or the coarse one with Shift; H holds the
 * sweep. One press is one step: a key held down does not repeat, and no key moves the tone further
 * than one coarse step. A key with Ctrl, Alt or Cmd is the shell's, never the panel's.
 */
export function steerForKey(e: SteerKey, steps: { fine_per_oct: number; coarse_per_oct: number; fine_db: number; coarse_db: number }): SteerKeyAction | null {
	if (e.repeat || e.ctrlKey || e.altKey || e.metaKey) return null;
	const oct = 1 / (e.shiftKey ? steps.coarse_per_oct : steps.fine_per_oct);
	const dB = e.shiftKey ? steps.coarse_db : steps.fine_db;
	switch (e.key) {
		case 'ArrowRight':
			return { step_oct: oct };
		case 'ArrowLeft':
			return { step_oct: -oct };
		case 'ArrowUp':
			return { step_db: dB };
		case 'ArrowDown':
			return { step_db: -dB };
		case 'h':
		case 'H':
			return { hold: true };
		default:
			return null;
	}
}
