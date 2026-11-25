/**
 * What a MIMO Mixed Mode run offers and says: Proceed waits past a failed gate or a prediction over
 * a ceiling, and the override needs the words; a forecast that refuses holds the Proceed with no
 * override; a part switch never takes the floor; plain words for every phase, outcome, end name and
 * refusal the service names, and the facts a refusal's detail carries; the drives' budget in words;
 * and the screen's plot types (the seven quantities the service draws, and no others), its cards
 * and the subscription it sends.
 */
import { describe, expect, it } from 'vitest';
import { MIMO_MIXED_END_NAMES, MIMO_MIXED_NOT_BUILT, MIMO_MIXED_OUTCOMES, MIMO_MIXED_PHASES, MIMO_MIXED_QUANTITIES, type MimoMixedPrediction, type MimoMixedState } from '$lib/protocol/mimoMixed';
import type { PlotPanel } from '../screen/screen.svelte';
import { MMIX_PLOT_TYPES, MMIX_QUANTITIES, MMIX_SCREEN, mmixSubscription } from './mmixLayout';
import { mmixBarActions, mmixModuleVerbs, mmixPartToggle, type MmixVerbContext } from './mmixVerbs';
import { LOG_WORDS } from '../words';
import { coherenceAgainst, endWords, gateWords, MMIX_END_WORDS, MMIX_LOG_WORDS, MMIX_OUTCOME_WORDS, MMIX_PHASE_WORDS, MMIX_REFUSAL_WORDS, mmixRefusalFacts, needWords, PART_REFUSED_WORDS, scaleWords } from './mmixWords';

const ctx = (run: Partial<MimoMixedState> | null): MmixVerbContext => ({
	connected: true,
	controller: true,
	run: run ? ({ state: 'running', phase: 'engaged', mode: 'auto', held: false, ...run } as MimoMixedState) : null,
	setup: 'applied',
	setupErrors: 0,
	busy: null,
	pending: null
});

const refusedForecast = { drives: [], controls: [], within: true, forecast: { step_s: 0.5, worst: null, cannot_fit: { drive: 5, time_s: 0, need_v: 0.75, ceiling_v: 0.5 }, refused: 'budget_cannot_fit' } } as MimoMixedPrediction;

describe('verbs', () => {
	it('proceeds from a passed review, and waits for an override with words past a failed gate or a prediction over a ceiling', () => {
		expect(mmixBarActions(ctx({ state: 'review', phase: 'awaiting_review' })).proceed.enabled).toBe(true);
		for (const phase of ['gate_failed', 'prediction_exceeded'] as const) {
			const bar = mmixBarActions(ctx({ state: 'review', phase }));
			expect(bar.proceed).toMatchObject({ visible: true, enabled: false });
			expect(bar.proceed.reason).toMatch(/override with your words/);
			const none = mmixModuleVerbs(ctx({ state: 'review', phase }), '  ');
			expect(none.override).toMatchObject({ visible: true, enabled: false, reason: 'Write your words for the log first.' });
			expect(mmixModuleVerbs(ctx({ state: 'review', phase }), 'Seen and accepted.').override.enabled).toBe(true);
		}
		expect(mmixModuleVerbs(ctx({ state: 'review', phase: 'awaiting_review' }), 'x').override.visible).toBe(false);
		expect(mmixModuleVerbs({ ...ctx({ state: 'review', phase: 'gate_failed' }), controller: false }, 'x').override.reason).toBe('Take control to proceed.');
	});

	it('holds the Proceed on a forecast that refuses, with no override offered', () => {
		const c = ctx({ state: 'review', phase: 'awaiting_review', prediction: refusedForecast });
		expect(mmixBarActions(c).proceed).toMatchObject({ visible: true, enabled: false });
		expect(mmixBarActions(c).proceed.reason).toMatch(/no override passes it/);
		expect(mmixModuleVerbs(c, 'words').override.visible).toBe(false);
	});

	it('holds once the floor plays, and never switches the floor', () => {
		expect(mmixModuleVerbs(ctx({ phase: 'floor_only' }), '').hold.enabled).toBe(true);
		expect(mmixModuleVerbs(ctx({ held: true }), '').resume.visible).toBe(true);
		expect(mmixPartToggle(ctx({}), { kind: 'floor', enabled: true }).visible).toBe(false);
		expect(mmixPartToggle(ctx({ phase: 'full_level' }), { kind: 'tone', enabled: true })).toMatchObject({ enabled: true, to: false });
	});

	it('offers the mode before the run and while it runs, never once it has finished', () => {
		expect(mmixBarActions(ctx({ state: 'configured', phase: 'idle' })).mode.enabled).toBe(true);
		expect(mmixBarActions(ctx({ state: 'running', phase: 'engaged' })).mode.enabled).toBe(true);
		expect(mmixBarActions(ctx({ state: 'finished', phase: 'stopped' })).mode.enabled).toBe(false);
	});
});

describe('words', () => {
	it('has plain words for every phase, outcome, end name and refusal the service names', () => {
		for (const p of MIMO_MIXED_PHASES) expect(MMIX_PHASE_WORDS[p], p).toBeTruthy();
		for (const o of MIMO_MIXED_OUTCOMES) expect(MMIX_OUTCOME_WORDS[o], o).toBeTruthy();
		for (const n of MIMO_MIXED_END_NAMES) expect(MMIX_END_WORDS[n], n).toBeTruthy();
		const refusals = ['not_configured', 'wrong_state', 'run_finished', 'run_active', 'shutting_down', 'config_invalid', 'config_unreadable', 'table_invalid', 'law_not_checked', 'arm_refused', 'record_unwritable', 'hardware_busy', 'gate_failed', 'prediction_exceeded', 'override_not_needed', 'budget_cannot_fit', 'tone_model_not_finite', 'manual_mode_required', 'not_a_rung', 'part_refused', 'bad_params', ...Object.values(MIMO_MIXED_NOT_BUILT)];
		for (const r of refusals) expect(MMIX_REFUSAL_WORDS[r as keyof typeof MMIX_REFUSAL_WORDS], r).toBeTruthy();
		for (const why of ['unknown_part', 'floor', 'band_folded']) expect(PART_REFUSED_WORDS[why], why).toBeTruthy();
	});

	it('has a short word for every kind the service logs', () => {
		const kinds = ['state', 'note', 'station', 'identification', 'engage', 'floor', 'level', 'alarm', 'part', 'limit', 'hold', 'override', 'record', 'stop'];
		const words: Record<string, string> = { ...LOG_WORDS, ...MMIX_LOG_WORDS };
		for (const k of kinds) {
			expect(words[k], k).toBeTruthy();
			expect(words[k].length, k).toBeLessThanOrEqual(9);
		}
	});

	it("never shows a failed gate's worst coherence at or over the gate", () => {
		const ident = { level_db: -20.04, steps: 2, frames: 64, lag_frames: 0, band_coherence: [0.999, 0.9991, null, 0.9995], gate_coherence: 0.999999, passed: false, drive_vrms: [] };
		expect(gateWords(ident)).toContain("did not pass: the worst band coherence is 0.999 against the gate's 0.999999");
		expect(coherenceAgainst(0.9937, 0.5)).toBe('0.99');
		expect(coherenceAgainst(0.99999, 0.99)).toBe('0.99');
		expect(coherenceAgainst(0.5, 0.5)).toBe('0.50');
		expect(gateWords({ ...ident, band_coherence: [0.9943, 0.9937], gate_coherence: 0.5, passed: true })).toContain("passed: the worst band coherence is 0.99 against the gate's 0.5");
	});

	it("says a refusal's detail in words, naming channels by the table", () => {
		const names = { channel: (c: number) => `ch ${c}`, part: (id: string) => (id === 't1' ? 'Tone 1' : id) };
		expect(mmixRefusalFacts('wrong_state', { state: 'review', phase: 'gate_failed' }, names)).toEqual(['The run is waiting for review: coherence gate not passed, waiting for you']);
		expect(mmixRefusalFacts('override_not_needed', { holds: 'prediction_exceeded' }, names)[0]).toMatch(/prediction is over a drive ceiling/);
		expect(mmixRefusalFacts('not_a_rung', { levels_db: [-6, -3, 0] }, names)).toEqual(['The levels of the schedule: -6 dB, -3 dB, 0 dB']);
		expect(mmixRefusalFacts('budget_cannot_fit', { drive: 5, time_s: 2, need_v: 3.2, ceiling_v: 3 }, names)).toEqual(['Drive ch 5 needs 3.2 V against its 3 V ceiling at 2 s']);
		expect(mmixRefusalFacts('part_refused', { id: 'floor', why: 'floor' }, names)).toEqual(['Part floor', 'The floor plays for the whole run; it cannot be switched off.']);
		expect(mmixRefusalFacts('tone_model_not_finite', { part: 't1', f_hz: 120 }, names)[0]).toMatch(/^Part Tone 1 at 120/);
	});

	it("says a drive's need against its ceiling, the common scale, and how a run ended by channel", () => {
		expect(needWords(2.1, 3)).toBe('2.1 V of 3 V (70 %)');
		expect(scaleWords(1, 'common_scale')).toMatch(/No drive is short of room/);
		expect(scaleWords(0.86, 'common_scale')).toBe('Every tone at 86 % of its target: a drive is out of room.');
		expect(scaleWords(0.5, 'in_turn')).toBe('Each tone is scaled in turn.');
		const names = { part: (id: string) => (id === 't1' ? 'Tone 1' : id), channel: (c: number) => `C${c}` };
		const end = { outcome: 'aborted', message: 'Stopped.', name: 'tone_abort', part: 't1', control: 2, drive: null, at_s: 3 } as const;
		expect(endWords(end as never, names)).toBe('Tone Abort on part Tone 1, control C2. Stopped.');
		const drive = { outcome: 'aborted', message: 'Over.', name: 'drive_vpk', part: null, control: null, drive: 6, at_s: 3 } as const;
		expect(endWords(drive as never, names)).toBe('Drive Peak Abort on drive C6. Over.');
		const abort = { outcome: 'operator_abort', message: 'Aborted.', name: null, part: null, control: null, drive: null, at_s: 3 } as const;
		expect(endWords(abort as never, names)).toBe('Aborted by the Operator. Aborted.');
	});
});

describe('the screen', () => {
	it('offers the seven quantities the service draws, and the parts, drives and limits cards', () => {
		expect(MMIX_PLOT_TYPES.map((t) => t.id)).toEqual([...MMIX_QUANTITIES]);
		expect([...MMIX_QUANTITIES]).toEqual([...MIMO_MIXED_QUANTITIES]);
		expect(MMIX_PLOT_TYPES.filter((t) => t.channels).map((t) => t.id)).toEqual(['control', 'residual', 'cross', 'tone_amplitude', 'tone_phase', 'band_level']);
		expect(MMIX_SCREEN.cards).toEqual(expect.arrayContaining(['run', 'parts', 'drives', 'limits', 'level', 'log']));
		expect(MMIX_SCREEN.cards).not.toContain('station');
		const control = MMIX_SCREEN.plots.find(([q]) => q === 'control')?.[1];
		expect(control?.families).toEqual(expect.arrayContaining(['tolerance', 'abort']));
	});

	it('sends a list of channels or none, never a named choice the service refuses', () => {
		const plot = (id: string, quantity: string, channels: unknown) => ({ id, kind: 'plot', at: { x: 0, y: 0, w: 1, h: 1 }, plot: { quantity, channels } }) as unknown as PlotPanel;
		expect(mmixSubscription([], 2)).toEqual({ default: true });
		expect(mmixSubscription([plot('a', 'control', 'all'), plot('b', 'residual', [1, 3]), plot('c', 'drive', 'all')], 2)).toEqual({
			rate_hz: 2,
			plots: [
				{ id: 'a', quantity: 'control' },
				{ id: 'b', quantity: 'residual', channels: [1, 3] },
				{ id: 'c', quantity: 'drive' }
			]
		});
	});
});
