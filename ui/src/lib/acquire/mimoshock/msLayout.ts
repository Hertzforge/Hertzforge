/**
 * The MIMO Shock run's screen (PROTOCOL.md, "MIMO Shock Display"): every quantity the service
 * draws offered as a plot type, and the page kinds a screen opens with, on the frame the pulse
 * page layer shares (`../pulse/pulseScreen`).
 *
 * The pages until the operator changes them: Pulses (each control's measured pulse over its
 * reference and tolerance band), SRS (each control's SRS over its target and tolerance), Drives,
 * Verdicts (the verdict grid and the latest pulse's judgement per control), Ladder (the levels,
 * the run's numbers), Identification (the identified gains and coherence, and each control's
 * reachable floor) and Law (the law's reported values).
 */

import type { MimoShockQuantity } from '$lib/protocol/mimoShock';
import type { PlotType } from '../plots/types';
import type { PulsePageSet } from '../pulse/pulseLayout';
import { pulsePlotTypes, type PulseScreenSpec } from '../pulse/pulseScreen';
import type { PlotPanel } from '../screen/screen.svelte';

export type MsPageKind = 'pulse' | 'srs' | 'drives' | 'verdicts' | 'ladder' | 'identification' | 'law';

export const MS_PAGE_KINDS: MsPageKind[] = ['pulse', 'srs', 'drives', 'verdicts', 'ladder', 'identification', 'law'];

export const MS_PAGE_WORDS: Record<MsPageKind, string> = {
	pulse: 'Pulses',
	srs: 'SRS',
	drives: 'Drives',
	verdicts: 'Verdicts',
	ladder: 'Ladder',
	identification: 'Identification',
	law: 'Law'
};

export const MS_PAGE_QUANTITIES: Record<MsPageKind, MimoShockQuantity[]> = {
	pulse: ['pulse'],
	srs: ['srs'],
	drives: ['drives'],
	verdicts: ['verdicts'],
	ladder: ['ladder', 'readings'],
	identification: ['identification', 'review'],
	law: ['law', 'readings']
};

export const MS_TIME_QUANTITIES: readonly MimoShockQuantity[] = ['pulse', 'drives'];
/** The quantities drawn per control: a plot may pick one control. */
export const MS_CHANNEL_QUANTITIES: readonly MimoShockQuantity[] = ['pulse', 'srs', 'identification'];

export const MS_PAGES: PulsePageSet<MsPageKind, MimoShockQuantity> = {
	testType: 'mimo_shock',
	kinds: MS_PAGE_KINDS,
	words: MS_PAGE_WORDS,
	quantities: MS_PAGE_QUANTITIES,
	timeQuantities: MS_TIME_QUANTITIES,
	channelQuantities: MS_CHANNEL_QUANTITIES,
	rates: [1, 2, 5],
	defaultRate: 2,
	maxPages: 12
};

/** MIMO Shock's screen: its plot types, its cards, and the verdict grid on the Verdicts page. */
export const MS_SCREEN: PulseScreenSpec<MsPageKind, MimoShockQuantity> = {
	pages: MS_PAGES,
	plots: [
		['pulse', { words: 'Pulse', help: "Each control's measured pulse over its reference at the level fired, moved by its delay, with its tolerance band.", group: 'time', families: ['measured', 'reference', 'tolerance'] }],
		['srs', { words: 'SRS', help: "Each control's measured SRS over its target at the level fired, with its tolerance band.", group: 'frequency', families: ['measured', 'reference', 'tolerance'] }],
		['drives', { words: 'Drives', help: 'Every drive as played, in volts.', group: 'time', families: ['drive'] }],
		['verdicts', { words: 'Verdicts', help: "Each control's judgement of the latest pulse: why it failed, its error and its delay.", group: 'numbers' }],
		['ladder', { words: 'Ladder', help: 'The levels, the pulses fired and passed at each, and the level in force.', group: 'numbers' }],
		['identification', { words: 'Identification', help: 'The identified gain from every drive to a control, and every control\'s coherence with the floor.', group: 'frequency', families: ['measured', 'tolerance'] }],
		['review', { words: 'Reachable Floors', help: "Each control's reachable share and floor before the first pulse, and the first drives' predicted peak.", group: 'numbers' }],
		['law', { words: 'Law Values', help: "The control law's named values on its last call.", group: 'numbers' }],
		['readings', { words: 'Readings', help: 'The run in numbers.', group: 'numbers' }],
		['pulses', { words: 'Pulse Rows', help: 'Every pulse fired, in a table.', group: 'numbers' }]
	],
	cards: ['run', 'level', 'verdicts', 'pulse', 'station'],
	pageCards: { verdicts: ['verdicts'] },
	selectors: ['all']
};

export const MS_PLOT_TYPES: PlotType[] = pulsePlotTypes(MS_SCREEN);

/**
 * What a window sends for the plots it shows, or the default subscription when it shows none. A
 * per-control plot that picked controls asks for the first one it picked (the service draws one
 * control or every control), and a time history its point count.
 */
export function msSubscription(plots: PlotPanel[], rate: number): Record<string, unknown> {
	if (!plots.length) return { default: true };
	return {
		rate_hz: rate,
		plots: plots.map((p) => {
			const out: Record<string, unknown> = { id: p.id, quantity: p.plot.quantity };
			const c = p.plot.channels;
			if ((MS_CHANNEL_QUANTITIES as readonly string[]).includes(p.plot.quantity) && Array.isArray(c) && c.length) out.channel = c[0];
			if ((MS_TIME_QUANTITIES as readonly string[]).includes(p.plot.quantity)) out.points = p.plot.points;
			return out;
		})
	};
}
