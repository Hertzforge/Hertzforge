/**
 * What the run bar offers, and why an action is unavailable, from the run state, this screen's
 * role and the setup. One pure function, so the bar, the command palette and the keyboard all
 * agree, and the rules are testable on their own.
 *
 * Control: every run action needs control of the service, except Stop and Abort, which any
 * connected screen may send. The service and the amplifier's emergency stop remain the abort
 * authority; a socket Stop or Abort is a request.
 */

import type { RandomPhase, RandomRunState, RandomState, SessionRole } from '$lib/protocol/types';

export type ActionId = 'apply' | 'arm' | 'override' | 'start' | 'proceed' | 'level' | 'mode' | 'stop' | 'abort' | 'reset';

export interface ActionState {
	/** Offered in the present state. */
	visible: boolean;
	enabled: boolean;
	/** Why it cannot run now, in words; null when it can. */
	reason: string | null;
}

/**
 * - `none`: the service holds no configuration.
 * - `applied`: the service holds exactly this screen's setup.
 * - `changed`: this screen applied its setup and has changed it since.
 * - `foreign`: the service holds a configuration this screen did not send (another screen, or
 *   before a reload). Arm and Start run what the service holds.
 */
export type SetupStatus = 'none' | 'applied' | 'changed' | 'foreign';

export interface RunContext {
	connected: boolean;
	role: SessionRole;
	run: RandomState | null;
	setup: SetupStatus;
	/** Errors in the setup and the channel table, from the editor and the service's dry run. */
	setupErrors: number;
	/** A request in flight (its verb), or null. */
	busy: string | null;
	/** Edits on this screen the service has not been sent (a station choice, the channel table), in words, or null. */
	pending: string | null;
}

export const LIVE_STATES: RandomRunState[] = ['armed', 'running', 'review', 'stopping'];
export const DRIVING_PHASES: RandomPhase[] = ['engaged', 'full_level'];

export function isLive(run: RandomState | null): boolean {
	return !!run && LIVE_STATES.includes(run.state);
}

const hidden: ActionState = { visible: false, enabled: false, reason: null };

export function runActions(ctx: RunContext): Record<ActionId, ActionState> {
	const run = ctx.run;
	const state: RandomRunState = run?.state ?? 'unconfigured';
	const live = LIVE_STATES.includes(state);
	const controller = ctx.role === 'controller';

	const gate = (visible: boolean, verb: string, own: string | null, needsControl = true): ActionState => {
		if (!visible) return hidden;
		let reason: string | null = null;
		if (!ctx.connected) reason = 'The control service is not connected.';
		else if (needsControl && !controller) reason = `Take control to ${verb}.`;
		else if (ctx.busy && needsControl) reason = `Waiting for ${ctx.busy}.`;
		else reason = own;
		return { visible, enabled: reason === null, reason };
	};

	const setupBlocks = ctx.pending ?? (ctx.setup === 'changed' ? 'The setup changed since it was applied. Apply it first.' : null);
	const errors = ctx.setupErrors > 0 ? `Fix the ${ctx.setupErrors === 1 ? 'error' : `${ctx.setupErrors} errors`} in the setup first.` : null;

	const apply = gate(!live, 'apply the setup', ctx.pending ?? errors ?? (ctx.setup === 'applied' ? 'The service already holds this setup.' : null));

	const arm = gate(state === 'configured', 'arm', setupBlocks);
	const override = gate(state === 'blocked', 'arm', setupBlocks);
	const start = gate(state === 'configured' || state === 'armed', 'start', setupBlocks);
	const proceed = gate(state === 'review', 'proceed', null);

	const driving = state === 'running' && !!run && DRIVING_PHASES.includes(run.phase);
	const level = gate(
		state === 'running',
		'change the level',
		!driving ? 'The level moves once the run is driving at the schedule.' : run?.mode !== 'manual' ? 'Switch the ladder to manual to step the level.' : null
	);
	const mode = gate(!!run && state !== 'unconfigured' && state !== 'finished', 'change the ladder mode', null);

	const stop = gate(live, 'stop', state === 'stopping' ? 'The run is already ramping down.' : null, false);
	const abort = gate(live, 'abort', null, false);

	const reset = gate(state === 'finished', 'start a new run', ctx.setup === 'changed' ? 'The setup changed; apply it to start a new run from it.' : null);

	return { apply, arm, override, start, proceed, level, mode, stop, abort, reset };
}

/** The one next step the bar emphasises, or null while the run drives. */
export function primaryAction(ctx: RunContext): ActionId | null {
	const state = ctx.run?.state ?? 'unconfigured';
	switch (state) {
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
