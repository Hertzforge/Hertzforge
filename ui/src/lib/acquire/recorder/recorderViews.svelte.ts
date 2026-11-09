/**
 * One window's Recorder run views: the layout's pages, the page this window shows, and the
 * subscription that keeps the window's plots equal to what that page draws (PROTOCOL.md, "Recorder
 * Display"). The page layer the shock types, Sine, TWR and Explorer share (`../pulse/pulseViews`)
 * does the work; this names the Recorder's pages and topics. The service sends a subscribed window
 * its data frame at its rate while the run is live, once more at the end, and at once on
 * subscribe; a window on the Recordings page asks for no plot and reads the run's live frame.
 */

import type { ServiceClient } from '$lib/protocol/client.svelte';
import { RECORDER_TOPICS, type RecorderData, type RecorderLiveQuantity } from '$lib/protocol/recorder';
import { PulseViewsState, type PulseViewsOptions } from '../pulse/pulseViews.svelte';
import { recorderPagesFor, type RecorderPageKind } from './recorderLayout';

export const RECORDER_TEST_TYPE = 'recorder';

export interface RecorderViewsOptions extends PulseViewsOptions {
	/** The setup holds an FRF pair: a new layout then has an FRF page. */
	hasPair?: () => boolean;
}

export class RecorderViewsState extends PulseViewsState<RecorderPageKind, RecorderLiveQuantity, RecorderData> {
	constructor(control: ServiceClient, opts: RecorderViewsOptions) {
		super(control, { pages: recorderPagesFor(opts.hasPair ?? (() => true)), dataTopic: RECORDER_TOPICS.data, subscribe: 'recorder.subscribe', pageKey: 'hertzforge.display.recorder.page' }, opts);
	}
}
