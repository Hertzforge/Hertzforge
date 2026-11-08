/**
 * What a Sine run offers now, and why a verb waits. The run bar's verbs are Random's: the same
 * states and reasons, the level step and the ladder mode included, since `sine.level` and
 * `sine.mode` are `random.level` and `random.mode` for Sine. Sine adds its own: Hold and Resume,
 * and on a sweep Reverse, Jog and Go to. And a run's law may declare parameters the operator
 * changes during the run (`sine.adjust`); every other parameter is fixed once the run starts.
 */

import type { LawEntry, LawParam } from '$lib/protocol/mimo';
import type { SineLawRef, SineState } from '$lib/protocol/sine';
import type { RandomState } from '$lib/protocol/types';
import { DRIVING_PHASES, LIVE_STATES, primaryAction, runActions, type ActionId, type ActionState, type SetupStatus } from '../actions';

export interface SineVerbContext {
	connected: boolean;
	controller: boolean;
	/** The latest numbers: the state with the live frame laid over it. */
	run: Pick<SineState, 'state' | 'phase' | 'mode' | 'test' | 'held'> | null;
	setup: SetupStatus;
	setupErrors: number;
	busy: string | null;
	pending: string | null;
}

export type SineVerbId = 'hold' | 'resume' | 'reverse' | 'jog' | 'goto';

export const SINE_LIVE_STATES = LIVE_STATES;

export function sineIsLive(s: { state: SineState['state'] } | null | undefined): boolean {
	return !!s && LIVE_STATES.includes(s.state);
}

const hidden: ActionState = { visible: false, enabled: false, reason: null };

function runContext(ctx: SineVerbContext) {
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
export function sineBarActions(ctx: SineVerbContext): Record<ActionId, ActionState> {
	return runActions(runContext(ctx));
}

export function sinePrimary(ctx: SineVerbContext): ActionId | null {
	return primaryAction(runContext(ctx));
}

/** Hold or Resume, and on a sweep Reverse, Jog and Go to, with why each waits. */
export function sineModuleVerbs(ctx: SineVerbContext): Record<SineVerbId, ActionState> {
	const run = ctx.run;
	const running = run?.state === 'running';
	const driving = running && !!run && DRIVING_PHASES.includes(run.phase);
	const sweep = run?.test === 'sweep';
	const held = !!run?.held;
	const gate = (visible: boolean, verb: string, own: string | null): ActionState => {
		if (!visible) return hidden;
		let reason: string | null = null;
		if (!ctx.connected) reason = 'The control service is not connected.';
		else if (!ctx.controller) reason = `Take control to ${verb}.`;
		else if (ctx.busy) reason = `Waiting for ${ctx.busy}.`;
		else reason = own;
		return { visible, enabled: reason === null, reason };
	};
	const notDriving = driving ? null : 'Once the tone is driving at the schedule.';
	return {
		hold: running && !held ? gate(true, 'hold the test', notDriving) : hidden,
		resume: running && held ? gate(true, 'resume the test', null) : hidden,
		reverse: running && sweep ? gate(true, 'turn the sweep', notDriving) : hidden,
		jog: running && sweep ? gate(true, 'move the frequency', notDriving) : hidden,
		goto: running && sweep ? gate(true, 'move the frequency', notDriving) : hidden
	};
}

/** A law parameter as the run's live card shows it. */
export interface LiveParamState {
	key: string;
	label: string;
	help: string;
	unit: string;
	min: number;
	max: number;
	step: number;
	/** The value in force: the run's, else the setup's. */
	value: number | null;
	/** The law declares it changeable during a run. */
	live: boolean;
	enabled: boolean;
	reason: string | null;
}

const isNumber = (p: LawParam): p is Extract<LawParam, { kind: 'number' }> | Extract<LawParam, { kind: 'integer' }> => p.kind === 'number' || p.kind === 'integer';

/**
 * Every number parameter of the run's law, the live ones first: each live one may be changed while
 * the run is running or holding for review, by the screen in control; the others are fixed.
 */
export function liveParams(
	law: LawEntry | null,
	ref: Pick<SineLawRef, 'live'> | null,
	values: Record<string, unknown>,
	ctx: Pick<SineVerbContext, 'connected' | 'controller' | 'busy' | 'run'>
): LiveParamState[] {
	if (!law) return [];
	const live = new Set(law.live ?? []);
	const open = ctx.run?.state === 'running' || ctx.run?.state === 'review';
	const out = law.params.filter(isNumber).map((p): LiveParamState => {
		const isLive = live.has(p.key);
		let reason: string | null = null;
		if (!isLive) reason = 'Fixed for the run.';
		else if (!open) reason = 'Changed during a run.';
		else if (!ctx.connected) reason = 'The control service is not connected.';
		else if (!ctx.controller) reason = 'Take control to change it.';
		else if (ctx.busy) reason = `Waiting for ${ctx.busy}.`;
		const inForce = ref?.live?.[p.key];
		const set = values[p.key];
		return {
			key: p.key,
			label: p.label,
			help: p.help,
			unit: p.unit,
			min: p.min,
			max: p.max,
			step: p.step,
			value: typeof inForce === 'number' ? inForce : typeof set === 'number' ? set : p.default,
			live: isLive,
			enabled: reason === null,
			reason
		};
	});
	return [...out.filter((p) => p.live), ...out.filter((p) => !p.live)];
}

/** The words for a value the card would send that is outside the parameter's range, or null. */
export function liveValueError(p: Pick<LiveParamState, 'min' | 'max' | 'unit'>, v: number | null): string | null {
	if (v === null || !Number.isFinite(v)) return 'Enter a number.';
	if (v < p.min || v > p.max) return `From ${p.min} to ${p.max}${p.unit ? ` ${p.unit}` : ''}.`;
	return null;
}
