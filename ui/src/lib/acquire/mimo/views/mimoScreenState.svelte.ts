/**
 * One window's MIMO Random run screen: its pages of plots and cards (a `ScreenState` with MIMO's
 * plot types, cards and first pages), the page this window shows, the run's status, and the data
 * the control service sends for the plots on screen through `mimo_random.subscribe`.
 *
 * The plots ask for their data at the width they are drawn, so the service decimates every
 * quantity to what is on screen; a page this window does not show costs nothing. Layouts the MIMO
 * run views kept are taken over the first time each is read.
 */

import { untrack } from 'svelte';
import type { ServiceClient } from '$lib/protocol/client.svelte';
import { MIMO_TOPICS, type MimoData, type MimoDataPlot, type MimoQuantity, type MimoStatus, type MimoSubscribeResult } from '$lib/protocol/mimo';
import type { ChannelSelector } from '$lib/protocol/types';
import { revealControl } from '$lib/viz/matrixGrid';
import { MIMO_QUANTITY_WORDS } from '../../display/mimoLayout';
import { regresses } from '../../runOrder';
import type { PlotSelect, PlotType } from '../../plots/types';
import { ScreenState, type ScreenOptions } from '../../screen/screen.svelte';
import type { Panel } from '../../screen/types';
import { findPlot, gridChoice, mimoHooks, mimoSpec, mimoTitle, selectorOf, type MimoContext } from './mimoScreen';

export interface MimoScreenOptions extends ScreenOptions {
	/** The screen's pixel ratio, for the points a plot asks for; the device's (at most 2) otherwise. */
	pixelRatio?: number;
}

/** What the plots depend on besides their choices, kept apart so the subscription can read it. */
class RunContext {
	/** The run's last status, or null before one has come. */
	status = $state.raw<MimoStatus | null>(null);
	/** Each panel's drawn widths, by part, in CSS pixels. Not kept: each window measures its own. */
	widths = $state<Record<string, Record<string, number>>>({});
	quantities: () => MimoQuantity[] | null = () => null;
	private readonly pixelRatio: number | undefined;

	/** Take a status, unless it is older than the run's own word that it finished. */
	take(s: MimoStatus): void {
		if (!regresses(this.status, s)) this.status = s;
	}

	constructor(pixelRatio?: number) {
		this.pixelRatio = pixelRatio;
	}

	view(): MimoContext {
		return {
			labels: this.status?.shape?.labels ?? [],
			drive_labels: this.status?.shape?.drive_labels ?? [],
			space: this.status?.shape?.space ?? null,
			quantities: this.quantities(),
			widths: this.widths,
			pixelRatio: this.pixelRatio ?? (typeof devicePixelRatio === 'number' ? Math.min(2, devicePixelRatio) : 1)
		};
	}
}

export class MimoScreenState extends ScreenState<MimoData, MimoSubscribeResult> {
	private readonly run: RunContext;

	constructor(control: ServiceClient, opts: MimoScreenOptions) {
		const run = new RunContext(opts.pixelRatio);
		super(control, mimoHooks(() => run.view()), opts);
		this.run = run;
		run.quantities = () => this.accepted?.quantities ?? null;
		control.on(MIMO_TOPICS.status, (d) => run.take(d as unknown as MimoStatus));
	}

	// =========================================================================================
	// Reading

	get status(): MimoStatus | null {
		return this.run.status;
	}

	get labels(): string[] {
		return this.run.status?.shape?.labels ?? [];
	}

	get driveLabels(): string[] {
		return this.run.status?.shape?.drive_labels ?? [];
	}

	/** The quantities the service offers, or null before it has said. */
	get quantities(): MimoQuantity[] | null {
		return this.accepted?.quantities ?? null;
	}

	/** What the plots depend on now. */
	get context(): MimoContext {
		return this.run.view();
	}

	/** One plot's, or one part's, data in the last frame, or null. */
	plotData(id: string): MimoDataPlot | null {
		return (this.data?.plots.find((p) => p.id === id) as MimoDataPlot | undefined) ?? null;
	}

	override titleOf(panel: Panel): string {
		return panel.kind === 'plot' ? mimoTitle(panel.plot, { labels: this.labels, drive_labels: this.driveLabels, quantities: this.quantities }) : super.titleOf(panel);
	}

	// =========================================================================================
	// Plots

	/** A new plot starts with MIMO's own defaults: its first selector and its axis. */
	override addPlot(type: PlotType | string, channels: ChannelSelector = 'all'): string | null {
		const id = super.addPlot(type, channels);
		const t = typeof type === 'string' ? this.typeOf(type) : type;
		if (id && t) {
			const spec = mimoSpec(t);
			this.updatePlot(id, { select: spec.select, display: spec.display });
		}
		return id;
	}

	override removePanel(id: string): void {
		super.removePanel(id);
		if (this.run.widths[id]) {
			const { [id]: _gone, ...rest } = this.run.widths;
			void _gone;
			this.run.widths = rest;
		}
	}

	/** Change part of a plot's own choice. */
	choose(panelId: string, patch: PlotSelect): void {
		const spec = this.plot(panelId);
		if (spec) this.updatePlot(panelId, { select: { ...(spec.select ?? {}), ...patch } });
	}

	/** A plot's part was drawn this wide (CSS pixels); its data is asked for to match. */
	setWidth(panelId: string, part: string, px: number): void {
		const w = Math.round(px);
		if (!(w > 0) || this.run.widths[panelId]?.[part] === w) return;
		this.run.widths[panelId] = { ...(this.run.widths[panelId] ?? {}), [part]: w };
	}

	/** Bring a control into a matrix grid's window and show its page (from the heatmap); a grid is added when there is none. */
	openInGrid(position: number): void {
		let found = findPlot(this.layout, 'sdm');
		if (!found) {
			if (!this.addPage(MIMO_QUANTITY_WORDS.sdm) || !this.addPlot('sdm')) return;
			found = findPlot(this.layout, 'sdm');
			if (!found) return;
		}
		this.choose(found.panel.id, { window: revealControl(this.labels, gridChoice(found.panel.plot.select).window, position) });
		this.selectPage(found.pageId);
	}

	/** Show drive `k` on the page's drive spectra that show one drive; false when there is none. */
	showDrive(k: number): boolean {
		let shown = false;
		for (const p of this.page.panels) {
			if (p.kind !== 'plot' || p.plot.quantity !== 'drive_psd' || selectorOf('drive_psd', p.plot.select).kind !== 'index') continue;
			this.choose(p.id, { kind: 'index', index: k });
			shown = true;
		}
		return shown;
	}

	// =========================================================================================
	// Wiring

	/** Keep the layout and the subscription (the frame's own), and the run's status on every connection. */
	override watch(): () => void {
		const stop = super.watch();
		const stopStatus = $effect.root(() => {
			$effect(() => {
				if (this.control.status !== 'open') return;
				untrack(() => {
					this.control
						.request<MimoStatus>('mimo_random.status', {})
						.then((s) => this.run.take(s))
						.catch(() => {
							/* a service without MIMO Random: the plots wait */
						});
				});
			});
		});
		return () => {
			stop();
			stopStatus();
		};
	}
}
