/**
 * One window's Explorer run screen: its pages of plots and cards, the page this window shows, and
 * the data the control service sends for it through `explorer.subscribe` (PROTOCOL.md, "Explorer
 * Display"), at the window's rate while the run acquires and at once on subscribe. Layouts the page
 * layer kept are taken over the first time each is read.
 */

import type { ServiceClient } from '$lib/protocol/client.svelte';
import { EXPLORER_TOPICS, type ExplorerData } from '$lib/protocol/explorer';
import { pulseHooks } from '../pulse/pulseScreen';
import { PulseScreenState } from '../pulse/pulseScreenState.svelte';
import type { ScreenHooks, ScreenOptions } from '../screen/screen.svelte';
import { EXPLORER_SCREEN } from './explorerLayout';

export const EXPLORER_TEST_TYPE = 'explorer';

export const EXPLORER_HOOKS: ScreenHooks = pulseHooks(EXPLORER_SCREEN, { method: 'explorer.subscribe', topic: EXPLORER_TOPICS.data });

export class ExplorerViewsState extends PulseScreenState<ExplorerData> {
	constructor(control: ServiceClient, opts: ScreenOptions) {
		super(control, EXPLORER_HOOKS, opts);
	}
}
