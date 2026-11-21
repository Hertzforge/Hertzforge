/**
 * What a MIMO Mixed Mode run offers now, and why a verb waits. The run's states and phases are
 * Mixed Mode's, so its rules are: the run bar's verbs (Random's), Hold and Resume, and a part
 * switched on or off once the parts have come in. The review adds two holds, a failed coherence
 * gate and a prediction over a drive ceiling: past either only by an override with the operator's
 * words, which names the hold. A forecast that refuses (no drive budget fits) holds the Proceed
 * with no override.
 */

import type { MimoMixedPartRow, MimoMixedState } from '$lib/protocol/mimoMixed';
import type { ActionId, ActionState, SetupStatus } from '../actions';
import { mixedBarActions, mixedIsLive, mixedModuleVerbs, mixedPrimary, partToggle, type MixedVerbContext } from '../mixed/mixedVerbs';

export interface MmixVerbContext {
	connected: boolean;
	controller: boolean;
	run: (Pick<MimoMixedState, 'state' | 'phase' | 'mode' | 'held'> & Partial<Pick<MimoMixedState, 'prediction'>>) | null;
	setup: SetupStatus;
	setupErrors: number;
	busy: string | null;
	pending: string | null;
}

export type MmixVerbId = 'hold' | 'resume' | 'override';

/** The gate's holds are a review to the shared bar: Proceed waits for the words. */
function mixedContext(ctx: MmixVerbContext): MixedVerbContext {
	const run = ctx.run;
	if (!run) return ctx as unknown as MixedVerbContext;
	const phase = run.phase === 'gate_failed' || run.phase === 'prediction_exceeded' ? 'awaiting_review' : run.phase;
	return { ...ctx, run: { ...run, phase } } as MixedVerbContext;
}

export function mmixIsLive(s: { state: MimoMixedState['state'] } | null | undefined): boolean {
	return mixedIsLive(s);
}

export function mmixBarActions(ctx: MmixVerbContext): Record<ActionId, ActionState> {
	const out = mixedBarActions(mixedContext(ctx));
	const p = ctx.run?.phase;
	if ((p === 'gate_failed' || p === 'prediction_exceeded') && out.proceed) out.proceed = { visible: true, enabled: false, reason: p === 'gate_failed' ? 'The coherence gate did not pass: override with your words, or stop.' : 'The prediction is over a drive ceiling: override with your words, or stop.' };
	else if (p === 'awaiting_review' && ctx.run?.prediction?.forecast.refused && out.proceed)
		out.proceed = { visible: true, enabled: false, reason: ctx.run.prediction.forecast.refused === 'budget_cannot_fit' ? 'The forecast finds no drive budget that fits the schedule; no override passes it. Change the setup, or stop.' : "The model is not usable at a tone's frequency; change the setup, or stop." };
	return out;
}

export function mmixPrimary(ctx: MmixVerbContext): ActionId | null {
	return mixedPrimary(mixedContext(ctx));
}

const hidden: ActionState = { visible: false, enabled: false, reason: null };

export function mmixModuleVerbs(ctx: MmixVerbContext, words: string): Record<MmixVerbId, ActionState> {
	const { hold, resume } = mixedModuleVerbs(mixedContext(ctx));
	const p = ctx.run?.phase;
	let override = hidden;
	if (ctx.run?.state === 'review' && (p === 'gate_failed' || p === 'prediction_exceeded')) {
		const reason = !ctx.connected ? 'The control service is not connected.' : !ctx.controller ? 'Take control to proceed.' : ctx.busy ? `Waiting for ${ctx.busy}.` : !words.trim() ? 'Write your words for the log first.' : null;
		override = { visible: true, enabled: reason === null, reason };
	}
	return { hold, resume, override };
}

export function mmixPartToggle(ctx: MmixVerbContext, row: Pick<MimoMixedPartRow, 'kind' | 'enabled'>): ActionState & { to: boolean } {
	return partToggle(mixedContext(ctx), row);
}
