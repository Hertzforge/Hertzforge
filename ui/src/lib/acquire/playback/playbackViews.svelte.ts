/**
 * One window's Playback run screen: its pages of plots and cards, the page this window shows, and
 * the time histories each plot draws. The control service sends `playback.live` to every
 * connection ten times a second with the last display points of every input and every drive
 * (PROTOCOL.md, "Playback Display"), so a window asks for nothing: each plot draws the channels it
 * shows from the last frame of the run on screen, as the Recorder's Time History draws its rows.
 */

import type { ServiceClient } from '$lib/protocol/client.svelte';
import { PLAYBACK_TOPICS, type PlaybackLive } from '$lib/protocol/playback';
import type { Series, SpectrumView, View } from '$lib/protocol/types';
import { pulseHooks } from '../pulse/pulseScreen';
import { PulseScreenState } from '../pulse/pulseScreenState.svelte';
import type { ScreenHooks, ScreenOptions } from '../screen/screen.svelte';
import { PLAYBACK_SCREEN } from './playbackLayout';

export const PLAYBACK_TEST_TYPE = 'playback';

const BASE = pulseHooks(PLAYBACK_SCREEN, { method: 'playback.live', topic: PLAYBACK_TOPICS.live });
/** Playback's screen hooks: no feed of its own, since every frame reaches every window. */
export const PLAYBACK_HOOKS: ScreenHooks = { ...BASE, feed: undefined };

/**
 * A plot's time history from a live frame: every channel of its role the plot shows, each drawn
 * against station time, inputs in their unit and drives in volts.
 */
export function timeView(live: PlaybackLive | null, quantity: string, channels: unknown, names: (index: number) => string): View[] {
	if (!live) return [];
	const role = quantity === 'drives' ? 'drive' : 'input';
	const shown = live.time.channels.filter((c) => c.role === role && (!Array.isArray(channels) || channels.includes(c.index)));
	if (!shown.length || !shown[0].values.length) return [];
	const n = shown[0].values.length;
	const x = new Float64Array(n);
	for (let i = 0; i < n; i++) x[i] = live.time.t0_s + i / live.time.rate_hz;
	const series: Series[] = shown.map((c) => ({ name: names(c.index), role: role === 'drive' ? 'drive' : 'measured', x, y: c.values }));
	const units = [...new Set(shown.map((c) => c.unit))];
	const view: SpectrumView = {
		view: 'spectrum',
		title: role === 'drive' ? 'Drives' : 'Inputs',
		x: { label: 'Time', unit: 's', scale: 'linear' },
		y: { label: role === 'drive' ? 'Drive' : 'Amplitude', unit: units.length === 1 ? units[0] : '', scale: 'linear' },
		series
	};
	return [view];
}

export class PlaybackViewsState extends PulseScreenState {
	/** The last frame of the run on screen. */
	live = $state.raw<PlaybackLive | null>(null);
	/** A row's name for a plot's legend. */
	names: (index: number) => string = (i) => `Channel ${i}`;

	constructor(control: ServiceClient, opts: ScreenOptions & { runId?: () => string | null }) {
		super(control, PLAYBACK_HOOKS, opts);
		const runId = opts.runId ?? (() => null);
		control.on(PLAYBACK_TOPICS.live, (d) => {
			const l = d as unknown as PlaybackLive;
			const id = runId();
			if (id && l.run_id !== id) return;
			if (opts.showing()) this.live = l;
		});
	}

	override views(id: string): View[] {
		const p = this.plot(id);
		return p ? timeView(this.live, p.quantity, p.channels, this.names) : [];
	}
}
