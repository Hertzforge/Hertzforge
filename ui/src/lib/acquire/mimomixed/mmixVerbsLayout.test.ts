/**
 * What a MIMO Mixed Mode run offers and says: Proceed waits past a failed gate or a forecast that
 * cannot fit, and Proceed With Your Words needs the words; a part switch never takes the floor; the
 * drives' budget in words; and the screen's plot types, every quantity offered, and its cards.
 */
import { describe, expect, it } from 'vitest';
import type { MimoMixedState } from '$lib/protocol/mimoMixed';
import { MMIX_PLOT_TYPES, MMIX_QUANTITIES, MMIX_SCREEN } from './mmixLayout';
import { mmixBarActions, mmixModuleVerbs, mmixPartToggle, type MmixVerbContext } from './mmixVerbs';
import { endWords, needWords, scaleWords } from './mmixWords';

const ctx = (run: Partial<MimoMixedState> | null): MmixVerbContext => ({
	connected: true,
	controller: true,
	run: run ? ({ state: 'running', phase: 'engaged', mode: 'auto', held: false, ...run } as MimoMixedState) : null,
	setup: 'applied',
	setupErrors: 0,
	busy: null,
	pending: null
});

describe('verbs', () => {
	it('proceeds from a passed review, and waits for the words past a failed gate or a forecast that cannot fit', () => {
		expect(mmixBarActions(ctx({ state: 'review', phase: 'awaiting_review' })).proceed.enabled).toBe(true);
		for (const phase of ['gate_failed', 'prediction_exceeded'] as const) {
			const bar = mmixBarActions(ctx({ state: 'review', phase }));
			expect(bar.proceed).toMatchObject({ visible: true, enabled: false });
			expect(bar.proceed.reason).toMatch(/your words/);
			const none = mmixModuleVerbs(ctx({ state: 'review', phase }), '  ');
			expect(none.override).toMatchObject({ visible: true, enabled: false, reason: 'Write your words for the log first.' });
			expect(mmixModuleVerbs(ctx({ state: 'review', phase }), 'Seen and accepted.').override.enabled).toBe(true);
		}
		expect(mmixModuleVerbs(ctx({ state: 'review', phase: 'awaiting_review' }), 'x').override.visible).toBe(false);
		expect(mmixModuleVerbs({ ...ctx({ state: 'review', phase: 'gate_failed' }), controller: false }, 'x').override.reason).toBe('Take control to proceed.');
	});

	it('holds once the floor plays, and never switches the floor', () => {
		expect(mmixModuleVerbs(ctx({ phase: 'floor_only' }), '').hold.enabled).toBe(true);
		expect(mmixModuleVerbs(ctx({ held: true }), '').resume.visible).toBe(true);
		expect(mmixPartToggle(ctx({}), { kind: 'floor', enabled: true }).visible).toBe(false);
		expect(mmixPartToggle(ctx({ phase: 'full_level' }), { kind: 'tone', enabled: true })).toMatchObject({ enabled: true, to: false });
	});
});

describe('words', () => {
	it("says a drive's need against its ceiling, the common scale, and how a run ended", () => {
		expect(needWords(2.1, 3)).toBe('2.1 V of 3 V (70 %)');
		expect(scaleWords(1, 'common_scale')).toMatch(/No drive is short of room/);
		expect(scaleWords(0.86, 'common_scale')).toBe('Every tone at 86 % of its target: a drive is out of room.');
		expect(scaleWords(0.5, 'in_turn')).toBe('Each tone is scaled in turn.');
		const names = { part: (id: string) => (id === 't1' ? 'Tone 1' : id), control: (i: number) => `C${i + 1}`, drive: (i: number) => `D${i + 1}` };
		const end = { outcome: 'aborted', message: 'Stopped.', name: 'tone_abort', part: 't1', control: 1, drive: null, at_s: 3 } as const;
		expect(endWords(end as never, names)).toMatch(/on part Tone 1, control C2\. Stopped\.$/);
	});
});

describe('the screen', () => {
	it('offers every quantity as a plot type, and the parts and drives cards', () => {
		expect(MMIX_PLOT_TYPES.map((t) => t.id)).toEqual([...MMIX_QUANTITIES]);
		expect(MMIX_PLOT_TYPES.filter((t) => t.channels).map((t) => t.id)).toEqual(['control', 'residual', 'cross', 'drive']);
		expect(MMIX_SCREEN.cards).toEqual(expect.arrayContaining(['run', 'parts', 'drives', 'level', 'log', 'station']));
		const residual = MMIX_SCREEN.plots.find(([q]) => q === 'residual')?.[1];
		expect(residual?.families).toEqual(expect.arrayContaining(['tolerance', 'abort']));
	});
});
