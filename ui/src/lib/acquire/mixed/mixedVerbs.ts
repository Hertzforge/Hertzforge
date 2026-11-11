/**
 * What a Mixed Mode run offers now, and why a verb waits. The run bar's verbs are Random's: the
 * same states and reasons, the level step and the ladder mode included, since `mixed.level` and
 * `mixed.mode` are Random's for Mixed Mode. Mixed Mode adds Hold and Resume (every part's clock
 * stops and starts together), a part switched on or off live (`mixed.part`, once the parts have
 * come in), and the tone law's live parameters (`mixed.adjust`, once the laws are built).
 */

import type { LawEntry } from '$lib/protocol/mimo';
import type { MixedLawRef, MixedPartRow, MixedState } from '$lib/protocol/mixed';
import type { RandomState } from '$lib/protocol/types';
import { DRIVING_PHASES, LIVE_STATES, primaryAction, runActions, type ActionId, type ActionState, type SetupStatus } from '../actions';
import { liveParams, type LiveParamState } from '../sine/sineVerbs';

export interface MixedVerbContext {
	connected: boolean;
	controller: boolean;
	/** The latest numbers: the state with the live frame laid over it. */
	run: Pick<MixedState, 'state' | 'phase' | 'mode' | 'held'> | null;
	setup: SetupStatus;
	setupErrors: number;
	busy: string | null;
	pending: string | null;
}

export type MixedVerbId = 'hold' | 'resume';

export function mixedIsLive(s: { state: MixedState['state'] } | null | undefined): boolean {
	return !!s && LIVE_STATES.includes(s.state);
}

const hidden: ActionState = { visible: false, enabled: false, reason: null };

function runContext(ctx: MixedVerbContext) {
	return {
		connected: ctx.connected,
		role: ctx.controller ? ('controller' as const) : ('observer' as const),
		run: ctx.run as unknown as RandomState | null,
		setup: ctx.setup,
		setupErrors: ctx.setupErrors,
		busy: ctx.busy,
		pending: ctx.pending
	};
}

/** The run bar's verbs, by the shared bar's names: Random's rules. */
export function mixedBarActions(ctx: MixedVerbContext): Record<ActionId, ActionState> {
	return runActions(runContext(ctx));
}

export function mixedPrimary(ctx: MixedVerbContext): ActionId | null {
	return primaryAction(runContext(ctx));
}

function gate(ctx: Pick<MixedVerbContext, 'connected' | 'controller' | 'busy'>, verb: string, own: string | null): ActionState {
	let reason: string | null = null;
	if (!ctx.connected) reason = 'The control service is not connected.';
	else if (!ctx.controller) reason = `Take control to ${verb}.`;
	else if (ctx.busy) reason = `Waiting for ${ctx.busy}.`;
	else reason = own;
	return { visible: true, enabled: reason === null, reason };
}

/** The phases in which the floor is driving: alone, or with the parts. */
const FLOOR_DRIVING = ['floor_only', 'engaged', 'full_level'];

/** Hold or Resume, with why each waits. */
export function mixedModuleVerbs(ctx: MixedVerbContext): Record<MixedVerbId, ActionState> {
	const run = ctx.run;
	const running = run?.state === 'running';
	const held = !!run?.held;
	const driving = running && !!run && FLOOR_DRIVING.includes(run.phase);
	return {
		hold: running && !held ? gate(ctx, 'hold the test', driving ? null : 'Once the floor is driving.') : hidden,
		resume: running && held ? gate(ctx, 'resume the test', null) : hidden
	};
}

/**
 * Switching one part on or off live: offered for a tone or a narrowband once the parts have come
 * in (`engaged` or `full_level`), never for the floor. The row's own `on` says which way it goes.
 */
export function partToggle(ctx: MixedVerbContext, row: Pick<MixedPartRow, 'kind' | 'enabled'>): ActionState & { to: boolean } {
	const to = !row.enabled;
	if (row.kind === 'floor') return { ...hidden, to };
	const run = ctx.run;
	if (run?.state !== 'running') return { visible: true, enabled: false, reason: 'During a run.', to };
	if (!DRIVING_PHASES.includes(run.phase as never)) return { visible: true, enabled: false, reason: 'Once the parts have come in.', to };
	return { ...gate(ctx, to ? 'switch the part on' : 'switch the part off', null), to };
}

/**
 * The tone law's number parameters during the run: the live ones changeable once the laws are built
 * (from the floor's engage: `floor_only`, `engaged`, `full_level`), the others fixed. Sine's rules,
 * with the floor alone counted as driving.
 */
export function mixedLiveParams(
	law: LawEntry | null,
	ref: Pick<MixedLawRef, 'live'> | null,
	values: Record<string, unknown>,
	ctx: Pick<MixedVerbContext, 'connected' | 'controller' | 'busy' | 'run'>
): LiveParamState[] {
	const run = ctx.run;
	const phase = run && run.phase === 'floor_only' ? 'engaged' : (run?.phase ?? 'idle');
	return liveParams(law, ref, values, { ...ctx, run: run ? ({ state: run.state, phase, mode: run.mode, held: run.held, test: null } as never) : null });
}
