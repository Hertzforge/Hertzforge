/**
 * The MIMO Mixed Mode run's screen: every quantity the service draws offered as a plot type, and
 * the pages a screen opens with, on the frame the pulse page layer shares (`../pulse/pulseScreen`).
 *
 * The pages until the operator changes them: Controls (each control's spectrum with the tones in,
 * against its reference and its alarm and abort lines), Residual (each control's autospectrum with
 * the tones out, against the floor's reference, with the cross terms), Tones (each tone's amplitude
 * and phase history per control, and each narrowband's level against its statement) and Drives
 * (each drive's spectrum, with the parts, drives and limits tables). The service draws seven
 * quantities and no others: the identification, its coherence and the laws' values are not drawn,
 * so the identification is read in the Run card while the run holds for review.
 */

import { MIMO_MIXED_QUANTITIES, type MimoMixedQuantity } from '$lib/protocol/mimoMixed';
import type { ChannelSelector } from '$lib/protocol/types';
import { pulsePlotTypes, pulseHooks, type PulseScreenSpec } from '../pulse/pulseScreen';
import type { PulsePageSet } from '../pulse/pulseLayout';
import type { PlotType } from '../plots/types';
import type { PlotPanel, ScreenHooks } from '../screen/screen.svelte';

export type MmixPageKind = 'controls' | 'residual' | 'tones' | 'drives';

export const MMIX_PAGE_KINDS: MmixPageKind[] = ['controls', 'residual', 'tones', 'drives'];

export const MMIX_PAGE_WORDS: Record<MmixPageKind, string> = {
	controls: 'Controls',
	residual: 'Residual',
	tones: 'Tones & Bands',
	drives: 'Drives'
};

export const MMIX_PAGE_QUANTITIES: Record<MmixPageKind, MimoMixedQuantity[]> = {
	controls: ['control'],
	residual: ['residual', 'cross'],
	tones: ['tone_amplitude', 'tone_phase', 'band_level'],
	drives: ['drive']
};

/** The quantities a channel choice applies to: the service picks controls by channel for every plot, the drives' plot draws every drive. */
export const MMIX_CHANNEL_QUANTITIES: readonly MimoMixedQuantity[] = ['control', 'residual', 'cross', 'tone_amplitude', 'tone_phase', 'band_level'];

/** Every quantity the service draws, in its order. */
export const MMIX_QUANTITIES: readonly MimoMixedQuantity[] = MIMO_MIXED_QUANTITIES;

export const MMIX_PAGES: PulsePageSet<MmixPageKind, MimoMixedQuantity> = {
	testType: 'mimo_mixed',
	kinds: MMIX_PAGE_KINDS,
	words: MMIX_PAGE_WORDS,
	quantities: MMIX_PAGE_QUANTITIES,
	timeQuantities: [],
	rates: [1, 2, 5, 10],
	// the service's own rate when a subscription names none
	defaultRate: 2,
	maxPages: 12,
	defaults: MMIX_PAGE_KINDS,
	channelQuantities: MMIX_CHANNEL_QUANTITIES,
	channels: {} as Partial<Record<MmixPageKind, ChannelSelector>>
};

export const MMIX_SCREEN: PulseScreenSpec<MmixPageKind, MimoMixedQuantity> = {
	pages: MMIX_PAGES,
	plots: [
		['control', { words: 'Control', help: "Each control's spectrum with the tones in, against the reference at the present level, with its alarm and abort lines.", group: 'frequency', families: ['measured', 'reference', 'tolerance', 'abort'] }],
		['residual', { words: 'Residual', help: "Each control's autospectrum with the tones taken out, against the floor's reference, with the cross terms.", group: 'frequency', families: ['measured', 'reference', 'other'] }],
		['cross', { words: 'Cross Terms', help: 'The magnitude of the cross spectrum of each pair of controls.', group: 'frequency', families: ['measured'] }],
		['drive', { words: 'Drives', help: "Each drive's spectrum as played, tones in.", group: 'frequency', families: ['drive'] }],
		['tone_amplitude', { words: 'Tone Amplitude', help: "Each tone's amplitude history at every control.", group: 'time', families: ['measured'] }],
		['tone_phase', { words: 'Tone Phase', help: "Each tone's phase history at every control.", group: 'time', families: ['measured'] }],
		['band_level', { words: 'Band Level', help: "Each narrowband's level against its statement, at every control.", group: 'time', families: ['measured'] }]
	],
	cards: ['run', 'parts', 'drives', 'limits', 'level', 'log'],
	pageCards: { tones: ['parts'], drives: ['drives', 'limits'] },
	selectors: ['all']
};

export const MMIX_PLOT_TYPES: PlotType[] = pulsePlotTypes(MMIX_SCREEN);

/**
 * What a window sends for the plots it shows. The service takes a plot's channels as a list of
 * channel numbers, or none for every control; it refuses a named choice, so "all" is sent as none.
 */
export function mmixSubscription(plots: PlotPanel[], rate: number): Record<string, unknown> {
	if (!plots.length) return { default: true };
	return {
		rate_hz: rate,
		plots: plots.map((p) => {
			const out: Record<string, unknown> = { id: p.id, quantity: p.plot.quantity };
			const c = p.plot.channels;
			if (MMIX_CHANNEL_QUANTITIES.includes(p.plot.quantity as MimoMixedQuantity) && Array.isArray(c)) out.channels = [...c];
			if (Array.isArray(p.plot.parts)) out.parts = [...p.plot.parts];
			return out;
		})
	};
}

/** The screen's hooks: the pulse layer's, with the service's own subscription. */
export function mmixHooks(feed: { method: string; topic: string }): ScreenHooks {
	const hooks = pulseHooks(MMIX_SCREEN, feed);
	return { ...hooks, feed: { method: feed.method, topic: feed.topic, params: (plots, rate) => mmixSubscription(plots, rate) } };
}
