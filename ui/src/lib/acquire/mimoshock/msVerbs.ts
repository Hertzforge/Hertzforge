/**
 * What a MIMO Shock run offers now, and why a verb waits, from the run's state, this screen's
 * role and the setup. One pure function, so the run bar, the Fire verb and the tests agree.
 *
 * The run bar's verbs (Apply Setup, Arm, Start, Proceed, New Run, Stop, Abort) follow the other
 * pulse tests' rules. Proceed past the unreachable hold waits for the operator's own words. Fire is
 * offered in the manual mode only; the ladder alone sets the level, so there are no level verbs.
 * Stop and Abort need no control; everything else does.
 */

import type { MimoShockState } from '$lib/protocol/mimoShock';
import type { ActionId, ActionState, SetupStatus } from '../actions';
import { needsAcknowledgement } from './msWords';

export interface MsVerbContext {
	connected: boolean;
	controller: boolean;
	run: Pick<MimoShockState, 'state' | 'phase' | 'mode' | 'next_pulse_s' | 'review'> | null;
	setup: SetupStatus;
	setupErrors: number;
	busy: string | null;
	pending: string | null;
	/** The operator's acknowledgement as typed, for the unreachable hold. */
	acknowledgement: string;
}

export const MS_LIVE_STATES: MimoShockState['state'][] = ['armed', 'running', 'review', 'stopping'];

export function msIsLive(run: { state: MimoShockState['state'] } | null | undefined): boolean {
	return !!run && MS_LIVE_STATES.includes(run.state);
}

const hidden: ActionState = { visible: false, enabled: false, reason: null };

function gateWith(ctx: MsVerbContext) {
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

/** The run bar's verbs, by its names. The bar's level and mode verbs are not offered. */
export function msBarActions(ctx: MsVerbContext): Record<ActionId, ActionState> {
	const gate = gateWith(ctx);
	const state = ctx.run?.state ?? 'unconfigured';
	const live = MS_LIVE_STATES.includes(state);
	const setupBlocks = ctx.pending ?? (ctx.setup === 'changed' ? 'The setup changed since it was applied. Apply it first.' : null);
	const errors = ctx.setupErrors > 0 ? `Fix the ${ctx.setupErrors === 1 ? 'error' : `${ctx.setupErrors} errors`} in the setup first.` : null;
	const ack = needsAcknowledgement(ctx.run) && !ctx.acknowledgement.trim() ? 'A control cannot reach its target: say in your own words why the run should go on, then proceed.' : null;
	return {
		apply: gate(!live, 'apply the setup', ctx.pending ?? errors ?? (ctx.setup === 'applied' ? 'The service already holds this setup.' : null)),
		arm: gate(state === 'configured', 'arm', setupBlocks),
		override: hidden,
		start: gate(state === 'configured' || state === 'armed', 'start', setupBlocks),
		proceed: gate(state === 'review', 'proceed', ack),
		level: hidden,
		mode: hidden,
		stop: gate(live, 'stop', state === 'stopping' ? 'The run is already stopping after the pulse in flight.' : null, false),
		abort: gate(live, 'abort', null, false),
		reset: gate(state === 'finished', 'start a new run', ctx.setup === 'changed' ? 'The setup changed; apply it to start a new run from it.' : null)
	};
}

export function msPrimary(ctx: MsVerbContext): ActionId | null {
	switch (ctx.run?.state ?? 'unconfigured') {
		case 'unconfigured':
			return 'apply';
		case 'configured':
			return ctx.setup === 'changed' ? 'apply' : 'start';
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

/** Fire, in the manual mode only, with why it waits. */
export function msFire(ctx: MsVerbContext): ActionState {
	const run = ctx.run;
	const visible = run?.mode === 'manual' && run.state === 'running';
	let own: string | null = null;
	if (run?.phase === 'sysid') own = 'Identification is running.';
	else if (run?.phase === 'firing') own = 'A pulse is playing.';
	else if (run && run.next_pulse_s !== null && run.next_pulse_s > 0.05) own = `The next pulse may fire in ${run.next_pulse_s.toFixed(1)} s.`;
	return gateWith(ctx)(visible, 'fire', own);
}
