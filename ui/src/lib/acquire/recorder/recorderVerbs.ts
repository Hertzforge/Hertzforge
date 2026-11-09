/**
 * What a Recorder run offers now, and why a verb waits, from the run's state, phase and trigger
 * mode, this screen's role and the setup. One pure function, so the run bar, the Recorder's own
 * verbs and the tests agree.
 *
 * The run bar's verbs: Apply Setup, Start (the station opens and acquisition starts in one step,
 * so the bar has no Arm of its own), New Run, Stop and Abort. The Recorder's own verbs sit beside
 * them and follow the trigger mode:
 * - `start`: the run records from its first frame; Cut ends that recording.
 * - `manual`: Arm starts the pre-window filling, Disarm drops it, Record begins a recording (with
 *   the pre-window when armed), Cut ends it.
 * - `level`: Arm arms the level detector and the pre-window, Disarm returns to monitoring; each
 *   recording has its length, so there is no Record and no Cut.
 * Stop and Abort need no control; everything else does.
 */

import type { RecorderPhase, RecorderRunState, RecorderTriggerMode } from '$lib/protocol/recorder';
import type { ActionId, ActionState, SetupStatus } from '../actions';

export type RecorderVerbId = 'arm' | 'disarm' | 'record' | 'cut';

export const RECORDER_VERB_IDS: RecorderVerbId[] = ['arm', 'disarm', 'record', 'cut'];

/** What the verbs read of a run: the state, with the live frame laid over it. */
export interface RecorderVerbRun {
	state: RecorderRunState;
	phase: RecorderPhase;
	mode: RecorderTriggerMode | null;
	/** Recordings of the level trigger's count not yet begun; null when it has none. */
	left?: number | null;
}

export interface RecorderVerbContext {
	connected: boolean;
	controller: boolean;
	run: RecorderVerbRun | null;
	setup: SetupStatus;
	/** Errors in the setup and the channel table. */
	setupErrors: number;
	/** A request in flight (its verb), or null. */
	busy: string | null;
	/** Edits on this screen the service has not been sent, in words, or null. */
	pending: string | null;
}

/** A run is live while the station opens, while it runs and while its files are finished. */
export const RECORDER_LIVE_STATES: RecorderRunState[] = ['starting', 'running', 'stopping'];

export function recorderIsLive(run: { state: RecorderRunState } | null | undefined): boolean {
	return !!run && RECORDER_LIVE_STATES.includes(run.state);
}

const hidden: ActionState = { visible: false, enabled: false, reason: null };

function gateWith(ctx: RecorderVerbContext) {
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

/** The run bar's verbs, by its names. The bar's Arm, Proceed, level step and ladder mode are not offered. */
export function recorderBarActions(ctx: RecorderVerbContext): Record<ActionId, ActionState> {
	const gate = gateWith(ctx);
	const state = ctx.run?.state ?? 'unconfigured';
	const live = RECORDER_LIVE_STATES.includes(state);
	const setupBlocks = ctx.pending ?? (ctx.setup === 'changed' ? 'The setup changed since it was applied. Apply it first.' : null);
	const errors = ctx.setupErrors > 0 ? `Fix the ${ctx.setupErrors === 1 ? 'error' : `${ctx.setupErrors} errors`} in the setup first.` : null;
	const apply = gate(!live, 'apply the setup', ctx.pending ?? errors ?? (ctx.setup === 'applied' ? 'The service already holds this setup.' : null));
	const start = gate(state === 'configured', 'start', setupBlocks);
	const stop = gate(live, 'stop', state === 'stopping' ? 'The run is already finishing its recordings.' : null, false);
	const abort = gate(live, 'abort', null, false);
	const reset = gate(state === 'finished', 'start a new run', ctx.setup === 'changed' ? 'The setup changed; apply it to start a new run from it.' : null);
	return { apply, arm: hidden, override: hidden, start, proceed: hidden, level: hidden, mode: hidden, stop, abort, reset };
}

export function recorderPrimary(ctx: RecorderVerbContext): ActionId | null {
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

/**
 * The Recorder's own verbs, with why each waits. Shown while the run is starting, running or
 * stopping, as its trigger mode offers them: Arm and Disarm take each other's place.
 */
export function recorderVerbs(ctx: RecorderVerbContext): Record<RecorderVerbId, ActionState> {
	const gate = gateWith(ctx);
	const run = ctx.run;
	const state = run?.state ?? 'unconfigured';
	const mode = run?.mode ?? null;
	const phase = run?.phase ?? 'idle';
	const shown = RECORDER_LIVE_STATES.includes(state) && mode !== null;
	const notNow = state === 'starting' ? 'Once the station has opened.' : state === 'stopping' ? 'The run is stopping: its recordings are being finished.' : null;
	const recording = phase === 'recording';
	const armed = phase === 'armed';
	const triggered = mode === 'manual' || mode === 'level';
	// In level mode a recording with more of the count to come re-arms by itself: Disarm is the verb in its place.
	const rearms = mode === 'level' && recording && run?.left !== 0;
	const armShown = shown && triggered && !armed && !rearms;
	const disarmShown = shown && triggered && (armed || rearms);
	return {
		arm: gate(armShown, 'arm', notNow ?? (recording ? 'Once this recording has ended.' : null)),
		disarm: gate(disarmShown, 'disarm', notNow ?? (recording ? 'Once this recording has ended: the trigger then waits again.' : null)),
		record: gate(shown && mode === 'manual', 'record', notNow ?? (recording ? 'A recording is being saved: cut it first.' : null)),
		cut: gate(shown && mode !== 'level', 'cut the recording', notNow ?? (recording ? null : 'No recording is being saved.'))
	};
}
