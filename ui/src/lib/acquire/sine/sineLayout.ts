/**
 * The Sine run's screen (PROTOCOL.md, "Sine Display"): every quantity the service draws offered as
 * a plot type, and the page kinds a screen opens with, on the frame the pulse page layer shares
 * (`../pulse/pulseScreen`). The service draws every plot: against frequency on the run's sweep grid
 * in a sweep or a step test, against test time in a dwell.
 *
 * The pages until the operator changes them: Control (the control amplitude against the profile
 * with its alarm and abort bands, and the drive as played), Limits (each limit channel against its
 * bound), Responses (every input's tracked amplitude and its phase against the drive),
 * Identification (the identified gain with its trust floor and the coherence) and Law (the law's
 * values on its last call and the run in numbers). A layout kept with pages of these kinds, or of
 * the Phase, Distortion and Time History kinds, is taken over as it was.
 */

import type { SineLiveQuantity } from '$lib/protocol/sine';
import type { ChannelSelector } from '$lib/protocol/types';
import { pulsePlotTypes, type PulseScreenSpec } from '../pulse/pulseScreen';
import type { PulsePageSet } from '../pulse/pulseLayout';
import type { PlotType } from '../plots/types';

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
	kinds: SINE_PAGE_KINDS,
	words: SINE_PAGE_WORDS,
	quantities: SINE_PAGE_QUANTITIES,
	timeQuantities: SINE_TIME_QUANTITIES,
	rates: SINE_RATES,
	defaultRate: SINE_DEFAULT_RATE,
	maxPages: MAX_SINE_PAGES,
	defaults: SINE_DEFAULT_PAGES,
	channelQuantities: SINE_CHANNEL_QUANTITIES,
	channels: SINE_PAGE_CHANNELS
};

/** Sine's screen: its plot types, cards and first pages. */
export const SINE_SCREEN: PulseScreenSpec<SinePageKind, SineLiveQuantity> = {
	pages: SINE_PAGES,
	plots: [
		['control', { words: 'Control', help: 'The control amplitude against the profile at the present level, with its alarm and abort bands.', group: 'frequency', families: ['measured', 'reference', 'tolerance', 'abort'] }],
		['drive', { words: 'Drive', help: 'The drive as played.', group: 'frequency', families: ['drive'] }],
		['response', { words: 'Response', help: "Each input's tracked amplitude at the tone's frequency, and a limit channel's bound.", group: 'frequency', families: ['measured', 'limit'] }],
		['phase', { words: 'Phase', help: "Each input's phase against the drive.", group: 'frequency', families: ['measured'] }],
		['thd', { words: 'Distortion', help: "The control channel's total harmonic distortion.", group: 'frequency', families: ['measured'] }],
		['time', { words: 'Time History', help: "Each input's samples from the last batch.", group: 'time', families: ['measured'] }],
		['identification', { words: 'Identification', help: 'The identified gain to the control channels with its trust floor, and the coherence with the trusted frequencies.', group: 'frequency', families: ['measured', 'reference', 'tolerance'] }],
		['law', { words: 'Law Values', help: "The control law's values on its last call.", group: 'numbers' }],
		['readings', { words: 'Readings', help: 'The run in numbers.', group: 'numbers' }]
	],
	cards: ['run', 'level', 'law', 'station']
};

export const SINE_PLOT_TYPES: PlotType[] = pulsePlotTypes(SINE_SCREEN);
