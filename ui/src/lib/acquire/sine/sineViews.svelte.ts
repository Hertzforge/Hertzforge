/**
 * One window's Sine run screen: its pages of plots and cards, the page this window shows, and the
 * data the control service sends for it through `sine.subscribe` (PROTOCOL.md, "Sine Display").
 * The service sends a window's data frame at its rate while the run is live, and once at once on
 * subscribe. Layouts the page layer kept are taken over the first time each is read.
 */

import type { ServiceClient } from '$lib/protocol/client.svelte';
import { SINE_TOPICS, type SineData, type SineSubscribeResult } from '$lib/protocol/sine';
import { pulseHooks } from '../pulse/pulseScreen';
import { PulseScreenState } from '../pulse/pulseScreenState.svelte';
import type { ScreenHooks, ScreenOptions } from '../screen/screen.svelte';
import { SINE_SCREEN } from './sineLayout';

export const SINE_TEST_TYPE = 'sine';

export const SINE_HOOKS: ScreenHooks = pulseHooks(SINE_SCREEN, { method: 'sine.subscribe', topic: SINE_TOPICS.data });

export class SineViewsState extends PulseScreenState<SineData, SineSubscribeResult> {
	constructor(control: ServiceClient, opts: ScreenOptions) {
		super(control, SINE_HOOKS, opts);
	}
}
