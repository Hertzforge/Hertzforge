/**
 * One window's MIMO Shock run screen: its pages of plots and cards, the page this window shows,
 * and the data the control service sends for it through `mimo_shock.subscribe` (PROTOCOL.md, "MIMO
 * Shock Display"), once per pulse, once when identification ends, and at once on subscribe.
 * Before this window's first frame, the last pulse event's views stand in on the Pulse, SRS and
 * Drives plots.
 */

import type { ServiceClient } from '$lib/protocol/client.svelte';
import { MIMO_SHOCK_TOPICS, type MimoShockData, type MimoShockSubscribeResult } from '$lib/protocol/mimoShock';
import type { SpectrumView, View } from '$lib/protocol/types';
import { pulseHooks } from '../pulse/pulseScreen';
import { PulseScreenState } from '../pulse/pulseScreenState.svelte';
import type { ScreenHooks, ScreenOptions } from '../screen/screen.svelte';
import { MS_SCREEN, msSubscription } from './msLayout';

export const MS_TEST_TYPE = 'mimo_shock';

const base = pulseHooks(MS_SCREEN, { method: 'mimo_shock.subscribe', topic: MIMO_SHOCK_TOPICS.data });

export const MS_HOOKS: ScreenHooks = {
	...base,
	feed: { method: 'mimo_shock.subscribe', topic: MIMO_SHOCK_TOPICS.data, params: (plots, rate) => msSubscription(plots, rate) }
};

/** The last pulse's views for a quantity, before a window's own frame has come. */
export function standIn(quantity: string, lastViews: readonly View[]): View[] {
	const spectra = lastViews.filter((v): v is SpectrumView => v.view === 'spectrum');
	if (quantity === 'pulse') return spectra.filter((v) => /^pulse\b/i.test(v.title));
	if (quantity === 'srs') return spectra.filter((v) => /^srs\b/i.test(v.title));
	if (quantity === 'drives') return spectra.filter((v) => /^drives\b/i.test(v.title));
	return [];
}

export class MsViewsState extends PulseScreenState<MimoShockData, MimoShockSubscribeResult> {
	constructor(control: ServiceClient, opts: ScreenOptions, store: { lastViews: View[] } | null = null) {
		super(control, MS_HOOKS, opts, store ? (q, views) => (views.length ? views : standIn(q, store.lastViews)) : null);
	}
}
