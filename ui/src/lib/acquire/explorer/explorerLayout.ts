/**
 * The Explorer run's pages and what each asks the service for (PROTOCOL.md, "Explorer Display"), on
 * the page layer the shock types, Sine and TWR share (`../pulse/pulseLayout`). The service draws
 * every plot; the shell marks the tone and the survey's peak on the survey line from the run's own
 * numbers.
 *
 * The pages until the operator changes them: Survey (the response level at each frequency the
 * tone has visited, on a log axis), Spectra (every acquired channel's latest display spectrum),
 * Time (the control row's most recent samples) and Readings (the run in numbers).
 */

import type { ExplorerLiveQuantity } from '$lib/protocol/explorer';
import type { ChannelSelector } from '$lib/protocol/types';
import {
	defaultPulseLayout,
	pulseLayoutFromSaved,
	pulseLayoutToSaved,
	pulsePagePlots,
	pulsePlotId,
	pulseSubscribeParams,
	type PulseLayout,
	type PulsePageSet,
	type PulsePageSpec,
	type PulsePlotSpec
} from '../pulse/pulseLayout';

export type ExplorerPageKind = 'survey' | 'spectra' | 'time' | 'readings';

export const EXPLORER_PAGE_KINDS: ExplorerPageKind[] = ['survey', 'spectra', 'time', 'readings'];

export const EXPLORER_DEFAULT_PAGES: ExplorerPageKind[] = ['survey', 'spectra', 'time', 'readings'];

export const EXPLORER_PAGE_WORDS: Record<ExplorerPageKind, string> = {
	survey: 'Survey',
	spectra: 'Spectra',
	time: 'Time',
	readings: 'Readings'
};

export const EXPLORER_PAGE_HELP: Record<ExplorerPageKind, string> = {
	survey: "The control channel's response level at each frequency the tone has visited, kept after the tone moves on. The tone and the highest point are marked.",
	spectra: "Every acquired channel's latest display spectrum.",
	time: "The control channel's most recent samples, on the station's clock.",
	readings: 'The run in numbers.'
};

export type ExplorerPageSpec = PulsePageSpec<ExplorerPageKind>;
export type ExplorerLayout = PulseLayout<ExplorerPageKind>;
export type ExplorerPlotSpec = PulsePlotSpec<ExplorerLiveQuantity>;

export const EXPLORER_RATES = [1, 2, 5, 10];
export const EXPLORER_DEFAULT_RATE = 10;
export const MAX_EXPLORER_PAGES = 12;

/** The quantities each page draws, in order. */
export const EXPLORER_PAGE_QUANTITIES: Record<ExplorerPageKind, ExplorerLiveQuantity[]> = {
	survey: ['survey'],
	spectra: ['spectrum'],
	time: ['time'],
	readings: ['readings']
};

/** The quantity that takes a channel selector. */
export const EXPLORER_CHANNEL_QUANTITIES: readonly ExplorerLiveQuantity[] = ['spectrum'];

export const EXPLORER_PAGE_CHANNELS: Partial<Record<ExplorerPageKind, ChannelSelector>> = {};

/**
 * The quantity a point count is sent with: the time history. The survey is drawn on its own
 * points (the service's default, `points` null), so it is sent without one.
 */
export const EXPLORER_TIME_QUANTITIES: readonly ExplorerLiveQuantity[] = ['time'];

/** Every quantity the service offers, in the order its result lists them. */
export const EXPLORER_QUANTITIES: readonly ExplorerLiveQuantity[] = ['survey', 'spectrum', 'time', 'readings'];

export const EXPLORER_PAGES: PulsePageSet<ExplorerPageKind, ExplorerLiveQuantity> = {
	testType: 'explorer',
	prefix: 'xp',
	kinds: EXPLORER_PAGE_KINDS,
	words: EXPLORER_PAGE_WORDS,
	help: EXPLORER_PAGE_HELP,
	quantities: EXPLORER_PAGE_QUANTITIES,
	timeQuantities: EXPLORER_TIME_QUANTITIES,
	rates: EXPLORER_RATES,
	defaultRate: EXPLORER_DEFAULT_RATE,
	maxPages: MAX_EXPLORER_PAGES,
	defaults: EXPLORER_DEFAULT_PAGES,
	channelQuantities: EXPLORER_CHANNEL_QUANTITIES,
	channels: EXPLORER_PAGE_CHANNELS
};

export const explorerPlotId = pulsePlotId;

export function defaultExplorerLayout(): ExplorerLayout {
	return defaultPulseLayout(EXPLORER_PAGES);
}

/** Everything a page subscribes to, at its drawn width (CSS pixels). */
export function explorerPagePlots(page: ExplorerPageSpec, width = 900, pixelRatio = 1): ExplorerPlotSpec[] {
	return pulsePagePlots(EXPLORER_PAGES, page, width, pixelRatio);
}

/** What a window sends: channels on the spectra, a point count on the time history only. */
export function explorerSubscribeParams(plots: ExplorerPlotSpec[], rate: number): Record<string, unknown> {
	return pulseSubscribeParams(EXPLORER_PAGES, plots, rate);
}

export function explorerLayoutFromSaved(text: string | null): ExplorerLayout | null {
	return pulseLayoutFromSaved(EXPLORER_PAGES, text);
}

export function explorerLayoutToSaved(layout: ExplorerLayout): string {
	return pulseLayoutToSaved(layout);
}
