/**
 * The Mixed Mode run's screen (PROTOCOL.md, "Mixed Mode Display"): every quantity the service draws
 * offered as a plot type, the parts table as a card, and the pages a screen opens with, on the frame
 * the pulse page layer shares (`../pulse/pulseScreen`).
 *
 * The pages until the operator changes them: Control (the control spectrum with the tones in, and
 * the residual the floor and the narrowbands are judged on, each against the reference with its
 * alarm and abort lines), Parts (the parts table beside each part's frequency), Tones (each tone's
 * amplitude against its target and bands), Narrowbands (each band's level in its own moving frame
 * against its widened bands), Drive, Identification and Law. The per-part plots (amplitude,
 * frequency, level) take a part choice; the service resolves it on every frame.
 */

import type { MixedLiveQuantity, MixedSubscribePlot } from '$lib/protocol/mixed';
import type { ChannelSelector } from '$lib/protocol/types';
import { pulsePlotTypes, pulseScreenLayout, type PulseScreenSpec } from '../pulse/pulseScreen';
import type { PulsePageSet } from '../pulse/pulseLayout';
import type { PartSelector, PlotType } from '../plots/types';
import { layoutKey, lastLayoutKey } from '../display/layout';
import { DEFAULT_ASPECT } from '../screen/layout';
import type { PlotPanel, ScreenHooks } from '../screen/screen.svelte';
import type { ScreenLayout } from '../screen/types';

export type MixedPageKind = 'control' | 'parts' | 'tones' | 'bands' | 'drive' | 'responses' | 'identification' | 'law' | 'time';

export const MIXED_PAGE_KINDS: MixedPageKind[] = ['control', 'parts', 'tones', 'bands', 'drive', 'responses', 'identification', 'law', 'time'];

/** The pages a new layout starts with. */
export const MIXED_DEFAULT_PAGES: MixedPageKind[] = ['control', 'parts', 'tones', 'bands', 'drive', 'identification', 'law'];

export const MIXED_PAGE_WORDS: Record<MixedPageKind, string> = {
	control: 'Control',
	parts: 'Parts',
	tones: 'Tones',
	bands: 'Narrowbands',
	drive: 'Drive',
	responses: 'Responses',
	identification: 'Identification',
	law: 'Law',
	time: 'Time History'
};

export const MIXED_RATES = [1, 2, 5, 10];
export const MIXED_DEFAULT_RATE = 5;
export const MAX_MIXED_PAGES = 12;

export const MIXED_PAGE_QUANTITIES: Record<MixedPageKind, MixedLiveQuantity[]> = {
	control: ['control', 'residual'],
	parts: ['frequency'],
	tones: ['amplitude'],
	bands: ['level'],
	drive: ['drive'],
	responses: ['response'],
	identification: ['identification', 'coherence'],
	law: ['law', 'readings'],
	time: ['time']
};

/** Which parts each page's per-part plots ask for. */
export const MIXED_PAGE_PARTS: Partial<Record<MixedPageKind, PartSelector>> = { parts: 'all', tones: 'tones', bands: 'bands' };

export const MIXED_CHANNEL_QUANTITIES: readonly MixedLiveQuantity[] = ['response', 'time'];
export const MIXED_TIME_QUANTITIES: readonly MixedLiveQuantity[] = ['time'];
/** The per-part histories and the parts each takes. */
export const MIXED_PART_QUANTITIES: Partial<Record<MixedLiveQuantity, 'tones' | 'bands' | 'both'>> = { amplitude: 'tones', frequency: 'both', level: 'bands' };

/** Every quantity the service offers, in the order its result lists them. */
export const MIXED_QUANTITIES: readonly MixedLiveQuantity[] = ['control', 'residual', 'drive', 'response', 'time', 'coherence', 'identification', 'amplitude', 'frequency', 'level', 'law', 'readings'];

export const MIXED_PAGES: PulsePageSet<MixedPageKind, MixedLiveQuantity> = {
	testType: 'mixed',
	kinds: MIXED_PAGE_KINDS,
	words: MIXED_PAGE_WORDS,
	quantities: MIXED_PAGE_QUANTITIES,
	timeQuantities: MIXED_TIME_QUANTITIES,
	rates: MIXED_RATES,
	defaultRate: MIXED_DEFAULT_RATE,
	maxPages: MAX_MIXED_PAGES,
	defaults: MIXED_DEFAULT_PAGES,
	channelQuantities: MIXED_CHANNEL_QUANTITIES,
	channels: {}
};

/** Mixed Mode's screen: its plot types, cards and first pages. */
export const MIXED_SCREEN: PulseScreenSpec<MixedPageKind, MixedLiveQuantity> = {
	pages: MIXED_PAGES,
	plots: [
		['control', { words: 'Control', help: 'The control spectrum with the tones in, against the reference (the floor and each narrowband where it is now) with its alarm and abort lines.', group: 'frequency', families: ['measured', 'reference', 'tolerance', 'abort'] }],
		['residual', { words: 'Residual', help: 'The control spectrum with each tracked tone taken out: what the floor and the narrowbands are judged on.', group: 'frequency', families: ['measured', 'reference', 'tolerance', 'abort'] }],
		['drive', { words: 'Drive', help: 'The random part of the drive (floor and narrowbands) as made for the next segment.', group: 'frequency', families: ['drive'] }],
		['response', { words: 'Response', help: "Each input's spectrum with the tones in, and a limit channel's ceiling.", group: 'frequency', families: ['measured', 'limit'] }],
		['time', { words: 'Time History', help: "Each input's samples from the last loop, tones in.", group: 'time', families: ['measured'] }],
		['coherence', { words: 'Coherence', help: "The identification's coherence across the band.", group: 'frequency', families: ['measured', 'tolerance'] }],
		['identification', { words: 'Identification', help: 'The identified gain from the drive to the control point, and where the tones may trust it.', group: 'frequency', families: ['measured', 'reference'] }],
		['amplitude', { words: 'Tone Amplitude', help: "Each tone's tracked amplitude against its target and its alarm and abort lines, over the last minute.", group: 'time', families: ['measured', 'reference', 'tolerance', 'abort'] }],
		['frequency', { words: 'Part Frequency', help: "Each tone's frequency or narrowband's center, over the last minute.", group: 'time', families: ['reference'] }],
		['level', { words: 'Narrowband Level', help: "Each narrowband's level in its own moving frame against its reference and its widened alarm and abort lines.", group: 'time', families: ['measured', 'reference', 'tolerance', 'abort'] }],
		['law', { words: 'Law Values', help: "The floor law's and each tone law's values on their last calls.", group: 'numbers' }],
		['readings', { words: 'Readings', help: 'The run in numbers, a row for each part that is on.', group: 'numbers' }]
	],
	cards: ['run', 'level', 'parts', 'law', 'station'],
	pageCards: { parts: ['parts'] },
	selectors: ['all', 'control', 'limited']
};

/** The plot types: the pulse layer's, with the per-part histories marked as taking parts. */
export const MIXED_PLOT_TYPES: PlotType[] = pulsePlotTypes(MIXED_SCREEN).map((t) => {
	const parts = MIXED_PART_QUANTITIES[t.id as MixedLiveQuantity];
	return parts ? { ...t, parts } : t;
});

/** A layout of these pages, each per-part plot asking for its page's parts. */
export function mixedScreenLayout(pages: readonly { name: string; kind: MixedPageKind }[], rate = MIXED_DEFAULT_RATE, aspect = DEFAULT_ASPECT): ScreenLayout {
	const layout = pulseScreenLayout(MIXED_SCREEN, MIXED_PLOT_TYPES, pages, rate, aspect);
	layout.pages.forEach((page, i) => {
		const want = MIXED_PAGE_PARTS[pages[i].kind];
		for (const p of page.panels) if (p.kind === 'plot' && p.plot.parts !== undefined && want) p.plot.parts = want;
	});
	return layout;
}

export function defaultMixedScreen(aspect = DEFAULT_ASPECT): ScreenLayout {
	return mixedScreenLayout(
		MIXED_DEFAULT_PAGES.map((kind) => ({ name: MIXED_PAGE_WORDS[kind], kind })),
		MIXED_DEFAULT_RATE,
		aspect
	);
}

/**
 * What a window sends for the plots it shows, or the default subscription when it shows none. A
 * channel selector goes with the plots that take one, a part selector with the per-part histories,
 * and a point count with the time history only: the service refuses either on any other plot.
 */
export function mixedSubscription(plots: PlotPanel[], rate: number): Record<string, unknown> {
	if (!plots.length) return { default: true };
	return {
		rate_hz: rate,
		plots: plots.map((p): MixedSubscribePlot => {
			const type = MIXED_PLOT_TYPES.find((t) => t.id === p.plot.quantity);
			const out: MixedSubscribePlot = { id: p.id, quantity: p.plot.quantity as MixedLiveQuantity };
			if (type?.channels) {
				const c = p.plot.channels as ChannelSelector;
				out.channels = Array.isArray(c) ? [...c] : c;
			}
			if (type?.parts) {
				const s = p.plot.parts ?? 'all';
				out.parts = Array.isArray(s) ? [...s] : s;
			}
			if (type?.time) out.points = p.plot.points;
			return out;
		})
	};
}

/** Everything Mixed Mode gives its screen: plot types, cards, first pages and its feed. */
export function mixedHooks(feed: { method: string; topic: string }): ScreenHooks {
	return {
		testType: 'mixed',
		types: MIXED_PLOT_TYPES,
		cards: MIXED_SCREEN.cards,
		defaultLayout: (aspect) => defaultMixedScreen(aspect),
		legacy: { key: (title) => layoutKey('mixed', title), last: lastLayoutKey('mixed'), read: () => null },
		feed: { method: feed.method, topic: feed.topic, params: mixedSubscription }
	};
}
