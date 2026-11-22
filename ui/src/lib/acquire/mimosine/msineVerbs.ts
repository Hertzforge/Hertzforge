/**
 * What a MIMO Sine run offers now, and why a verb waits. The run bar's verbs, the level step and
 * the ladder mode are Sine's, as are the module's own: Hold and Resume, and on a sweep Jog and Go
 * to. `mimo_sine.*` takes them with Sine's rules (PROTOCOL.md, "Running a MIMO Sine Test"), so the
 * rules are Sine's functions, read with the MIMO Sine state. Sine's Reverse is not offered: a MIMO
 * Sine sweep turns only at the band's edges, and the service refuses `mimo_sine.reverse`.
 */

import type { MimoSineState } from '$lib/protocol/mimoSine';
import type { SineState } from '$lib/protocol/sine';
import type { ActionId, ActionState, SetupStatus } from '../actions';
import { LIVE_STATES } from '../actions';
import { sineBarActions, sineModuleVerbs, sinePrimary, type SineVerbContext, type SineVerbId } from '../sine/sineVerbs';

export type MsineVerbId = Exclude<SineVerbId, 'reverse'>;

export interface MsineVerbContext {
	connected: boolean;
	controller: boolean;
	run: Pick<MimoSineState, 'state' | 'phase' | 'mode' | 'test' | 'held'> | null;
	setup: SetupStatus;
	setupErrors: number;
	busy: string | null;
	pending: string | null;
}

export const MSINE_LIVE_STATES: MimoSineState['state'][] = LIVE_STATES as MimoSineState['state'][];

export function msineIsLive(s: { state: MimoSineState['state'] } | null | undefined): boolean {
	return !!s && MSINE_LIVE_STATES.includes(s.state);
}

function sineContext(ctx: MsineVerbContext): SineVerbContext {
	return { ...ctx, run: ctx.run as unknown as Pick<SineState, 'state' | 'phase' | 'mode' | 'test' | 'held'> | null };
}

export function msineBarActions(ctx: MsineVerbContext): Record<ActionId, ActionState> {
	return sineBarActions(sineContext(ctx));
}

export function msinePrimary(ctx: MsineVerbContext): ActionId | null {
	return sinePrimary(sineContext(ctx));
}

/** Hold or Resume, and on a sweep Jog and Go to, with why each waits. */
export function msineModuleVerbs(ctx: MsineVerbContext): Record<MsineVerbId, ActionState> {
	const { hold, resume, jog, goto } = sineModuleVerbs(sineContext(ctx));
	return { hold, resume, jog, goto };
}
