/**
 * The Random test's screen in one window: its pages of plots and cards (a `ScreenState` with
 * Random's plot types, cards and first pages), the page this window shows, and the data the
 * control service sends for it through `random.subscribe`.
 *
 * Every window subscribes for exactly the plots it has on screen (PROTOCOL.md, "Live Data
 * Subscriptions"): the page in view while the run view is up, and nothing but the run's numbers
 * otherwise. A window on another monitor is the same thing with its own page. Random's layouts
 * kept before the page grid are taken over the first time each is read.
 */

import type { ServiceClient } from '$lib/protocol/client.svelte';
import type { DataPlot, LiveQuantity, RandomData, RandomSubscribeResult, SubscribePlot } from '$lib/protocol/types';
import { CONTROL_TOPICS as T } from '$lib/protocol/types';
import { defaultRandomScreen, RANDOM_CARDS, RANDOM_LEGACY, RANDOM_TYPES } from '../screen/layout';
import { ScreenState, type PlotPanel, type ScreenHooks, type ScreenOptions } from '../screen/screen.svelte';
import { signalsHooks } from '../signals/signals';

export type DisplayOptions = ScreenOptions;

/** The subscription a window sends for the plots it shows. */
export function subscriptionFor(plots: PlotPanel[]): SubscribePlot[] {
	return plots.map((p) => {
		const type = RANDOM_TYPES.find((t) => t.id === p.plot.quantity);
		const out: SubscribePlot = { id: p.id, quantity: p.plot.quantity as LiveQuantity };
		if (type?.channels) out.channels = Array.isArray(p.plot.channels) ? [...p.plot.channels] : p.plot.channels;
		if (type?.time) out.points = p.plot.points;
		return out;
	});
}

/** Random's screen: its own plot types and Signals, any input or drive against time. */
export const RANDOM_SCREEN: ScreenHooks = signalsHooks({
	testType: 'random',
	types: RANDOM_TYPES,
	cards: RANDOM_CARDS,
	defaultLayout: defaultRandomScreen,
	legacy: RANDOM_LEGACY,
	feed: { method: 'random.subscribe', topic: T.data, params: (plots, rate) => ({ rate_hz: rate, plots: subscriptionFor(plots) }) }
});

export class DisplayState extends ScreenState<RandomData, RandomSubscribeResult> {
	constructor(control: ServiceClient, opts: DisplayOptions) {
		super(control, RANDOM_SCREEN, opts);
	}

	/** The quantities the service offers (all of them until it has said). */
	get quantities(): LiveQuantity[] {
		return this.accepted?.quantities ?? ['control', 'drive', 'response', 'time', 'coherence', 'readings'];
	}

	/** This plot's data in the last frame, or null. */
	plotData(id: string): DataPlot | null {
		return this.data?.plots.find((p) => p.id === id) ?? null;
	}
}
