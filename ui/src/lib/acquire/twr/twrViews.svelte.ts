/**
 * One window's time waveform replication (TWR) run views: the layout's pages, the page this
 * window shows, and the subscription that keeps the window's plots equal to what that page draws
 * (PROTOCOL.md, "TWR Display"). The page layer the shock types and Sine share
 * (`../pulse/pulseViews`) does the work; this names TWR's pages and topics. The service sends a
 * window's data frame at its rate while a plotted quantity changes, once when identification ends,
 * once after each play, and at once on subscribe.
 */

import type { ServiceClient } from '$lib/protocol/client.svelte';
import { TWR_TOPICS, type TwrData, type TwrQuantity, type TwrSubscribeResult } from '$lib/protocol/twr';
import { PulseViewsState, type PulseViewsOptions } from '../pulse/pulseViews.svelte';
import { TWR_PAGES, type TwrPageKind } from './twrLayout';

export const TWR_TEST_TYPE = 'twr';

export class TwrViewsState extends PulseViewsState<TwrPageKind, TwrQuantity, TwrData, TwrSubscribeResult> {
	constructor(control: ServiceClient, opts: PulseViewsOptions) {
		super(control, { pages: TWR_PAGES, dataTopic: TWR_TOPICS.data, subscribe: 'twr.subscribe', pageKey: 'hertzforge.display.twr.page' }, opts);
	}
}
