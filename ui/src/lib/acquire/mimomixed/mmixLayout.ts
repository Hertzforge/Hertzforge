/**
 * The MIMO Mixed Mode run's screen: every quantity the service draws offered as a plot type, and
 * the pages a screen opens with, on the frame the pulse page layer shares (`../pulse/pulseScreen`).
 *
 * The pages until the operator changes them: Controls (each control's spectrum with the tones in,
 * against its reference), Residual (each control's autospectrum with the tones out, against the
 * floor's reference with the alarm and abort lines, and the chosen cross terms against theirs),
 * Tones (each tone's amplitude and phase history per control, and each narrowband's level),
 * Drives (each drive's spectrum, with the parts and drives tables), Identification and Law.
 */

import type { MimoMixedQuantity } from '$lib/protocol/mimoMixed';
import type { ChannelSelector } from '$lib/protocol/types';
import { pulsePlotTypes, type PulseScreenSpec } from '../pulse/pulseScreen';
import type { PulsePageSet } from '../pulse/pulseLayout';
import type { PlotType } from '../plots/types';

export type MmixPageKind = 'controls' | 'residual' | 'tones' | 'drives' | 'identification' | 'law';

export const MMIX_PAGE_KINDS: MmixPageKind[] = ['controls', 'residual', 'tones', 'drives', 'identification', 'law'];

export const MMIX_PAGE_WORDS: Record<MmixPageKind, string> = {
	controls: 'Controls',
	residual: 'Residual',
	tones: 'Tones & Bands',
	drives: 'Drives',
	identification: 'Identification',
	law: 'Laws'
};

export const MMIX_PAGE_QUANTITIES: Record<MmixPageKind, MimoMixedQuantity[]> = {
	controls: ['control'],
	residual: ['residual', 'cross'],
	tones: ['tone_amplitude', 'tone_phase', 'band_level'],
	drives: ['drive'],
	identification: ['identification', 'coherence'],
	law: ['law', 'readings']
};

export const MMIX_CHANNEL_QUANTITIES: readonly MimoMixedQuantity[] = ['control', 'residual', 'cross', 'drive'];

export const MMIX_QUANTITIES: readonly MimoMixedQuantity[] = ['control', 'residual', 'cross', 'drive', 'tone_amplitude', 'tone_phase', 'band_level', 'identification', 'coherence', 'law', 'readings'];

export const MMIX_PAGES: PulsePageSet<MmixPageKind, MimoMixedQuantity> = {
	testType: 'mimo_mixed',
	kinds: MMIX_PAGE_KINDS,
	words: MMIX_PAGE_WORDS,
	quantities: MMIX_PAGE_QUANTITIES,
	timeQuantities: [],
	rates: [1, 2, 5, 10],
	defaultRate: 5,
	maxPages: 12,
	defaults: MMIX_PAGE_KINDS,
	channelQuantities: MMIX_CHANNEL_QUANTITIES,
	channels: {} as Partial<Record<MmixPageKind, ChannelSelector>>
};

export const MMIX_SCREEN: PulseScreenSpec<MmixPageKind, MimoMixedQuantity> = {
	pages: MMIX_PAGES,
	plots: [
		['control', { words: 'Control', help: "Each control's spectrum with the tones in, against the reference at the present level.", group: 'frequency', families: ['measured', 'reference'] }],
		['residual', { words: 'Residual', help: "Each control's autospectrum with the tones taken out, against the floor's reference with its alarm and abort lines.", group: 'frequency', families: ['measured', 'reference', 'tolerance', 'abort'] }],
		['cross', { words: 'Cross Terms', help: "The chosen pairs' coherence and phase against the reference's.", group: 'frequency', families: ['measured', 'reference'] }],
		['drive', { words: 'Drives', help: "Each drive's spectrum as played, tones in.", group: 'frequency', families: ['drive'] }],
		['tone_amplitude', { words: 'Tone Amplitude', help: "Each tone's amplitude history at every control, against its target.", group: 'time', families: ['measured', 'reference'] }],
		['tone_phase', { words: 'Tone Phase', help: "Each tone's phase history at every control against the first, against its stated phase.", group: 'time', families: ['measured', 'reference'] }],
		['band_level', { words: 'Band Level', help: "Each narrowband's level history at every control, against its target.", group: 'time', families: ['measured', 'reference'] }],
		['identification', { words: 'Identification', help: 'The model from every drive to every control.', group: 'frequency', families: ['measured'] }],
		['coherence', { words: 'Coherence', help: "Each control's multiple coherence with the drives, with the gate's line.", group: 'frequency', families: ['measured', 'tolerance'] }],
		['law', { words: 'Law Values', help: "Each law's declared values on its last call.", group: 'numbers' }],
		['readings', { words: 'Readings', help: 'The run in numbers: the floor, each tone and each drive.', group: 'numbers' }]
	],
	cards: ['run', 'parts', 'drives', 'level', 'log', 'station'],
	pageCards: { tones: ['parts'], drives: ['drives'] },
	selectors: ['all', 'control']
};

export const MMIX_PLOT_TYPES: PlotType[] = pulsePlotTypes(MMIX_SCREEN);
