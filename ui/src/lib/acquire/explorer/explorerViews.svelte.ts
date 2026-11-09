/**
 * One window's Explorer run views: the layout's pages, the page this window shows, and the
 * subscription that keeps the window's plots equal to what that page draws (PROTOCOL.md, "Explorer
 * Display"). The page layer the shock types, Sine and TWR share (`../pulse/pulseViews`) does the
 * work; this names Explorer's pages and topics. The service sends a subscribed window its data
 * frame at its rate while the run is live, once more at the end, and at once on subscribe.
 */

import type { ServiceClient } from '$lib/protocol/client.svelte';
import { EXPLORER_TOPICS, type ExplorerData, type ExplorerLiveQuantity } from '$lib/protocol/explorer';
import { PulseViewsState, type PulseViewsOptions } from '../pulse/pulseViews.svelte';
import { EXPLORER_PAGES, type ExplorerPageKind } from './explorerLayout';

export const EXPLORER_TEST_TYPE = 'explorer';

export class ExplorerViewsState extends PulseViewsState<ExplorerPageKind, ExplorerLiveQuantity, ExplorerData> {
	constructor(control: ServiceClient, opts: PulseViewsOptions) {
		super(control, { pages: EXPLORER_PAGES, dataTopic: EXPLORER_TOPICS.data, subscribe: 'explorer.subscribe', pageKey: 'hertzforge.display.explorer.page' }, opts);
	}
}
