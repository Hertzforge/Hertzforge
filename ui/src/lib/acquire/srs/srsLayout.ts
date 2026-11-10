/**
 * The SRS shock run's screen (PROTOCOL.md, "SRS Shock Display"): every quantity the service draws
 * offered as a plot type, the page kinds a screen opens with, and the pulses picked in the pulse
 * table to lay over the SRS and Pulse plots, on the frame the pulse page layer shares
 * (`../pulse/pulseScreen`).
 *
 * The pages until the operator changes them: SRS (the measured SRS against the target at the level
 * fired, with its alarm and abort bands), Pulse (the measured pulse and the drive played),
 * Kinematics (velocity and displacement), Identification (the identified gain with its floor and
 * the coherence with its hold), Limit Channels (each limit channel's predicted SRS against its
 * bound, and its gain), Components (the table in force and the law's last gains), Pulses (the
 * pulse table and the run's numbers) and Law (the law's reported values).
 */

import type { SrsQuantity } from '$lib/protocol/srs';
import type { PlotType } from '../plots/types';
import type { PulsePageSet } from '../pulse/pulseLayout';
import { pulsePlotTypes, type PulseScreenSpec } from '../pulse/pulseScreen';

export { MAX_OVERLAY, overlaidViews, toggleOverlay, withOverlay } from '../pulse/pulseLayout';

export type SrsPageKind = 'srs' | 'pulse' | 'kinematics' | 'identification' | 'limit_channels' | 'components' | 'pulses' | 'law';

export const SRS_PAGE_KINDS: SrsPageKind[] = ['srs', 'pulse', 'kinematics', 'identification', 'limit_channels', 'components', 'pulses', 'law'];

export const SRS_PAGE_WORDS: Record<SrsPageKind, string> = {
	srs: 'SRS',
	pulse: 'Pulse',
	kinematics: 'Kinematics',
	identification: 'Identification',
	limit_channels: 'Limit Channels',
	components: 'Components',
	pulses: 'Pulses',
	law: 'Law'
};

export const SRS_PAGE_HELP: Record<SrsPageKind, string> = {
	srs: "The measured pulse's shock response spectrum against the target at the level fired, with its alarm and abort bands.",
	pulse: 'The measured pulse, and the drive as played.',
	kinematics: 'Velocity and displacement of the measured pulse.',
	identification: "The identified gain to the control channel with its floor, and the coherence with the hold it must reach.",
	limit_channels: "Each limit channel's predicted SRS for the last drive against its bound, and the gain to it, measured or supplied.",
	components: "Each component's drive amplitude in the table in force, and the gains the control law last returned.",
	pulses: 'Every pulse fired, with its worst band and verdict, and the run in numbers.',
	law: "The control law's reported values on its last call, and the run in numbers."
};

export const SRS_RATES = [1, 2, 5];
export const SRS_DEFAULT_RATE = 2;
export const MAX_SRS_PAGES = 12;

/** The quantities each page draws, in order. */
export const SRS_PAGE_QUANTITIES: Record<SrsPageKind, SrsQuantity[]> = {
	srs: ['srs'],
	pulse: ['pulse'],
	kinematics: ['velocity', 'displacement'],
	identification: ['identification'],
	limit_channels: ['limit_channels', 'limit_frf'],
	components: ['components'],
	pulses: ['readings'],
	law: ['law', 'readings']
};

/** The quantities drawn as time histories: the only plots that take a point count. */
export const SRS_TIME_QUANTITIES: readonly SrsQuantity[] = ['pulse', 'velocity', 'displacement'];

/** Every quantity the service offers, in the order its result lists them. */
export const SRS_QUANTITIES: readonly SrsQuantity[] = ['srs', 'pulse', 'velocity', 'displacement', 'identification', 'components', 'limit_channels', 'limit_frf', 'law', 'readings', 'pulses'];

export const SRS_PAGES: PulsePageSet<SrsPageKind, SrsQuantity> = {
	testType: 'srs',
	prefix: 'rp',
	kinds: SRS_PAGE_KINDS,
	words: SRS_PAGE_WORDS,
	help: SRS_PAGE_HELP,
	quantities: SRS_PAGE_QUANTITIES,
	timeQuantities: SRS_TIME_QUANTITIES,
	rates: SRS_RATES,
	defaultRate: SRS_DEFAULT_RATE,
	maxPages: MAX_SRS_PAGES
};

/** SRS shock's screen: its plot types, cards and first pages; the pulse table on the Pulses page. */
export const SRS_SCREEN: PulseScreenSpec<SrsPageKind, SrsQuantity> = {
	pages: SRS_PAGES,
	plots: [
		['srs', { words: 'SRS', help: "The measured pulse's shock response spectrum against the target at the level fired, with its alarm and abort bands.", group: 'frequency', families: ['measured', 'reference', 'tolerance', 'abort'] }],
		['pulse', { words: 'Pulse', help: 'The measured pulse, and the drive as played.', group: 'time', families: ['measured', 'drive'] }],
		['velocity', { words: 'Velocity', help: "The measured pulse's velocity.", group: 'time', families: ['measured'] }],
		['displacement', { words: 'Displacement', help: "The measured pulse's displacement.", group: 'time', families: ['measured'] }],
		['identification', { words: 'Identification', help: 'The identified gain to the control channel with its floor, and the coherence with the hold it must reach.', group: 'frequency', families: ['measured', 'tolerance'] }],
		['components', { words: 'Components', help: "Each component's drive amplitude in the table in force, and the gains the control law last returned.", group: 'frequency', families: ['measured', 'drive'] }],
		['limit_channels', { words: 'Limit Channels', help: "Each limit channel's predicted SRS for the last drive against its bound.", group: 'frequency', families: ['measured', 'limit'] }],
		['limit_frf', { words: 'Limit Gains', help: 'The gain from the drive to each limit channel, measured or supplied.', group: 'frequency', families: ['measured'] }],
		['law', { words: 'Law Values', help: "The control law's reported values on its last call.", group: 'numbers' }],
		['readings', { words: 'Readings', help: 'The run in numbers.', group: 'numbers' }]
	],
	cards: ['run', 'level', 'pulses', 'station'],
	pageCards: { pulses: ['pulses'] }
};

export const SRS_PLOT_TYPES: PlotType[] = pulsePlotTypes(SRS_SCREEN);
