/**
 * What a Playback run offers now, and why a verb waits, from the run's state, this screen's role
 * and the setup. One pure function, so the run bar and the tests agree.
 *
 * The run bar's verbs: Apply Setup, Start (the file is checked again, the station opens at the
 * file's rate and the pre-roll starts, in one step, so the bar has no Arm of its own), New Run,
 * Stop and Abort. Stop and Abort need no control; everything else does. Every protection is the
 * control service's: a verb offered here can still be refused there, and says why.
 */

import type { PlaybackPhase, PlaybackRunState } from '$lib/protocol/playback';
import type { ActionId, ActionState, SetupStatus } from '../actions';

export interface PlaybackVerbContext {
	connected: boolean;
	controller: boolean;
	run: { state: PlaybackRunState; phase: PlaybackPhase } | null;
	setup: SetupStatus;
	/** Errors in the setup, the drive file's check and the channel table. */
	setupErrors: number;
	/** A request in flight (its verb), or null. */
	busy: string | null;
	/** Edits on this screen the service has not been sent, in words, or null. */
	pending: string | null;
}

/** A run is live while it starts, plays and stops. */
export const PLAYBACK_LIVE_STATES: PlaybackRunState[] = ['starting', 'running', 'stopping'];

export function playbackIsLive(run: { state: PlaybackRunState } | null | undefined): boolean {
	return !!run && PLAYBACK_LIVE_STATES.includes(run.state);
}

const hidden: ActionState = { visible: false, enabled: false, reason: null };

/** The run bar's verbs, by its names. The bar's Arm, Proceed, level step and ladder mode are not offered. */
export function playbackBarActions(ctx: PlaybackVerbContext): Record<ActionId, ActionState> {
	const gate = (visible: boolean, verb: string, own: string | null, needsControl = true): ActionState => {
		if (!visible) return hidden;
		let reason: string | null = null;
		if (!ctx.connected) reason = 'The control service is not connected.';
		else if (needsControl && !ctx.controller) reason = `Take control to ${verb}.`;
		else if (ctx.busy && needsControl) reason = `Waiting for ${ctx.busy}.`;
		else reason = own;
		return { visible, enabled: reason === null, reason };
	};
	const state = ctx.run?.state ?? 'unconfigured';
	const live = PLAYBACK_LIVE_STATES.includes(state);
	const setupBlocks = ctx.pending ?? (ctx.setup === 'changed' ? 'The setup changed since it was applied. Apply it first.' : null);
	const errors = ctx.setupErrors > 0 ? `Fix the ${ctx.setupErrors === 1 ? 'error' : `${ctx.setupErrors} errors`} in the setup first.` : null;
	const apply = gate(!live, 'apply the setup', ctx.pending ?? errors ?? (ctx.setup === 'applied' ? 'The service already holds this setup.' : null));
	const start = gate(state === 'configured', 'start', setupBlocks);
	const stop = gate(live, 'stop', state === 'stopping' ? 'The run is already stopping.' : null, false);
	const abort = gate(live, 'abort', null, false);
	const reset = gate(state === 'finished', 'start a new run', ctx.setup === 'changed' ? 'The setup changed; apply it to start a new run from it.' : null);
	return { apply, arm: hidden, override: hidden, start, proceed: hidden, level: hidden, mode: hidden, stop, abort, reset };
}

export function playbackPrimary(ctx: PlaybackVerbContext): ActionId | null {
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
