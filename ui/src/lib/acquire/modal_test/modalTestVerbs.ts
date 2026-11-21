/**
 * What a Modal Test offers now, and why a verb waits, from the test's state, this screen's role,
 * the setup and the service's counts. Pure functions, so the screen, the keys and the tests agree.
 *
 * The run bar's verbs: Apply Setup, Start (the station opens and the test is ready at its first
 * point), New Run, Stop (the controlled end: in shaker mode the shared stop ramp) and Abort. Stop
 * and Abort need no control; everything else does. The Measure screen's own verbs (Arm, Accept,
 * Reject, Undo, Clear Point, Previous, Next, Start and Stop Driving, Use This Hit as the Target,
 * End Test) are
 * gated here too. Every rule is the control service's: a verb offered here can still be refused
 * there, and the screen shows the refusal in the service's words.
 */

import type { ModalMode, ModalRunState, ModalTestRun } from '$lib/protocol/modal_test';
import type { ActionId, ActionState, SetupStatus } from '../actions';

export interface ModalVerbContext {
	connected: boolean;
	controller: boolean;
	run: Pick<ModalTestRun, 'state' | 'mode' | 'counts' | 'latest' | 'point'> | null;
	setup: SetupStatus;
	setupErrors: number;
	busy: string | null;
	pending: string | null;
}

/** A test is live from its start until it has finished. */
export const MODAL_LIVE_STATES: ModalRunState[] = ['starting', 'idle', 'arming', 'armed', 'captured', 'measuring', 'stopping', 'point_done'];

export function modalIsLive(run: { state: ModalRunState } | null | undefined): boolean {
	return !!run && MODAL_LIVE_STATES.includes(run.state);
}

const hidden: ActionState = { visible: false, enabled: false, reason: null };

function gateOf(ctx: ModalVerbContext) {
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
export function modalBarActions(ctx: ModalVerbContext): Record<ActionId, ActionState> {
	const gate = gateOf(ctx);
	const state = ctx.run?.state ?? 'none';
	const live = MODAL_LIVE_STATES.includes(state);
	const setupBlocks = ctx.pending ?? (ctx.setup === 'changed' ? 'The setup changed since it was applied. Apply it first.' : null);
	const errors = ctx.setupErrors > 0 ? `Fix the ${ctx.setupErrors === 1 ? 'error' : `${ctx.setupErrors} errors`} in the setup first.` : null;
	const apply = gate(!live, 'apply the setup', ctx.pending ?? errors ?? (ctx.setup === 'applied' ? 'The service already holds this setup.' : null));
	const start = gate(state === 'configured', 'start', setupBlocks);
	const stop = gate(live, 'end the test', state === 'stopping' ? 'The test is already ending.' : null, false);
	const abort = gate(live, 'abort', null, false);
	const reset = gate(state === 'finished', 'start a new test', ctx.setup === 'changed' ? 'The setup changed; apply it to start a new test from it.' : null);
	return { apply, arm: hidden, override: hidden, start, proceed: hidden, level: hidden, mode: hidden, stop, abort, reset };
}

export function modalPrimary(ctx: ModalVerbContext): ActionId | null {
	switch (ctx.run?.state ?? 'none') {
		case 'none':
			return 'apply';
		case 'configured':
			return ctx.setup === 'changed' ? 'apply' : 'start';
		case 'finished':
			return ctx.setup === 'changed' ? 'apply' : 'reset';
		default:
			return null;
	}
}

export type ModalVerb = 'arm' | 'disarm' | 'accept' | 'reject' | 'undo' | 'clear' | 'next' | 'previous' | 'target' | 'drive_start' | 'drive_stop' | 'end';

/** The Measure screen's verbs. */
export function modalMeasureVerbs(ctx: ModalVerbContext): Record<ModalVerb, ActionState> {
	const gate = gateOf(ctx);
	const r = ctx.run;
	const state = r?.state ?? 'none';
	const live = MODAL_LIVE_STATES.includes(state);
	const mode: ModalMode | null = r?.mode ?? null;
	const impact = live && mode === 'impact';
	const shaker = live && mode === 'shaker';
	const settled = state === 'starting' ? 'The station is opening.' : state === 'stopping' ? 'The test is ending.' : null;
	// Accept and reject act on the latest hit of this point until the next hit lands, in either
	// accept mode: the service holds which hit that is; the screen offers what it shows.
	const latest = r?.latest && r.latest.point === r.point ? r.latest : null;
	const noHit = latest ? null : 'No hit at this point yet.';
	const accept = gate(impact, 'accept the hit', settled ?? noHit ?? (latest?.status === 'accepted' ? 'This hit is already accepted.' : latest?.status === 'undone' ? 'This hit was undone.' : null));
	const reject = gate(impact, 'reject the hit', settled ?? noHit ?? (latest?.status === 'rejected' ? 'This hit is already rejected.' : latest?.status === 'undone' ? 'This hit was undone.' : null));
	const undo = gate(live, 'undo', settled ?? ((r?.counts.point.accepted ?? 0) > 0 ? null : `Nothing accepted at this point to undo.`));
	// Clear Point takes every accepted average of this point out; not while a hit waits or the shakers average
	const clear = gate(live, 'clear the point', settled ?? (state === 'measuring' ? 'Stop driving first.' : state === 'captured' ? 'Accept or reject the hit first.' : (r?.counts.point.accepted ?? 0) > 0 ? null : 'Nothing accepted at this point to clear.'));
	const moving = settled ?? (state === 'measuring' ? 'Stop driving first.' : null);
	const next = gate(live, 'move to the next point', moving);
	const previous = gate(live, 'move to the previous point', moving);
	const arm = gate(impact && state !== 'armed' && state !== 'arming', 'arm', settled ?? (state === 'point_done' ? 'This point has its hits: move to the next point, or undo one.' : state === 'captured' ? 'Accept or reject the hit first.' : null));
	const disarm = gate(impact && (state === 'armed' || state === 'arming'), 'disarm', null);
	const target = gate(impact, 'set the target', settled ?? (latest?.report || latest?.peak !== undefined ? null : 'Take a hit first.'));
	const drive_start = gate(shaker && state !== 'measuring', 'start driving', settled ?? (state === 'point_done' ? 'This point has its frames: move to the next point, or undo.' : null));
	const drive_stop = gate(shaker && state === 'measuring', 'stop driving', null);
	const end = gate(live, 'end the test', state === 'stopping' ? 'The test is already ending.' : null, false);
	return { arm, disarm, accept, reject, undo, clear, next, previous, target, drive_start, drive_stop, end };
}
