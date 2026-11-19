/**
 * One window's classical shock run screen: its pages of plots and cards, the page this window
 * shows, and the data the control service sends for it through `shock.subscribe` (PROTOCOL.md,
 * "Classical Shock Display"), once per pulse and at once on subscribe. The Pulse and SRS plots
 * carry the pulses picked in the pulse table laid over the latest one; before this window's first
 * frame, the last pulse event's views stand in. Layouts the page layer kept are taken over the
 * first time each is read.
 */

import type { ServiceClient } from '$lib/protocol/client.svelte';
import { SHOCK_TOPICS, type ShockData, type ShockSubscribeResult } from '$lib/protocol/shock';
import { overlaidViews } from '../pulse/pulseLayout';
import { pulseHooks } from '../pulse/pulseScreen';
import { PulseScreenState } from '../pulse/pulseScreenState.svelte';
import type { ScreenHooks, ScreenOptions } from '../screen/screen.svelte';
import { SHOCK_SCREEN } from './shockLayout';
import type { ShockStore } from './shockStore.svelte';

export const SHOCK_TEST_TYPE = 'shock';

export type ShockViewsOptions = ScreenOptions;

export const SHOCK_HOOKS: ScreenHooks = pulseHooks(SHOCK_SCREEN, { method: 'shock.subscribe', topic: SHOCK_TOPICS.data });

export class ShockViewsState extends PulseScreenState<ShockData, ShockSubscribeResult> {
	/** `shock`, when given, lays the pulses picked in its pulse table over the Pulse and SRS plots. */
	constructor(control: ServiceClient, opts: ShockViewsOptions, shock: Pick<ShockStore, 'lastViews' | 'overlaid'> | null = null) {
		super(control, SHOCK_HOOKS, opts, shock ? (q, views) => (q === 'pulse' || q === 'srs' ? overlaidViews(views, shock.lastViews, shock.overlaid, q) : views) : null);
	}
}
