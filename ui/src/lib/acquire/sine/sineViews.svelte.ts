/**
 * One window's Sine run views: the layout's pages, the page this window shows, and the
 * subscription that keeps the window's plots equal to what that page draws (PROTOCOL.md, "Sine
 * Display"). The page layer the shock types share (`../pulse/pulseViews`) does the work; this names
 * Sine's pages and topics. The service sends a window's data frame at its rate while the run is
 * live, and once at once on subscribe.
 */

import type { ServiceClient } from '$lib/protocol/client.svelte';
import { SINE_TOPICS, type SineData, type SineLiveQuantity, type SineSubscribeResult } from '$lib/protocol/sine';
import { PulseViewsState, type PulseViewsOptions } from '../pulse/pulseViews.svelte';
import { SINE_PAGES, type SinePageKind } from './sineLayout';

export const SINE_TEST_TYPE = 'sine';

export class SineViewsState extends PulseViewsState<SinePageKind, SineLiveQuantity, SineData, SineSubscribeResult> {
	constructor(control: ServiceClient, opts: PulseViewsOptions) {
		super(control, { pages: SINE_PAGES, dataTopic: SINE_TOPICS.data, subscribe: 'sine.subscribe', pageKey: 'hertzforge.display.sine.page' }, opts);
	}
}
