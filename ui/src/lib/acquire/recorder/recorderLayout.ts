/**
 * The Recorder run's screen (PROTOCOL.md, "Recorder Display"): every quantity the service draws
 * offered as a plot type, and the page kinds a screen opens with, on the frame the pulse page layer
 * shares (`../pulse/pulseScreen`). The service draws every plot; the shell lays the limit marks on
 * a time plot from the run's own marks list.
 *
 * The pages until the operator changes them: Time (every recorded row's last seconds as an
 * envelope), Spectra (every recorded row's PSD), FRF (the pair's magnitude, phase and coherence;
 * the plot says so when the run has no pair), Readings (the run in numbers and every mark) and
 * Recordings (the run's recordings and their facts, from the service's list).
 */

import type { RecorderLiveQuantity } from '$lib/protocol/recorder';
import type { ChannelSelector } from '$lib/protocol/types';
import type { PlotType } from '../plots/types';
import type { PulsePageSet } from '../pulse/pulseLayout';
import { pulsePlotTypes, type PulseScreenSpec } from '../pulse/pulseScreen';

export type RecorderPageKind = 'time' | 'spectra' | 'frf' | 'readings' | 'recordings';

export const RECORDER_PAGE_KINDS: RecorderPageKind[] = ['time', 'spectra', 'frf', 'readings', 'recordings'];

/** The pages a new layout starts with. */
export const RECORDER_DEFAULT_PAGES: RecorderPageKind[] = ['time', 'spectra', 'frf', 'readings', 'recordings'];

export const RECORDER_PAGE_WORDS: Record<RecorderPageKind, string> = {
	time: 'Time',
	spectra: 'Spectra',
	frf: 'FRF',
	readings: 'Readings',
	recordings: 'Recordings'
};

export const RECORDER_RATES = [1, 2, 5, 10];
export const RECORDER_DEFAULT_RATE = 10;
export const MAX_RECORDER_PAGES = 12;

/** The quantities each page draws, in order. The Recordings page holds its card and asks for none. */
export const RECORDER_PAGE_QUANTITIES: Record<RecorderPageKind, RecorderLiveQuantity[]> = {
	time: ['time'],
	spectra: ['spectrum'],
	frf: ['frf'],
	readings: ['readings'],
	recordings: []
};

/** The quantities that take a channel selector. */
export const RECORDER_CHANNEL_QUANTITIES: readonly RecorderLiveQuantity[] = ['time', 'spectrum'];

export const RECORDER_PAGE_CHANNELS: Partial<Record<RecorderPageKind, ChannelSelector>> = {};

/** The quantity a point count is sent with: the time history. */
export const RECORDER_TIME_QUANTITIES: readonly RecorderLiveQuantity[] = ['time'];

/** Every quantity the service offers, in the order its result lists them. */
export const RECORDER_QUANTITIES: readonly RecorderLiveQuantity[] = ['time', 'spectrum', 'frf', 'readings'];

export const RECORDER_PAGES: PulsePageSet<RecorderPageKind, RecorderLiveQuantity> = {
	testType: 'recorder',
	kinds: RECORDER_PAGE_KINDS,
	words: RECORDER_PAGE_WORDS,
	quantities: RECORDER_PAGE_QUANTITIES,
	timeQuantities: RECORDER_TIME_QUANTITIES,
	rates: RECORDER_RATES,
	defaultRate: RECORDER_DEFAULT_RATE,
	maxPages: MAX_RECORDER_PAGES,
	defaults: RECORDER_DEFAULT_PAGES,
	channelQuantities: RECORDER_CHANNEL_QUANTITIES,
	channels: RECORDER_PAGE_CHANNELS
};

/** The Recorder's screen: its plot types, cards and first pages; the marks and the recordings on pages of their own. */
export const RECORDER_SCREEN: PulseScreenSpec<RecorderPageKind, RecorderLiveQuantity> = {
	pages: RECORDER_PAGES,
	plots: [
		['time', { words: 'Time History', help: "Each recorded row's last seconds on the station's clock, each interval's largest and smallest sample; limit marks where they fell.", group: 'time', families: ['measured'] }],
		['spectrum', { words: 'Spectra', help: "Each recorded row's PSD on the analysis grid, the average so far.", group: 'frequency', families: ['measured'] }],
		['frf', { words: 'FRF', help: "The pair's FRF (H1): its magnitude, its phase and the coherence.", group: 'frequency', families: ['measured'] }],
		['readings', { words: 'Readings', help: 'The run in numbers.', group: 'numbers' }]
	],
	cards: ['run', 'recording', 'recmarks', 'recordings', 'station'],
	pageCards: { readings: ['recmarks'], recordings: ['recordings'] },
	selectors: ['all']
};

export const RECORDER_PLOT_TYPES: PlotType[] = pulsePlotTypes(RECORDER_SCREEN);
