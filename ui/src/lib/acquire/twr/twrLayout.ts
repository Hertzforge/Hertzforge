/**
 * The time waveform replication (TWR) run's screen (PROTOCOL.md, "TWR Display"): every quantity
 * the service draws offered as a plot type, and the page kinds a screen opens with, on the frame
 * the pulse page layer shares (`../pulse/pulseScreen`). The service draws every plot: the records
 * of the last play against time over the record, the identification and the trust against
 * frequency, each play's error against the play index.
 *
 * The pages until the operator changes them: Replication (the last play's response over the level
 * target), Error (the response less the target), Spectra (the target, the response and the drive in
 * the correction band), Identification (the identified gain and phase, the coherence, and the bins
 * the correction trusts), Convergence (each play's error against the gate), Law (the law's values
 * on its calls and the run in numbers) and Plays (the play table and the run in numbers). A layout
 * kept with Target, Drive, Envelope or Scope pages is taken over as it was.
 */

import type { TwrQuantity } from '$lib/protocol/twr';
import { TWR_POINTS_QUANTITIES } from '$lib/protocol/twr';
import type { PlotType } from '../plots/types';
import type { PulsePageSet } from '../pulse/pulseLayout';
import { pulsePlotTypes, type PulseScreenSpec } from '../pulse/pulseScreen';

export type TwrPageKind = 'replication' | 'error' | 'spectra' | 'identification' | 'convergence' | 'law' | 'plays' | 'target' | 'drive' | 'envelope' | 'scope';

export const TWR_PAGE_KINDS: TwrPageKind[] = ['replication', 'error', 'spectra', 'identification', 'convergence', 'law', 'plays', 'target', 'drive', 'envelope', 'scope'];

/** The pages a new layout starts with. */
export const TWR_DEFAULT_PAGES: TwrPageKind[] = ['replication', 'error', 'spectra', 'identification', 'convergence', 'law', 'plays'];

export const TWR_PAGE_WORDS: Record<TwrPageKind, string> = {
	replication: 'Replication',
	error: 'Error',
	spectra: 'Spectra',
	identification: 'Identification',
	convergence: 'Convergence',
	law: 'Law',
	plays: 'Plays',
	target: 'Target',
	drive: 'Drive',
	envelope: 'Envelope',
	scope: 'Scope'
};

export const TWR_PAGE_HELP: Record<TwrPageKind, string> = {
	replication: "The last play's response over the target at its level, across the record.",
	error: "The last play's response less the target, across the record.",
	spectra: 'The target at the level, the last response and the drive it played, in the correction band.',
	identification: 'The identified gain and phase, the coherence against its floors, and the bins the correction trusts.',
	convergence: "Each play's rms error against the gate.",
	law: "The control law's values on each call, and the run in numbers.",
	plays: 'Every play, with its error, its gate, its drive and the law call that made it.',
	target: 'The target at the present level, across the record.',
	drive: 'The drive the last play played, in volts.',
	envelope: "The last play's response inside the alarm and abort bands around the target.",
	scope: 'The control channel during the play in flight.'
};

export const TWR_RATES = [1, 2, 5, 10];
export const TWR_DEFAULT_RATE = 5;
export const MAX_TWR_PAGES = 12;

/** The quantities each page draws, in order. */
export const TWR_PAGE_QUANTITIES: Record<TwrPageKind, TwrQuantity[]> = {
	replication: ['response'],
	error: ['error'],
	spectra: ['spectra'],
	identification: ['frf', 'coherence', 'mask'],
	convergence: ['convergence'],
	law: ['law', 'readings'],
	plays: ['readings'],
	target: ['target'],
	drive: ['drive'],
	envelope: ['envelope'],
	scope: ['scope']
};

/** Every quantity the service offers, in the order its result lists them. */
export const TWR_QUANTITIES: readonly TwrQuantity[] = ['target', 'response', 'error', 'drive', 'envelope', 'frf', 'coherence', 'mask', 'spectra', 'convergence', 'law', 'scope', 'readings', 'plays'];

export const TWR_PAGES: PulsePageSet<TwrPageKind, TwrQuantity> = {
	testType: 'twr',
	prefix: 'tp',
	kinds: TWR_PAGE_KINDS,
	words: TWR_PAGE_WORDS,
	help: TWR_PAGE_HELP,
	quantities: TWR_PAGE_QUANTITIES,
	timeQuantities: TWR_POINTS_QUANTITIES,
	rates: TWR_RATES,
	defaultRate: TWR_DEFAULT_RATE,
	maxPages: MAX_TWR_PAGES,
	defaults: TWR_DEFAULT_PAGES
};

/** TWR's screen: its plot types, cards and first pages; the play table on the Plays page. */
export const TWR_SCREEN: PulseScreenSpec<TwrPageKind, TwrQuantity> = {
	pages: TWR_PAGES,
	plots: [
		['response', { words: 'Response', help: "The last play's response over the target at its level, across the record.", group: 'time', families: ['measured', 'reference'] }],
		['error', { words: 'Error', help: "The last play's response less the target, across the record.", group: 'time', families: ['measured'] }],
		['target', { words: 'Target', help: 'The target at the present level, across the record.', group: 'time', families: ['reference'] }],
		['drive', { words: 'Drive', help: 'The drive the last play played, in volts.', group: 'time', families: ['drive'] }],
		['envelope', { words: 'Envelope', help: "The last play's response inside the alarm and abort bands around the target.", group: 'time', families: ['measured', 'reference', 'tolerance', 'abort'] }],
		['scope', { words: 'Scope', help: 'The control channel during the play in flight.', group: 'time', families: ['measured'] }],
		['spectra', { words: 'Spectra', help: 'The target at the level, the last response and the drive it played, in the correction band.', group: 'frequency', families: ['measured', 'reference', 'drive'] }],
		['frf', { words: 'Identification', help: 'The identified gain and phase.', group: 'frequency', families: ['measured'] }],
		['coherence', { words: 'Coherence', help: "The identification's coherence against its floors.", group: 'frequency', families: ['measured', 'tolerance', 'abort'] }],
		['mask', { words: 'Trust', help: 'The bins the correction trusts, with the reference spectrum.', group: 'frequency', families: ['measured', 'reference'] }],
		['convergence', { words: 'Convergence', help: "Each play's rms error against the gate.", group: 'other', families: ['measured', 'limit'] }],
		['law', { words: 'Law Values', help: "The control law's values on each call, and on its last.", group: 'other' }],
		['readings', { words: 'Readings', help: 'The run in numbers.', group: 'numbers' }]
	],
	cards: ['run', 'level', 'plays', 'station'],
	pageCards: { plays: ['plays'] }
};

export const TWR_PLOT_TYPES: PlotType[] = pulsePlotTypes(TWR_SCREEN);
