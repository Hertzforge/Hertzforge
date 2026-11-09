/**
 * The time waveform replication (TWR) run's pages and what each asks the service for
 * (PROTOCOL.md, "TWR Display"), on the page layer the shock types and Sine share
 * (`../pulse/pulseLayout`). The service draws every plot: the records of the last play against
 * time over the record, the identification and the trust against frequency, each play's error
 * against the play index.
 *
 * The pages until the operator changes them: Replication (the last play's response over the level
 * target), Error (the response less the target), Spectra (the target, the response and the drive in
 * the correction band), Identification (the identified gain and phase, the coherence, and the bins
 * the correction trusts), Convergence (each play's error against the gate), Law (the law's values
 * on its calls and the run in numbers) and Plays (every play, with the law's values). Target, Drive,
 * Envelope and Scope pages can be added.
 */

import type { TwrQuantity } from '$lib/protocol/twr';
import { TWR_POINTS_QUANTITIES } from '$lib/protocol/twr';
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

export type TwrPageSpec = PulsePageSpec<TwrPageKind>;
export type TwrLayout = PulseLayout<TwrPageKind>;
export type TwrPlotSpec = PulsePlotSpec<TwrQuantity>;

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

export const twrPlotId = pulsePlotId;

export function defaultTwrLayout(): TwrLayout {
	return defaultPulseLayout(TWR_PAGES);
}

/** Everything a page subscribes to, at its drawn width (CSS pixels). */
export function twrPagePlots(page: TwrPageSpec, width = 900, pixelRatio = 1): TwrPlotSpec[] {
	return pulsePagePlots(TWR_PAGES, page, width, pixelRatio);
}

/** What a window sends: its plots, a point count on the record plots only; no channels, the run has one control channel. */
export function twrSubscribeParams(plots: TwrPlotSpec[], rate: number): Record<string, unknown> {
	return pulseSubscribeParams(TWR_PAGES, plots, rate);
}

export function twrLayoutFromSaved(text: string | null): TwrLayout | null {
	return pulseLayoutFromSaved(TWR_PAGES, text);
}

export function twrLayoutToSaved(layout: TwrLayout): string {
	return pulseLayoutToSaved(layout);
}
