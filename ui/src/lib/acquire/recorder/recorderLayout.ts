/**
 * The Recorder run's pages and what each asks the service for (PROTOCOL.md, "Recorder Display"),
 * on the page layer the shock types, Sine, TWR and Explorer share (`../pulse/pulseLayout`). The
 * service draws every plot; the shell lays the limit marks on the Time page from the run's own
 * marks list.
 *
 * The pages until the operator changes them: Time (every recorded row's last seconds as an
 * envelope), Spectra (every recorded row's PSD), FRF (the pair's magnitude, phase and coherence;
 * the page says so when the run has no pair), Readings (the run in numbers) and Recordings (the
 * run's recordings and their facts, from the service's list; it asks for no plot).
 */

import type { RecorderLiveQuantity } from '$lib/protocol/recorder';
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

export const RECORDER_PAGE_HELP: Record<RecorderPageKind, string> = {
	time: "Every recorded row's last two seconds on the station's clock, each interval's largest and smallest sample. Limit marks are drawn where they fell.",
	spectra: "Every recorded row's PSD on the analysis grid, the average so far.",
	frf: "The pair's FRF (H1): its magnitude, its phase and the coherence.",
	readings: 'The run in numbers, and every mark.',
	recordings: "The run's recordings: each folder and its facts, the one being written last."
};

export type RecorderPageSpec = PulsePageSpec<RecorderPageKind>;
export type RecorderLayout = PulseLayout<RecorderPageKind>;
export type RecorderPlotSpec = PulsePlotSpec<RecorderLiveQuantity>;

export const RECORDER_RATES = [1, 2, 5, 10];
export const RECORDER_DEFAULT_RATE = 10;
export const MAX_RECORDER_PAGES = 12;

/** The quantities each page draws, in order. The Recordings page asks for none. */
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
	prefix: 'rc',
	kinds: RECORDER_PAGE_KINDS,
	words: RECORDER_PAGE_WORDS,
	help: RECORDER_PAGE_HELP,
	quantities: RECORDER_PAGE_QUANTITIES,
	timeQuantities: RECORDER_TIME_QUANTITIES,
	rates: RECORDER_RATES,
	defaultRate: RECORDER_DEFAULT_RATE,
	maxPages: MAX_RECORDER_PAGES,
	defaults: RECORDER_DEFAULT_PAGES,
	channelQuantities: RECORDER_CHANNEL_QUANTITIES,
	channels: RECORDER_PAGE_CHANNELS
};

export const recorderPlotId = pulsePlotId;

export function defaultRecorderLayout(): RecorderLayout {
	return defaultPulseLayout(RECORDER_PAGES);
}

/** Everything a page subscribes to, at its drawn width (CSS pixels). */
export function recorderPagePlots(page: RecorderPageSpec, width = 900, pixelRatio = 1): RecorderPlotSpec[] {
	return pulsePagePlots(RECORDER_PAGES, page, width, pixelRatio);
}

/** What a window sends: channels on the time and spectra plots, a point count on the time plot only. */
export function recorderSubscribeParams(plots: RecorderPlotSpec[], rate: number): Record<string, unknown> {
	return pulseSubscribeParams(RECORDER_PAGES, plots, rate);
}

export function recorderLayoutFromSaved(text: string | null): RecorderLayout | null {
	return pulseLayoutFromSaved(RECORDER_PAGES, text);
}

export function recorderLayoutToSaved(layout: RecorderLayout): string {
	return pulseLayoutToSaved(layout);
}
