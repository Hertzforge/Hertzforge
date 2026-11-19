/**
 * One window's Recorder run screen: its pages of plots and cards, the page this window shows, and
 * the data the control service sends for it through `recorder.subscribe` (PROTOCOL.md, "Recorder
 * Display"), at the window's rate while the run acquires and at once on subscribe. Layouts the page
 * layer kept are taken over the first time each is read.
 */

import type { ServiceClient } from '$lib/protocol/client.svelte';
import { RECORDER_TOPICS, type RecorderData } from '$lib/protocol/recorder';
import { pulseHooks } from '../pulse/pulseScreen';
import { PulseScreenState } from '../pulse/pulseScreenState.svelte';
import type { ScreenHooks, ScreenOptions } from '../screen/screen.svelte';
import { RECORDER_SCREEN } from './recorderLayout';

export const RECORDER_TEST_TYPE = 'recorder';

export const RECORDER_HOOKS: ScreenHooks = pulseHooks(RECORDER_SCREEN, { method: 'recorder.subscribe', topic: RECORDER_TOPICS.data });

export class RecorderViewsState extends PulseScreenState<RecorderData> {
	constructor(control: ServiceClient, opts: ScreenOptions) {
		super(control, RECORDER_HOOKS, opts);
	}
}
