/**
 * The classical shock run's pages and what each asks the service for (PROTOCOL.md, "Classical
 * Shock Display"), on the pulse-by-pulse pages both shock types share (`../pulse/pulseLayout`):
 * the plain-data form a layout is saved in, and the pulses an operator picks in the pulse table to
 * lay over the Pulse and SRS pages.
 *
 * The pages until the operator changes them: Pulse (the reference at the level fired with its
 * tolerance band, the measured pulse and the drive played), SRS, Kinematics (velocity and
 * displacement), Identification (|H|, phase and coherence, and the pulse's spectra), Pulses (the
 * pulse table and the run's numbers) and Law (the law's reported values).
 */

import type { ShockQuantity } from '$lib/protocol/shock';
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

export type ShockPageSpec = PulsePageSpec<ShockPageKind>;
export type ShockLayout = PulseLayout<ShockPageKind>;
/** One plot a page subscribes to. */
export type ShockPlotSpec = PulsePlotSpec<ShockQuantity>;

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

export function defaultShockLayout(): ShockLayout {
	return defaultPulseLayout(SHOCK_PAGES);
}

/** The plot id a page uses for one of its parts. */
export const shockPlotId = (page: ShockPageSpec, part: string) => pulsePlotId(page, part);

/** Everything a page subscribes to, at its drawn width (CSS pixels). */
export function shockPagePlots(page: ShockPageSpec, width = 900, pixelRatio = 1): ShockPlotSpec[] {
	return pulsePagePlots(SHOCK_PAGES, page, width, pixelRatio);
}

/** What a window sends: its plots, or the default subscription when it shows none. A point count
 * goes with the time histories only; the service refuses it on any other plot. */
export function shockSubscribeParams(plots: ShockPlotSpec[], rate: number): Record<string, unknown> {
	return pulseSubscribeParams(SHOCK_PAGES, plots, rate);
}

// ---------------------------------------------------------------------------------------------
// Saved form

export function shockLayoutFromSaved(text: string | null): ShockLayout | null {
	return pulseLayoutFromSaved(SHOCK_PAGES, text);
}

export function shockLayoutToSaved(layout: ShockLayout): string {
	return pulseLayoutToSaved(layout);
}
