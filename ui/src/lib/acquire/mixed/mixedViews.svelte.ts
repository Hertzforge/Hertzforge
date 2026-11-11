/**
 * One window's Mixed Mode run screen: its pages of plots and cards, the page this window shows, and
 * the data the control service sends for it through `mixed.subscribe` (PROTOCOL.md, "Mixed Mode
 * Display"). The service sends a window's data frame at its rate while the run is live, and once at
 * once on subscribe. A frame of another run than the one on screen is not drawn: a frame from the
 * run before can still arrive after a new one has been configured.
 */

import type { ServiceClient } from '$lib/protocol/client.svelte';
import { MIXED_TOPICS, type MixedData, type MixedSubscribeResult } from '$lib/protocol/mixed';
import { PulseScreenState } from '../pulse/pulseScreenState.svelte';
import type { FramePlot, ScreenOptions } from '../screen/screen.svelte';
import { mixedHooks } from './mixedLayout';

export const MIXED_TEST_TYPE = 'mixed';

export const MIXED_HOOKS = mixedHooks({ method: 'mixed.subscribe', topic: MIXED_TOPICS.data });

export interface MixedViewsOptions extends ScreenOptions {
	/** The run on screen: a frame of any other run is not drawn. */
	runId?: () => string | null;
}

export class MixedViewsState extends PulseScreenState<MixedData, MixedSubscribeResult> {
	private readonly runId: (() => string | null) | null;

	constructor(control: ServiceClient, opts: MixedViewsOptions) {
		super(control, MIXED_HOOKS, opts);
		this.runId = opts.runId ?? null;
	}

	/** The last frame, when it is the run's on screen. */
	get frameOfRun(): MixedData | null {
		const d = this.data;
		if (!d) return null;
		const run = this.runId?.() ?? null;
		return run !== null && d.run_id !== run ? null : d;
	}

	override frame(id: string): FramePlot | null {
		return this.frameOfRun ? super.frame(id) : null;
	}
}
