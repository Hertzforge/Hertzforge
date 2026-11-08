/**
 * One window's classical shock run views: the layout's pages, the page this window shows, and the
 * subscription that keeps the window's plots equal to what that page draws (PROTOCOL.md, "Classical
 * Shock Display"). The pulse-by-pulse views both shock types share (`../pulse/pulseViews`) do the
 * work; this names classical shock's pages and topics.
 */

import type { ServiceClient } from '$lib/protocol/client.svelte';
import { SHOCK_TOPICS, type ShockData, type ShockQuantity, type ShockSubscribeResult } from '$lib/protocol/shock';
import { PulseViewsState, type PulseViewsOptions } from '../pulse/pulseViews.svelte';
import { SHOCK_PAGES, type ShockPageKind } from './shockLayout';

export const SHOCK_TEST_TYPE = 'shock';

export type ShockViewsOptions = PulseViewsOptions;

export class ShockViewsState extends PulseViewsState<ShockPageKind, ShockQuantity, ShockData, ShockSubscribeResult> {
	constructor(control: ServiceClient, opts: ShockViewsOptions) {
		super(control, { pages: SHOCK_PAGES, dataTopic: SHOCK_TOPICS.data, subscribe: 'shock.subscribe', pageKey: 'hertzforge.display.shock.page' }, opts);
	}
}
