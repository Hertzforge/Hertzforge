/**
 * The MIMO Sine run's screen (PROTOCOL.md, "MIMO Sine Display"): every quantity the service draws
 * offered as a plot type, and the page kinds a screen opens with, on the frame the pulse page layer
 * shares (`../pulse/pulseScreen`). The service draws every plot: against frequency on the run's
 * sweep grid in a sweep or a stepped sine, against test time in a dwell.
 *
 * The pages until the operator changes them: Controls (each control's amplitude against its
 * reference with its alarm and abort bands, and its phase against its reference phase), Error
 * (every control's error), Limits (each limit channel against its bound), Drives (every drive's
 * amplitude and phase), Identification (the response from every drive to a control, and every
 * channel's coherence with the trust line) and Law (the law's declared values and the run's
 * readings).
 */

import type { MimoSineQuantity } from '$lib/protocol/mimoSine';
import type { ChannelSelector } from '$lib/protocol/types';
import { pulsePlotTypes, type PulseScreenSpec } from '../pulse/pulseScreen';
import type { PulsePageSet } from '../pulse/pulseLayout';
import type { PlotType } from '../plots/types';

export type MsinePageKind = 'controls' | 'error' | 'limits' | 'drives' | 'identification' | 'law' | 'time';

export const MSINE_PAGE_KINDS: MsinePageKind[] = ['controls', 'error', 'limits', 'drives', 'identification', 'law', 'time'];

/** The pages a new layout starts with. */
export const MSINE_DEFAULT_PAGES: MsinePageKind[] = ['controls', 'error', 'limits', 'drives', 'identification', 'law'];

export const MSINE_PAGE_WORDS: Record<MsinePageKind, string> = {
	controls: 'Controls',
	error: 'Error',
	limits: 'Limits',
	drives: 'Drives',
	identification: 'Identification',
	law: 'Law',
	time: 'Time History'
};

/** The quantities each page draws, in order. */
export const MSINE_PAGE_QUANTITIES: Record<MsinePageKind, MimoSineQuantity[]> = {
	controls: ['control', 'phase'],
	error: ['error', 'readings'],
	limits: ['limit'],
	drives: ['drive', 'drive_phase'],
	identification: ['identification'],
	law: ['law', 'readings'],
	time: ['time']
};

/** The quantities that take a channel selector (a list of table indexes, or every channel). */
export const MSINE_CHANNEL_QUANTITIES: readonly MimoSineQuantity[] = ['control', 'phase', 'limit', 'time', 'identification'];

/** The one quantity drawn from the batch's samples: the only plot that takes a point count. */
export const MSINE_TIME_QUANTITIES: readonly MimoSineQuantity[] = ['time'];

/** Every quantity the service offers, in the order its result lists them. */
export const MSINE_QUANTITIES: readonly MimoSineQuantity[] = ['control', 'phase', 'error', 'limit', 'drive', 'drive_phase', 'readings', 'time', 'identification', 'law'];

export const MSINE_PAGES: PulsePageSet<MsinePageKind, MimoSineQuantity> = {
	testType: 'mimo_sine',
	kinds: MSINE_PAGE_KINDS,
	words: MSINE_PAGE_WORDS,
	quantities: MSINE_PAGE_QUANTITIES,
	timeQuantities: MSINE_TIME_QUANTITIES,
	rates: [1, 2, 5, 10],
	defaultRate: 5,
	maxPages: 12,
	defaults: MSINE_DEFAULT_PAGES,
	channelQuantities: MSINE_CHANNEL_QUANTITIES,
	channels: {} as Partial<Record<MsinePageKind, ChannelSelector>>
};

export const MSINE_SCREEN: PulseScreenSpec<MsinePageKind, MimoSineQuantity> = {
	pages: MSINE_PAGES,
	plots: [
		['control', { words: 'Control', help: "Each control's amplitude against its reference at the present level, with its alarm and abort bands.", group: 'frequency', families: ['measured', 'reference', 'tolerance', 'abort'] }],
		['phase', { words: 'Phase', help: "Each control's phase against its reference phase, relative to the tone.", group: 'frequency', families: ['measured', 'reference'] }],
		['error', { words: 'Error', help: "Every control's error against its reference in dB, and the notch depth while one is taken.", group: 'frequency', families: ['measured'] }],
		['limit', { words: 'Limit', help: "Each limit channel's amplitude against its bound.", group: 'frequency', families: ['measured', 'limit'] }],
		['drive', { words: 'Drives', help: 'Every drive as played, in volts peak.', group: 'frequency', families: ['drive'] }],
		['drive_phase', { words: 'Drive Phase', help: "Every drive's phase relative to the tone.", group: 'frequency', families: ['drive'] }],
		['readings', { words: 'Readings', help: 'The opposition between the shakers and the largest share one carries, and the run in numbers.', group: 'frequency', families: ['measured'] }],
		['time', { words: 'Time History', help: "Each channel's samples from the last batch.", group: 'time', families: ['measured'] }],
		['identification', { words: 'Identification', help: 'The response from every drive to a control, its phase, and every channel\'s coherence with the trust line.', group: 'frequency', families: ['measured', 'tolerance'] }],
		['law', { words: 'Law Values', help: "The control law's declared values on its last call, its unusable calls, the ticks a guard bound and its requests.", group: 'numbers' }]
	],
	cards: ['run', 'level', 'law'],
	selectors: ['all']
};

export const MSINE_PLOT_TYPES: PlotType[] = pulsePlotTypes(MSINE_SCREEN);
