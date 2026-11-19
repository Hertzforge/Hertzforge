/**
 * What a Mixed Mode run offers and says: which verbs apply when, a part's switch (never the floor's,
 * and only once the parts have come in), the tone law's live parameters from the floor's engage, the
 * level card's words, a part's motion in words, a trip's facts naming its part, and the screen's
 * plot types and first pages (every quantity of the protocol offered, the per-part plots asking for
 * their page's parts, the parts table on the Parts page).
 */
import { describe, expect, it } from 'vitest';
import type { MixedPartRow, MixedState } from '$lib/protocol/mixed';
import type { LawEntry } from '$lib/protocol/mimo';
import { defaultMixedScreen, MIXED_PLOT_TYPES, MIXED_QUANTITIES, mixedSubscription } from './mixedLayout';
import { mixedLiveParams, mixedModuleVerbs, partToggle, type MixedVerbContext } from './mixedVerbs';
import { mixedLevelStatus, mixedTripFacts, partMotion } from './mixedWords';
import type { PlotPanel } from '../screen/screen.svelte';

const ctx = (run: Partial<MixedState> | null): MixedVerbContext => ({
	connected: true,
	controller: true,
	run: run ? ({ state: 'running', phase: 'engaged', mode: 'auto', held: false, ...run } as MixedState) : null,
	setup: 'applied',
	setupErrors: 0,
	busy: null,
	pending: null
});

const row = (over: Partial<MixedPartRow>): MixedPartRow =>
	({ id: 't1', kind: 'tone', name: 'Tone 1', enabled: true, on: true, scale: 1, freq_hz: 412.6, direction: 'up', sweeps: { done: 1, of: 4 }, rate_oct_per_min: 1, rate_hz_per_s: null, width_hz: null, unit: 'g', target: 1, measured: 1, error_db: 0, alarm: false, drive_vpk: 0.3, drive_vrms: null, gain_db: null, settled: true, limiting: false, dof: null, lines: null, law: { misses: 0, capped: 0 }, ...over }) as MixedPartRow;

describe('verbs', () => {
	it('a part switch: never the floor, and only once the parts have come in', () => {
		expect(partToggle(ctx({}), row({ kind: 'floor' })).visible).toBe(false);
		const early = partToggle(ctx({ phase: 'floor_only' }), row({}));
		expect(early).toMatchObject({ visible: true, enabled: false, reason: 'Once the parts have come in.' });
		expect(partToggle(ctx({ phase: 'full_level' }), row({ enabled: true }))).toMatchObject({ enabled: true, to: false });
		expect(partToggle(ctx({}), row({ enabled: false }))).toMatchObject({ enabled: true, to: true });
		expect(partToggle({ ...ctx({}), controller: false }, row({})).reason).toBe('Take control to switch the part off.');
	});

	it('hold while the floor drives, resume while held', () => {
		expect(mixedModuleVerbs(ctx({ phase: 'floor_only' })).hold.enabled).toBe(true);
		expect(mixedModuleVerbs(ctx({ phase: 'sysid' })).hold.enabled).toBe(false);
		expect(mixedModuleVerbs(ctx({ held: true })).resume.visible).toBe(true);
	});

	it("the tone law's live parameters may change from the floor's engage", () => {
		const law = { id: 'servo_blend', live: ['blend_w'], params: [{ key: 'blend_w', label: 'Prediction weight', help: '', kind: 'number', unit: '', min: 0, max: 1, step: 0.05, default: 0.5 }] } as unknown as LawEntry;
		expect(mixedLiveParams(law, null, {}, ctx({ phase: 'floor_only' }))[0].enabled).toBe(true);
		expect(mixedLiveParams(law, null, {}, ctx({ phase: 'sysid' }))[0].enabled).toBe(false);
	});
});

describe('words', () => {
	it('says what the run and each part are doing', () => {
		expect(mixedLevelStatus({ state: 'running', phase: 'floor_only', level_db: -12, held: false } as MixedState, null).text).toBe('Floor alone at -12.0 dB, equalizing');
		expect(partMotion(row({}), false)).toBe('Sweeping up at 1 oct/min, sweep 2 of 4');
		expect(partMotion(row({ direction: null, sweeps: null, rate_oct_per_min: null }), false)).toBe('Standing');
		expect(partMotion(row({ on: false, enabled: false }), false)).toBe('Off');
		expect(partMotion(row({ scale: 0.4 }), false)).toBe('Ramping in');
	});

	it("a trip's facts name the part by its name", () => {
		const f = mixedTripFacts({ kind: 'band_abort', part: 'b2', side: 'high', measured_g2_hz: 0.2, limit_g2_hz: 0.15, dof: 120, at_level_db: 0, at_s: 9, at_loop: 3, at_tick: 900, at_hz: 1400 }, (id) => (id === 'b2' ? 'Rotor band' : id));
		expect(f[0]).toEqual({ label: 'Part', value: 'Rotor band' });
		expect(f.map((x) => x.label)).toContain('Degrees of freedom');
	});
});

describe('the screen', () => {
	it('offers every quantity of the protocol as a plot type; the per-part ones take parts', () => {
		expect(MIXED_PLOT_TYPES.map((t) => t.id).sort()).toEqual([...MIXED_QUANTITIES].sort());
		expect(MIXED_PLOT_TYPES.filter((t) => t.parts).map((t) => [t.id, t.parts])).toEqual([
			['amplitude', 'tones'],
			['frequency', 'both'],
			['level', 'bands']
		]);
	});

	it('opens with the parts table on the Parts page and each per-part plot asking for its page parts', () => {
		const l = defaultMixedScreen();
		expect(l.pages.map((p) => p.name)).toEqual(['Control', 'Parts', 'Tones', 'Narrowbands', 'Drive', 'Identification', 'Law']);
		const parts = l.pages[1];
		expect(parts.panels.some((p) => p.kind === 'card' && p.card === 'parts')).toBe(true);
		expect(l.dock.cards.find((c) => c.kind === 'parts')?.shown).toBe(false);
		const sub = (i: number) => mixedSubscription(l.pages[i].panels.filter((p): p is PlotPanel => p.kind === 'plot'), 5) as { plots: { quantity: string; parts?: unknown; channels?: unknown }[] };
		expect(sub(2).plots).toEqual([{ id: expect.any(String), quantity: 'amplitude', parts: 'tones' }]);
		expect(sub(3).plots).toEqual([{ id: expect.any(String), quantity: 'level', parts: 'bands' }]);
		expect(sub(0).plots.every((p) => p.parts === undefined && p.channels === undefined)).toBe(true);
	});
});
