/**
 * MIMO Shock's run screen: its plot types, its first pages with the verdict grid on the Verdicts
 * page, what a window asks the service for, and the last pulse's views standing in before a
 * window's own frame.
 */
import { describe, expect, it } from 'vitest';
import type { SpectrumView } from '$lib/protocol/types';
import type { PlotPanel } from '../screen/screen.svelte';
import { MS_PAGE_KINDS, MS_PLOT_TYPES, msSubscription } from './msLayout';
import { MS_HOOKS, standIn } from './msViews.svelte';

const plot = (id: string, quantity: string, channels: number[] | 'all' = 'all', points = 1024) => ({ id, kind: 'plot', plot: { quantity, channels, points } }) as unknown as PlotPanel;

describe('the MIMO Shock run screen', () => {
	it('offers every quantity the service draws, per control where the service draws one control', () => {
		expect(MS_PLOT_TYPES.map((t) => t.id)).toEqual(['pulse', 'srs', 'drives', 'verdicts', 'ladder', 'identification', 'review', 'law', 'readings', 'pulses']);
		expect(MS_PLOT_TYPES.filter((t) => t.channels).map((t) => t.id)).toEqual(['pulse', 'srs', 'identification']);
		expect(MS_PLOT_TYPES.filter((t) => t.time).map((t) => t.id)).toEqual(['pulse', 'drives']);
	});

	it('opens on a page of each kind, with the verdict grid on the Verdicts page and not in the dock', () => {
		const layout = MS_HOOKS.defaultLayout(1.6);
		expect(layout.pages.map((p) => p.name)).toEqual(['Pulses', 'SRS', 'Drives', 'Verdicts', 'Ladder', 'Identification', 'Law']);
		expect(layout.pages.length).toBe(MS_PAGE_KINDS.length);
		const verdicts = layout.pages.find((p) => p.name === 'Verdicts')!;
		expect(verdicts.panels.some((p) => p.kind === 'card' && p.card === 'verdicts')).toBe(true);
		expect(layout.dock.cards.find((c) => c.kind === 'verdicts')?.shown).toBe(false);
		expect(layout.dock.cards.find((c) => c.kind === 'pulse')?.shown).toBe(true);
	});

	it('asks for one control or every control, and a point count for the time histories only', () => {
		expect(msSubscription([], 2)).toEqual({ default: true });
		expect(msSubscription([plot('a', 'pulse', [3]), plot('b', 'srs'), plot('c', 'drives', 'all', 2048), plot('d', 'verdicts')], 2)).toEqual({
			rate_hz: 2,
			plots: [
				{ id: 'a', quantity: 'pulse', channel: 3, points: 1024 },
				{ id: 'b', quantity: 'srs' },
				{ id: 'c', quantity: 'drives', points: 2048 },
				{ id: 'd', quantity: 'verdicts' }
			]
		});
	});

	it("lets the last pulse's views stand in before a window's first frame", () => {
		const v = (title: string) => ({ view: 'spectrum', title, x: {}, y: {}, series: [] }) as unknown as SpectrumView;
		const last = [v('Pulse 1'), v('SRS 1'), v('Pulse 2'), v('Drives')];
		expect(standIn('pulse', last).map((x) => (x as SpectrumView).title)).toEqual(['Pulse 1', 'Pulse 2']);
		expect(standIn('srs', last).map((x) => (x as SpectrumView).title)).toEqual(['SRS 1']);
		expect(standIn('drives', last).map((x) => (x as SpectrumView).title)).toEqual(['Drives']);
		expect(standIn('ladder', last)).toEqual([]);
	});
});
