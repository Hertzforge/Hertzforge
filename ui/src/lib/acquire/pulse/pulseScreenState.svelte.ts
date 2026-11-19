/**
 * One window's test screen for a test type on the pulse page layer: a `ScreenState` with the type's
 * plot types, cards, first pages and feed (`pulseHooks`), what the service accepted, and the views a
 * plot draws, with anything the test type lays over them (the pulses picked in the pulse table).
 */

import type { ServiceClient } from '$lib/protocol/client.svelte';
import type { View } from '$lib/protocol/types';
import { ScreenState, type ScreenFrame, type ScreenHooks, type ScreenOptions } from '../screen/screen.svelte';

/** What a subscription was accepted as. */
export interface PulseSubscribed {
	default: boolean;
	rate_hz: number;
	plots: { id: string; quantity: string; points?: number | null; channels?: unknown }[] | null;
	quantities: string[];
}

/** Views a test type lays something over: the plot's quantity and what the service sent for it. */
export type ViewsOver = (quantity: string, views: View[]) => View[];

export class PulseScreenState<D extends ScreenFrame = ScreenFrame, A extends PulseSubscribed = PulseSubscribed> extends ScreenState<D, A> {
	private readonly over: ViewsOver | null;

	constructor(control: ServiceClient, hooks: ScreenHooks, opts: ScreenOptions, over: ViewsOver | null = null) {
		super(control, hooks, opts);
		this.over = over;
	}

	/** The quantities the service offers (all the type's until it has said). */
	get quantities(): string[] {
		return this.accepted?.quantities ?? this.hooks.types.map((t) => t.id);
	}

	/** A plot's views in the last frame, with what the test type lays over them. */
	override views(id: string): View[] {
		const got = super.views(id);
		const quantity = this.over ? this.plot(id)?.quantity : undefined;
		return quantity && this.over ? this.over(quantity, got) : got;
	}

	/** The first page holding a plot of `quantity`, or null. */
	pageWith(quantity: string): string | null {
		return this.pages.find((p) => p.panels.some((x) => x.kind === 'plot' && x.plot.quantity === quantity))?.id ?? null;
	}

	/** Show the first page holding a plot of `quantity`; false when no page does. */
	bringForward(quantity: string): boolean {
		const id = this.pageWith(quantity);
		if (id) this.selectPage(id);
		return id !== null;
	}
}
