/**
 * The Playback run's screen (PROTOCOL.md, "Playback Display"): the inputs and the drives as time
 * histories, on the frame the pulse page layer shares (`../pulse/pulseScreen`). The control
 * service sends every input and every drive in each `playback.live` frame, so there is nothing to
 * subscribe to: each plot draws the channels it shows from the last frame. There are no spectra
 * and no analysis.
 *
 * The pages until the operator changes them: Time (the inputs and the drives, one plot each).
 */

import type { PlotType } from '../plots/types';
import type { PulsePageSet } from '../pulse/pulseLayout';
import { pulsePlotTypes, type PulseScreenSpec } from '../pulse/pulseScreen';

export type PlaybackQuantity = 'inputs' | 'drives';
export type PlaybackPageKind = 'time';

export const PLAYBACK_QUANTITIES: readonly PlaybackQuantity[] = ['inputs', 'drives'];

export const PLAYBACK_PAGES: PulsePageSet<PlaybackPageKind, PlaybackQuantity> = {
	testType: 'playback',
	kinds: ['time'],
	words: { time: 'Time' },
	quantities: { time: ['inputs', 'drives'] },
	timeQuantities: ['inputs', 'drives'],
	rates: [10],
	defaultRate: 10,
	maxPages: 12,
	defaults: ['time'],
	channelQuantities: ['inputs', 'drives'],
	channels: {}
};

/** Playback's screen: its plot types, cards and first page. */
export const PLAYBACK_SCREEN: PulseScreenSpec<PlaybackPageKind, PlaybackQuantity> = {
	pages: PLAYBACK_PAGES,
	plots: [
		['inputs', { words: 'Inputs', help: 'Every enabled input row, the last half second or so, on the station clock.', group: 'time', families: ['measured'] }],
		['drives', { words: 'Drives', help: 'Every drive as played, the level and any stop ramp in it, on the station clock.', group: 'time', families: ['drive'] }]
	],
	cards: ['run', 'play', 'checked', 'station'],
	selectors: ['all']
};

export const PLAYBACK_PLOT_TYPES: PlotType[] = pulsePlotTypes(PLAYBACK_SCREEN);
