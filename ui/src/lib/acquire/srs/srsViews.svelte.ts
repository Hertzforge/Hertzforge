/**
 * One window's SRS shock run views: the layout's pages, the page this window shows, and the
 * subscription that keeps the window's plots equal to what that page draws (PROTOCOL.md, "SRS
 * Shock Display"). The pulse-by-pulse views both shock types share (`../pulse/pulseViews`) do the
 * work; this names SRS shock's pages and topics.
 */

import type { ServiceClient } from '$lib/protocol/client.svelte';
import { SRS_TOPICS, type SrsData, type SrsQuantity, type SrsSubscribeResult } from '$lib/protocol/srs';
import { PulseViewsState, type PulseViewsOptions } from '../pulse/pulseViews.svelte';
import { SRS_PAGES, type SrsPageKind } from './srsLayout';

export const SRS_TEST_TYPE = 'srs';

export class SrsViewsState extends PulseViewsState<SrsPageKind, SrsQuantity, SrsData, SrsSubscribeResult> {
	constructor(control: ServiceClient, opts: PulseViewsOptions) {
		super(control, { pages: SRS_PAGES, dataTopic: SRS_TOPICS.data, subscribe: 'srs.subscribe', pageKey: 'hertzforge.display.srs.page' }, opts);
	}
}
