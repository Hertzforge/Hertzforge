/**
 * One window's MIMO Random display feed: it keeps the window's subscription (PROTOCOL.md, "MIMO
 * Random Display") equal to the plots the window shows, and holds the last `mimo_random.data`
 * frame for them.
 *
 * Only what is on screen is asked for, each plot at its own width: the service decimates every
 * quantity to that width before it is sent, so a 72-control run costs the socket what its plots
 * show and no more. A change of plots or rate is sent after a short debounce, one request in
 * flight at a time, the latest only; the subscription belongs to the connection, so a new
 * connection is subscribed again. With nothing shown, the window holds no subscription and
 * receives the run's status only.
 */

import { untrack } from 'svelte';
import type { ServiceClient } from '$lib/protocol/client.svelte';
import type { MimoData, MimoDataPlot, MimoSubscribePlot, MimoSubscribeResult } from '$lib/protocol/mimo';
import { MIMO_TOPICS } from '$lib/protocol/mimo';
import { toRefusal } from '../words';
import { mimoSubscriptionFor, type MimoPlotSpec } from './mimoLayout';

export interface MimoFeedOptions {
	/** The plots this window shows now, read reactively; empty for none. */
	plots: () => MimoPlotSpec[];
	/** Frames per second this window asks for, read reactively. */
	rate: () => number;
	/** Debounce of the subscription, ms. */
	subscribeDelayMs?: number;
}

export class MimoFeed {
	readonly control: ServiceClient;
	private readonly opts: MimoFeedOptions;

	/** The last frame for this window, or null. */
	data = $state.raw<MimoData | null>(null);
	/** What the service accepted for this window, or null before the first answer. */
	accepted = $state.raw<MimoSubscribeResult | null>(null);
	/** Why the last subscription was refused, in words, or null. */
	error = $state<string | null>(null);

	private want: { key: string; params: Record<string, unknown> } | null = null;
	private sentKey = '';
	private timer: ReturnType<typeof setTimeout> | null = null;
	private inflight = false;

	constructor(control: ServiceClient, opts: MimoFeedOptions) {
		this.control = control;
		this.opts = opts;
		control.on(MIMO_TOPICS.data, (d) => {
			this.data = d as unknown as MimoData;
		});
	}

	/** This plot's data in the last frame, or null. */
	plotData(id: string): MimoDataPlot | null {
		return this.data?.plots.find((p) => p.id === id) ?? null;
	}

	/** What the window asks for now, as the service reads it. */
	static params(plots: MimoPlotSpec[], rate: number): Record<string, unknown> {
		const wanted: MimoSubscribePlot[] = mimoSubscriptionFor(plots);
		return wanted.length ? { rate_hz: rate, plots: wanted } : { default: true };
	}

	/** Keep the subscription current while the returned function has not been called. */
	watch(): () => void {
		const stop = $effect.root(() => {
			$effect(() => {
				const params = MimoFeed.params($state.snapshot(this.opts.plots()) as MimoPlotSpec[], this.opts.rate());
				const open = this.control.status === 'open';
				untrack(() => {
					if (!open) {
						this.want = null;
						this.sentKey = '';
						this.cancelTimer();
						return;
					}
					this.want = { key: JSON.stringify(params), params };
					if (this.want.key !== this.sentKey) this.schedule();
				});
			});
		});
		return () => {
			this.cancelTimer();
			stop();
			// The subscription belongs to the connection: another feed on it may replace it
			// while this one is not watching, so the next watch sends again.
			this.want = null;
			this.sentKey = '';
		};
	}

	private schedule() {
		if (this.timer) return;
		this.timer = setTimeout(() => {
			this.timer = null;
			void this.send();
		}, this.opts.subscribeDelayMs ?? 40);
	}

	private cancelTimer() {
		if (this.timer) clearTimeout(this.timer);
		this.timer = null;
	}

	private async send() {
		const want = this.want;
		if (this.inflight || !want || want.key === this.sentKey) return;
		this.inflight = true;
		this.sentKey = want.key;
		try {
			this.accepted = await this.control.request<MimoSubscribeResult>('mimo_random.subscribe', want.params);
			this.error = null;
			if (this.accepted.default) this.data = null;
		} catch (err) {
			this.error = toRefusal('Show the plots', err).message;
		} finally {
			this.inflight = false;
			if (this.want && this.want.key !== this.sentKey) void this.send();
		}
	}
}
