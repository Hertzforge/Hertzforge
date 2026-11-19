/**
 * What an SRS shock run offers now, and why a verb waits. The run bar's verbs and the manual
 * ladder's (Fire, a level step, Go to a level) follow classical shock's rules exactly: the same
 * states, the same reasons. SRS shock has no trains and no gunfire bursts, so those two are never
 * offered.
 */

import type { ActionId, ActionState } from '../actions';
import { shockBarActions, shockIsLive, shockModuleVerbs, shockPrimary, SHOCK_LIVE_STATES, type ShockVerbContext, type ShockVerbId } from '../shock/shockVerbs';

export type SrsVerbContext = Omit<ShockVerbContext, 'gunfire'>;

export const SRS_LIVE_STATES = SHOCK_LIVE_STATES;
export const srsIsLive = shockIsLive;

const hidden: ActionState = { visible: false, enabled: false, reason: null };

/** The run bar's verbs, by its names. The bar's own level and mode verbs are not offered. */
export function srsBarActions(ctx: SrsVerbContext): Record<ActionId, ActionState> {
	return shockBarActions({ ...ctx, gunfire: false });
}

export function srsPrimary(ctx: SrsVerbContext): ActionId | null {
	return shockPrimary({ ...ctx, gunfire: false });
}

/** The manual ladder's verbs, with why each waits; a train and a burst are never offered. */
export function srsModuleVerbs(ctx: SrsVerbContext): Record<ShockVerbId, ActionState> {
	return { ...shockModuleVerbs({ ...ctx, gunfire: false }), train: hidden, burst: hidden };
}
