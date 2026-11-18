/**
 * One window's MIMO Sine run screen: its pages of plots and cards, the page this window shows, and
 * the data the control service sends for it through `mimo_sine.subscribe` (PROTOCOL.md, "MIMO Sine
 * Display"). The service sends a window's data frame at its rate while the run is live, and once
 * at once on subscribe.
 */

import type { ServiceClient } from '$lib/protocol/client.svelte';
import { MIMO_SINE_TOPICS, type MimoSineData, type MimoSineSubscribeResult } from '$lib/protocol/mimoSine';
import { pulseHooks } from '../pulse/pulseScreen';
import { PulseScreenState } from '../pulse/pulseScreenState.svelte';
import type { ScreenHooks, ScreenOptions } from '../screen/screen.svelte';
import { MSINE_SCREEN } from './msineLayout';

export const MSINE_TEST_TYPE = 'mimo_sine';

export const MSINE_HOOKS: ScreenHooks = pulseHooks(MSINE_SCREEN, { method: 'mimo_sine.subscribe', topic: MIMO_SINE_TOPICS.data });

export class MsineViewsState extends PulseScreenState<MimoSineData, MimoSineSubscribeResult> {
	constructor(control: ServiceClient, opts: ScreenOptions) {
		super(control, MSINE_HOOKS, opts);
	}
}
