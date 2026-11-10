/**
 * The Explorer run's screen (PROTOCOL.md, "Explorer Display"): every quantity the service draws
 * offered as a plot type, and the page kinds a screen opens with, on the frame the pulse page layer
 * shares (`../pulse/pulseScreen`). The service draws every plot; the shell marks the tone and the
 * survey's peak on the survey line from the run's own numbers.
 *
 * The pages until the operator changes them: Survey (the response level at each frequency the
 * tone has visited, on a log axis), Spectra (every acquired channel's latest display spectrum),
 * Time (the control row's most recent samples) and Readings (the run in numbers).
 */

import type { ExplorerLiveQuantity } from '$lib/protocol/explorer';
import type { ChannelSelector } from '$lib/protocol/types';
import type { PlotType } from '../plots/types';
import type { PulsePageSet } from '../pulse/pulseLayout';
import { pulsePlotTypes, type PulseScreenSpec } from '../pulse/pulseScreen';

export type ExplorerPageKind = 'survey' | 'spectra' | 'time' | 'readings';

export const EXPLORER_PAGE_KINDS: ExplorerPageKind[] = ['survey', 'spectra', 'time', 'readings'];

export const EXPLORER_DEFAULT_PAGES: ExplorerPageKind[] = ['survey', 'spectra', 'time', 'readings'];

export const EXPLORER_PAGE_WORDS: Record<ExplorerPageKind, string> = {
	survey: 'Survey',
	spectra: 'Spectra',
	time: 'Time',
	readings: 'Readings'
};

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
	kinds: EXPLORER_PAGE_KINDS,
	words: EXPLORER_PAGE_WORDS,
	quantities: EXPLORER_PAGE_QUANTITIES,
	timeQuantities: EXPLORER_TIME_QUANTITIES,
	rates: EXPLORER_RATES,
	defaultRate: EXPLORER_DEFAULT_RATE,
	maxPages: MAX_EXPLORER_PAGES,
	defaults: EXPLORER_DEFAULT_PAGES,
	channelQuantities: EXPLORER_CHANNEL_QUANTITIES,
	channels: EXPLORER_PAGE_CHANNELS
};

/** Explorer's screen: its plot types, cards and first pages. */
export const EXPLORER_SCREEN: PulseScreenSpec<ExplorerPageKind, ExplorerLiveQuantity> = {
	pages: EXPLORER_PAGES,
	plots: [
		['survey', { words: 'Survey', help: "The control channel's response level at each frequency the tone has visited; the tone and the highest point marked.", group: 'frequency', families: ['measured'] }],
		['spectrum', { words: 'Spectra', help: "Each acquired channel's latest display spectrum.", group: 'frequency', families: ['measured'] }],
		['time', { words: 'Time History', help: "The control channel's most recent samples, on the station's clock.", group: 'time', families: ['measured'] }],
		['readings', { words: 'Readings', help: 'The run in numbers.', group: 'numbers' }]
	],
	cards: ['run', 'level', 'steer', 'station'],
	selectors: ['all', 'control']
};

export const EXPLORER_PLOT_TYPES: PlotType[] = pulsePlotTypes(EXPLORER_SCREEN);
