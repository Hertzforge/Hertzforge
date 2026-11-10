/**
 * One window's SRS shock run screen: its pages of plots and cards, the page this window shows, and
 * the data the control service sends for it through `srs.subscribe` (PROTOCOL.md, "SRS Shock
 * Display"), once per pulse and at once on subscribe. The SRS and Pulse plots carry the pulses
 * picked in the pulse table laid over the latest one; before this window's first frame, the last
 * pulse event's views stand in. Layouts the page layer kept are taken over the first time each is
 * read.
 */

import type { ServiceClient } from '$lib/protocol/client.svelte';
import { SRS_TOPICS, type SrsData, type SrsSubscribeResult } from '$lib/protocol/srs';
import { overlaidViews } from '../pulse/pulseLayout';
import { pulseHooks } from '../pulse/pulseScreen';
import { PulseScreenState } from '../pulse/pulseScreenState.svelte';
import type { ScreenHooks, ScreenOptions } from '../screen/screen.svelte';
import { SRS_SCREEN } from './srsLayout';
import type { SrsStore } from './srsStore.svelte';

export const SRS_TEST_TYPE = 'srs';

export const SRS_HOOKS: ScreenHooks = pulseHooks(SRS_SCREEN, { method: 'srs.subscribe', topic: SRS_TOPICS.data });

export class SrsViewsState extends PulseScreenState<SrsData, SrsSubscribeResult> {
	/** `srs`, when given, lays the pulses picked in its pulse table over the SRS and Pulse plots. */
	constructor(control: ServiceClient, opts: ScreenOptions, srs: Pick<SrsStore, 'lastViews' | 'overlaid'> | null = null) {
		super(control, SRS_HOOKS, opts, srs ? (q, views) => (q === 'pulse' || q === 'srs' ? overlaidViews(views, srs.lastViews, srs.overlaid, q) : views) : null);
	}
}
