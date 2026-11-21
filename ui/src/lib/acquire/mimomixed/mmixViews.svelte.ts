/**
 * One window's MIMO Mixed Mode run screen: its pages of plots and cards, the page this window
 * shows, and the data the service sends for it through `mimo_mixed.subscribe`.
 */

import type { ServiceClient } from '$lib/protocol/client.svelte';
import { MIMO_MIXED_TOPICS, type MimoMixedData, type MimoMixedQuantity, type MimoMixedSubscribeResult } from '$lib/protocol/mimoMixed';
import { pulseHooks } from '../pulse/pulseScreen';
import { PulseScreenState } from '../pulse/pulseScreenState.svelte';
import type { ScreenHooks, ScreenOptions } from '../screen/screen.svelte';
import { MMIX_SCREEN } from './mmixLayout';

export const MMIX_TEST_TYPE = 'mimo_mixed';

export const MMIX_HOOKS: ScreenHooks = pulseHooks(MMIX_SCREEN, { method: 'mimo_mixed.subscribe', topic: MIMO_MIXED_TOPICS.data });

export class MmixViewsState extends PulseScreenState<MimoMixedData, MimoMixedSubscribeResult> {
	constructor(control: ServiceClient, opts: ScreenOptions) {
		super(control, MMIX_HOOKS, opts);
	}
}

export interface ReviewPages {
	review: string | null;
	proceeded: string | null;
}

/** The identification page when the run holds for review, then the controls once it drives again: once each per run. */
export function reviewPage(s: { state: string; run_id: string | null } | null, seen: ReviewPages): MimoMixedQuantity | null {
	if (!s?.run_id) return null;
	if (s.state === 'review' && seen.review !== s.run_id) {
		seen.review = s.run_id;
		return 'identification';
	}
	if (s.state === 'running' && seen.review === s.run_id && seen.proceeded !== s.run_id) {
		seen.proceeded = s.run_id;
		return 'control';
	}
	return null;
}
