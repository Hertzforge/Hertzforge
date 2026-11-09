/**
 * The Sine run's pages and what each asks the service for (PROTOCOL.md, "Sine Display"), on the
 * page layer the shock types share (`../pulse/pulseLayout`). The service draws every plot: against
 * frequency on the run's sweep grid in a sweep or a step test, against test time in a dwell.
 *
 * The pages until the operator changes them: Control (the control amplitude against the profile
 * with its alarm and abort bands, and the drive as played), Limits (each limit channel against its
 * bound), Responses (every input's tracked amplitude and its phase against the drive),
 * Identification (the identified gain with its trust floor and the coherence) and Law (the law's
 * values on its last call and the run in numbers). Phase, Distortion and Time History pages can be
 * added.
 */

import type { SineLiveQuantity } from '$lib/protocol/sine';
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

export type SinePageKind = 'control' | 'limits' | 'responses' | 'identification' | 'law' | 'phase' | 'distortion' | 'time';

export const SINE_PAGE_KINDS: SinePageKind[] = ['control', 'limits', 'responses', 'identification', 'law', 'phase', 'distortion', 'time'];

/** The pages a new layout starts with. */
export const SINE_DEFAULT_PAGES: SinePageKind[] = ['control', 'limits', 'responses', 'identification', 'law'];

export const SINE_PAGE_WORDS: Record<SinePageKind, string> = {
	control: 'Control',
	limits: 'Limits',
	responses: 'Responses',
	identification: 'Identification',
	law: 'Law',
	phase: 'Phase',
	distortion: 'Distortion',
	time: 'Time History'
};

export const SINE_PAGE_HELP: Record<SinePageKind, string> = {
	control: 'The control amplitude against the profile at the present level, with its alarm and abort bands, and the drive as played.',
	limits: 'Each limit channel against its bound.',
	responses: "Every input's tracked amplitude at the tone's frequency, and its phase against the drive.",
	identification: 'The identified gain to each control channel with its trust floor, and the coherence with the trusted frequencies.',
	law: "The control law's values on its last call, and the run in numbers.",
	phase: "Every input's phase against the drive.",
	distortion: "The control channel's total harmonic distortion.",
	time: "Every input's samples from the last batch."
};

export type SinePageSpec = PulsePageSpec<SinePageKind>;
export type SineLayout = PulseLayout<SinePageKind>;
export type SinePlotSpec = PulsePlotSpec<SineLiveQuantity>;

export const SINE_RATES = [1, 2, 5, 10];
export const SINE_DEFAULT_RATE = 5;
export const MAX_SINE_PAGES = 12;

/** The quantities each page draws, in order. */
export const SINE_PAGE_QUANTITIES: Record<SinePageKind, SineLiveQuantity[]> = {
	control: ['control', 'drive'],
	limits: ['response'],
	responses: ['response', 'phase'],
	identification: ['identification'],
	law: ['law', 'readings'],
	phase: ['phase'],
	distortion: ['thd'],
	time: ['time']
};

/** The quantities that take a channel selector. */
export const SINE_CHANNEL_QUANTITIES: readonly SineLiveQuantity[] = ['response', 'phase', 'time'];

/** The channels each page asks for; every input on the others. */
export const SINE_PAGE_CHANNELS: Partial<Record<SinePageKind, ChannelSelector>> = { limits: 'limited' };

/** The one quantity drawn from the batch's samples: the only plot that takes a point count. */
export const SINE_TIME_QUANTITIES: readonly SineLiveQuantity[] = ['time'];

/** Every quantity the service offers, in the order its result lists them. */
export const SINE_QUANTITIES: readonly SineLiveQuantity[] = ['control', 'drive', 'response', 'phase', 'thd', 'time', 'identification', 'law', 'readings'];

export const SINE_PAGES: PulsePageSet<SinePageKind, SineLiveQuantity> = {
	testType: 'sine',
	prefix: 'np',
	kinds: SINE_PAGE_KINDS,
	words: SINE_PAGE_WORDS,
	help: SINE_PAGE_HELP,
	quantities: SINE_PAGE_QUANTITIES,
	timeQuantities: SINE_TIME_QUANTITIES,
	rates: SINE_RATES,
	defaultRate: SINE_DEFAULT_RATE,
	maxPages: MAX_SINE_PAGES,
	defaults: SINE_DEFAULT_PAGES,
	channelQuantities: SINE_CHANNEL_QUANTITIES,
	channels: SINE_PAGE_CHANNELS
};

export const sinePlotId = pulsePlotId;

export function defaultSineLayout(): SineLayout {
	return defaultPulseLayout(SINE_PAGES);
}

/** Everything a page subscribes to, at its drawn width (CSS pixels). */
export function sinePagePlots(page: SinePageSpec, width = 900, pixelRatio = 1): SinePlotSpec[] {
	return pulsePagePlots(SINE_PAGES, page, width, pixelRatio);
}

/** What a window sends: channels on the plots that take them, a point count on the time history only. */
export function sineSubscribeParams(plots: SinePlotSpec[], rate: number): Record<string, unknown> {
	return pulseSubscribeParams(SINE_PAGES, plots, rate);
}

export function sineLayoutFromSaved(text: string | null): SineLayout | null {
	return pulseLayoutFromSaved(SINE_PAGES, text);
}

export function sineLayoutToSaved(layout: SineLayout): string {
	return pulseLayoutToSaved(layout);
}
