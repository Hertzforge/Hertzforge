/**
 * One window's TWR run screen: its pages of plots and cards, the page this window shows, and the
 * data the control service sends for it through `twr.subscribe` (PROTOCOL.md, "TWR Display"): at
 * its rate while a play is in flight, once per judged play, and at once on subscribe. Layouts the
 * page layer kept are taken over the first time each is read.
 */

import type { ServiceClient } from '$lib/protocol/client.svelte';
import { TWR_TOPICS, type TwrData, type TwrSubscribeResult } from '$lib/protocol/twr';
import { pulseHooks } from '../pulse/pulseScreen';
import { PulseScreenState } from '../pulse/pulseScreenState.svelte';
import type { ScreenHooks, ScreenOptions } from '../screen/screen.svelte';
import { TWR_SCREEN } from './twrLayout';

export const TWR_TEST_TYPE = 'twr';

export const TWR_HOOKS: ScreenHooks = pulseHooks(TWR_SCREEN, { method: 'twr.subscribe', topic: TWR_TOPICS.data });

export class TwrViewsState extends PulseScreenState<TwrData, TwrSubscribeResult> {
	constructor(control: ServiceClient, opts: ScreenOptions) {
		super(control, TWR_HOOKS, opts);
	}
}
