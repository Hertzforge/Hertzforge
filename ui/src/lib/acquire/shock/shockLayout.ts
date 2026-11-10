/**
 * The classical shock run's screen (PROTOCOL.md, "Classical Shock Display"): every quantity the
 * service draws offered as a plot type, the page kinds a screen opens with, and the pulses an
 * operator picks in the pulse table to lay over the Pulse and SRS plots, on the frame the pulse
 * page layer shares (`../pulse/pulseScreen`).
 *
 * The pages until the operator changes them: Pulse (the reference at the level fired with its
 * tolerance band, the measured pulse and the drive played), SRS, Kinematics (velocity and
 * displacement), Identification (|H|, phase and coherence, and the pulse's spectra), Pulses (the
 * pulse table and the run's numbers) and Law (the law's reported values).
 */

import type { ShockQuantity } from '$lib/protocol/shock';
import type { PlotType } from '../plots/types';
import type { PulsePageSet } from '../pulse/pulseLayout';
import { pulsePlotTypes, type PulseScreenSpec } from '../pulse/pulseScreen';

export { MAX_OVERLAY, pulseViewOf, toggleOverlay, withOverlay } from '../pulse/pulseLayout';

export type ShockPageKind = 'pulse' | 'srs' | 'kinematics' | 'identification' | 'pulses' | 'law';

export const SHOCK_PAGE_KINDS: ShockPageKind[] = ['pulse', 'srs', 'kinematics', 'identification', 'pulses', 'law'];

export const SHOCK_PAGE_WORDS: Record<ShockPageKind, string> = {
	pulse: 'Pulse',
	srs: 'SRS',
	kinematics: 'Kinematics',
	identification: 'Identification',
	pulses: 'Pulses',
	law: 'Law'
};

export const SHOCK_PAGE_HELP: Record<ShockPageKind, string> = {
	pulse: 'The reference at the level fired with its tolerance band, the measured pulse and the drive played.',
	srs: "The measured pulse's shock response spectrum against the reference's, with the informational band.",
	kinematics: 'Velocity and displacement of the measured pulse and of the reference.',
	identification: "The identified system's magnitude and phase, the coherence with the trusted lines marked, and the pulse's spectra.",
	pulses: 'Every pulse fired, with its verdicts, and the run in numbers.',
	law: "The control law's reported values on its last call, and the run in numbers."
};

export const SHOCK_RATES = [1, 2, 5];
export const SHOCK_DEFAULT_RATE = 2;
export const MAX_SHOCK_PAGES = 12;

/** The quantities each page draws, in order. */
export const PAGE_QUANTITIES: Record<ShockPageKind, ShockQuantity[]> = {
	pulse: ['pulse'],
	srs: ['srs'],
	kinematics: ['velocity', 'displacement'],
	identification: ['identification', 'spectrum'],
	pulses: ['readings'],
	law: ['law', 'readings']
};

/** The quantities drawn as time histories: the only plots that take a point count. */
export const TIME_QUANTITIES: readonly ShockQuantity[] = ['pulse', 'velocity', 'displacement'];

/** Classical shock's pages, as the shared run view reads them. */
export const SHOCK_PAGES: PulsePageSet<ShockPageKind, ShockQuantity> = {
	testType: 'shock',
	prefix: 'sp',
	kinds: SHOCK_PAGE_KINDS,
	words: SHOCK_PAGE_WORDS,
	help: SHOCK_PAGE_HELP,
	quantities: PAGE_QUANTITIES,
	timeQuantities: TIME_QUANTITIES,
	rates: SHOCK_RATES,
	defaultRate: SHOCK_DEFAULT_RATE,
	maxPages: MAX_SHOCK_PAGES
};

/** Classical shock's screen: its plot types, cards and first pages; the pulse table on the Pulses page. */
export const SHOCK_SCREEN: PulseScreenSpec<ShockPageKind, ShockQuantity> = {
	pages: SHOCK_PAGES,
	plots: [
		['pulse', { words: 'Pulse', help: 'The reference at the level fired with its tolerance band, the measured pulse, and the drive played.', group: 'time', families: ['measured', 'reference', 'tolerance', 'drive'] }],
		['velocity', { words: 'Velocity', help: "The measured pulse's velocity and the reference's.", group: 'time', families: ['measured', 'reference'] }],
		['displacement', { words: 'Displacement', help: "The measured pulse's displacement and the reference's.", group: 'time', families: ['measured', 'reference'] }],
		['srs', { words: 'SRS', help: "The measured pulse's shock response spectrum against the reference's, with the informational band.", group: 'frequency', families: ['measured', 'reference', 'tolerance'] }],
		['spectrum', { words: 'Spectra', help: "The reference's and the measured pulse's spectra in the correction band.", group: 'frequency', families: ['measured', 'reference'] }],
		['identification', { words: 'Identification', help: "The identified system's magnitude and phase, and the coherence with the trusted lines marked.", group: 'frequency', families: ['measured', 'reference', 'tolerance'] }],
		['law', { words: 'Law Values', help: "The control law's reported values on its last call.", group: 'numbers' }],
		['readings', { words: 'Readings', help: 'The run in numbers.', group: 'numbers' }]
	],
	cards: ['run', 'level', 'pulses', 'station'],
	pageCards: { pulses: ['pulses'] }
};

export const SHOCK_PLOT_TYPES: PlotType[] = pulsePlotTypes(SHOCK_SCREEN);
