/**
 * The SRS shock run's pages and what each asks the service for (PROTOCOL.md, "SRS Shock Display"),
 * on the pulse-by-pulse pages both shock types share (`../pulse/pulseLayout`).
 *
 * The pages until the operator changes them: SRS (the measured SRS against the target at the level
 * fired, with its alarm and abort bands), Pulse (the measured pulse and the drive played),
 * Kinematics (velocity and displacement), Identification (the identified gain with its floor and
 * the coherence with its hold), Limit Channels (each limit channel's predicted SRS against its
 * bound, and its gain), Components (the table in force and the law's last gains), Pulses (the
 * pulse table and the run's numbers) and Law (the law's reported values).
 */

import type { SrsQuantity } from '$lib/protocol/srs';
import {
	defaultPulseLayout,
	pulseLayoutFromSaved,
	pulseLayoutToSaved,
	pulsePagePlots,
	pulseSubscribeParams,
	type PulseLayout,
	type PulsePageSet,
	type PulsePageSpec,
	type PulsePlotSpec
} from '../pulse/pulseLayout';

export { MAX_OVERLAY, overlaidViews, pulsePlotId as srsPlotId, toggleOverlay, withOverlay } from '../pulse/pulseLayout';

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

export type SrsPageSpec = PulsePageSpec<SrsPageKind>;
export type SrsLayout = PulseLayout<SrsPageKind>;
export type SrsPlotSpec = PulsePlotSpec<SrsQuantity>;

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

export function defaultSrsLayout(): SrsLayout {
	return defaultPulseLayout(SRS_PAGES);
}

/** Everything a page subscribes to, at its drawn width (CSS pixels). */
export function srsPagePlots(page: SrsPageSpec, width = 900, pixelRatio = 1): SrsPlotSpec[] {
	return pulsePagePlots(SRS_PAGES, page, width, pixelRatio);
}

/** What a window sends: a point count with the time histories only, as the service requires. */
export function srsSubscribeParams(plots: SrsPlotSpec[], rate: number): Record<string, unknown> {
	return pulseSubscribeParams(SRS_PAGES, plots, rate);
}

export function srsLayoutFromSaved(text: string | null): SrsLayout | null {
	return pulseLayoutFromSaved(SRS_PAGES, text);
}

export function srsLayoutToSaved(layout: SrsLayout): string {
	return pulseLayoutToSaved(layout);
}
