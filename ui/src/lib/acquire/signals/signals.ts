/**
 * Signals on a test screen: the plot type that draws any input or drive channel against time, for
 * every test type, and the feed its plots are asked through (`signals.subscribe`; PROTOCOL.md,
 * "Signals"). A test screen offers it by adding `SIGNALS_TYPE` to its plot types and `SIGNALS_FEED`
 * to its other feeds (`signalsHooks`); the screen sends its Signals plots there and the rest to the
 * test's own feed.
 */

import {
	SIGNALS_MAX_POINTS,
	SIGNALS_MAX_RATE_HZ,
	SIGNALS_MAX_WINDOW_S,
	SIGNALS_METHODS,
	SIGNALS_MIN_POINTS,
	SIGNALS_MIN_RATE_HZ,
	SIGNALS_TOPICS,
	type SignalChannel,
	type SignalChannelSelector,
	type SignalsSubscribeParams,
	type SignalsSubscribePlot
} from '$lib/protocol/signals';
import type { PlotType } from '../plots/types';
import type { PlotPanel, ScreenFeed, ScreenHooks } from '../screen/screen.svelte';

/** The plot type's id, which is also its quantity on the screen. */
export const SIGNALS = 'signals';

export const SIGNALS_TYPE: PlotType = {
	id: SIGNALS,
	words: 'Signals',
	help: 'Any input or drive channel against time.',
	group: 'time',
	channels: true,
	time: true,
	families: ['measured', 'drive']
};

export function isSignals(quantity: string): boolean {
	return quantity === SIGNALS;
}

const clamp = (x: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, x));

/**
 * The service's selector for a plot's channels: its rows, or every channel, the control rows, the
 * inputs or the drives. A choice Signals has no rows for (the rows a test limits) shows every channel.
 */
export function signalsChannels(channels: unknown): SignalChannelSelector {
	if (Array.isArray(channels)) return channels.filter((c): c is number => Number.isInteger(c) && c >= 0);
	if (channels === 'control' || channels === 'inputs' || channels === 'drives') return channels;
	return 'all';
}

/** What a window asks for its Signals plots: each plot's channels, window, points and arrangement. */
export function signalsSubscription(plots: PlotPanel[]): SignalsSubscribePlot[] {
	return plots.map((p) => {
		const out: SignalsSubscribePlot = {
			id: p.id,
			channels: signalsChannels(p.plot.channels),
			points: clamp(Math.round(p.plot.points), SIGNALS_MIN_POINTS, SIGNALS_MAX_POINTS),
			arrange: p.plot.arrange
		};
		const w = p.plot.display.window_s;
		if (typeof w === 'number' && Number.isFinite(w) && w > 0) out.window_s = Math.min(w, SIGNALS_MAX_WINDOW_S);
		return out;
	});
}

export function signalsParams(plots: PlotPanel[], rate: number): SignalsSubscribeParams {
	return { rate_hz: clamp(rate, SIGNALS_MIN_RATE_HZ, SIGNALS_MAX_RATE_HZ), plots: signalsSubscription(plots) };
}

/** The feed a screen's Signals plots are asked through. */
export const SIGNALS_FEED: ScreenFeed = {
	method: SIGNALS_METHODS.subscribe,
	topic: SIGNALS_TOPICS.data,
	quantities: [SIGNALS],
	params: (plots, rate) => signalsParams(plots, rate) as unknown as Record<string, unknown>
};

/** A test screen's hooks with Signals offered: the plot type last in `+ Plot`, and its feed. */
export function signalsHooks(hooks: ScreenHooks): ScreenHooks {
	return {
		...hooks,
		types: hooks.types.some((t) => isSignals(t.id)) ? hooks.types : [...hooks.types, SIGNALS_TYPE],
		others: [...(hooks.others ?? []).filter((f) => f !== SIGNALS_FEED), SIGNALS_FEED]
	};
}

/** What an empty Signals plot says, from its last frame (null before any). */
export function signalsEmpty(frame: { channels?: number[] | null } | null): string {
	if (!frame) return 'Appears while a run acquires.';
	if (frame.channels && frame.channels.length === 0) return 'None of the chosen channels is in the run.';
	return 'Appears with the first samples of the run.';
}

/**
 * The channels a Signals plot offers to choose from: the run's when one acquires, else the enabled
 * rows of the channel table, inputs and drives, in table order.
 */
export function signalsChoices(run: SignalChannel[] | null | undefined, rows: { index: number; enabled: boolean; description: string; role: string }[]): { index: number; name: string }[] {
	if (run && run.length > 0) return run.map((c) => ({ index: c.index, name: c.name }));
	return rows.filter((r) => r.enabled).map((r) => ({ index: r.index, name: r.description || r.role }));
}
