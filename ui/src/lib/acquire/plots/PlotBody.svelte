<script lang="ts">
	/**
	 * The drawing area of one plot panel: the views the service sent for it, drawn with the panel's
	 * display options. Several channels are one pane each, all on one pane, or (on a time plot) one
	 * lane each. The legend and the readout draw inside the plot unless moved out or switched off.
	 * A zoom, a line hidden from the legend, a cursor placed or a note moved comes back through
	 * `onspec`, so it is kept with the layout.
	 *
	 * Every number drawn is the service's; a hold or an average is drawn beside its line and named
	 * as one.
	 */
	import type { ReadingView, View } from '$lib/protocol/types';
	import { copyCanvas, plotBackground } from '$lib/viz/copyImage';
	import { formatNumber } from '$lib/viz/format';
	import ReadingPane from '$lib/viz/ReadingPane.svelte';
	import type { ZoomChange } from '$lib/viz/paneOptions';
	import { safeFileName, saveFile } from '$lib/viz/saveFile';
	import SpectrumPane from '$lib/viz/SpectrumPane.svelte';
	import { cssVar } from '$lib/theme/theme.svelte';
	import { onDestroy, untrack } from 'svelte';
	import { csvCell, toCsv } from './csv';
	import { paneMarks, placeCursor } from './cursors';
	import { plotTitle } from './display';
	import { Holder, withHolds } from './hold';
	import { timeLinks } from './linkTime.svelte';
	import { isShown, present, traceList } from './present';
	import type { AxisDisplay, PlotDisplay, PlotPanelSpec, PlotType, TraceStyle } from './types';

	interface Props {
		views: View[];
		type: PlotType | null;
		spec: PlotPanelSpec;
		/** What the plot says while it has nothing to draw. */
		empty: string;
		/** Live data: hold ranges steady across frames. */
		live?: boolean;
		/** An in-plot change to the spec (a zoom held, a note placed, a trace hidden from the legend). */
		onspec?: (spec: PlotPanelSpec) => void;
		/** The test's band, for an x range held to it; without it the band is read from the reference line. */
		band?: [number, number] | null;
		/** The display stopped on what it shows, while the data keeps arriving. */
		frozen?: boolean;
		/** The plot's channels, one per view in the views' order (`DataPlot.channels`): they name the panes and the lines. */
		channels?: number[] | null;
		/**
		 * Time plots with the same key share one time window: they zoom and pan together, and freeze
		 * and run again together. Null for a plot of its own.
		 */
		linkKey?: string | null;
	}
	let { views, type, spec, empty, live = true, onspec, band = null, frozen = $bindable(false), channels = null, linkKey = null }: Props = $props();

	const display = $derived(spec.display);
	const time = $derived(type?.time ?? false);
	const numbers = $derived(type?.group === 'numbers');

	// Holds step once for each frame the service sends; the same frame drawn again (an option
	// changed) gives back what it gave before.
	const holder = new Holder();
	let lastIn: View[] | null = null;
	let lastKey = '';
	let lastOut: View[] = [];
	const held = $derived.by(() => {
		const key = `${display.hold}:${display.average}:${display.holdEpoch ?? 0}:${time}`;
		if (views === lastIn && key === lastKey) return lastOut;
		lastIn = views;
		lastKey = key;
		lastOut = withHolds(holder, views, time || numbers ? 'none' : display.hold, display.average, display.holdEpoch ?? 0);
		return lastOut;
	});

	/** What the plot showed when it froze. */
	let still = $state.raw<View[] | null>(null);
	$effect.pre(() => {
		if (!frozen) still = null;
		else if (!untrack(() => still)) still = untrack(() => held);
	});
	const shown = $derived(still ?? held);

	// Linked time axes: the link holds the x range, the freeze and the window of every plot in it.
	const linked = $derived(time && linkKey ? linkKey : null);
	$effect(() => {
		if (linked) return timeLinks.join(linked);
	});
	const link = $derived(linked ? timeLinks.get(linked) : null);
	// a plot that joins takes the link's freeze and window; a change made on any plot goes to all
	let sentFrozen: boolean | undefined;
	$effect(() => {
		const f = frozen;
		if (!linked) return;
		untrack(() => {
			if (sentFrozen !== undefined && f !== sentFrozen && timeLinks.get(linked).frozen !== f) timeLinks.set(linked, { frozen: f });
			sentFrozen = f;
		});
	});
	$effect(() => {
		const f = link?.frozen;
		if (f === undefined) return;
		untrack(() => {
			sentFrozen = f;
			if (frozen !== f) frozen = f;
		});
	});
	let sentWindow: number | null | undefined;
	let windowSeen = false;
	$effect(() => {
		const w = display.window_s;
		if (!linked) return;
		untrack(() => {
			const now = timeLinks.get(linked).window;
			if (now === undefined || (windowSeen && w !== sentWindow)) timeLinks.set(linked, { window: w });
			sentWindow = w;
			windowSeen = true;
		});
	});
	/** The spec drawn: the panel's own, in the link's time window when it has one. */
	const drawnSpec = $derived(link && link.window !== undefined && link.window !== display.window_s ? { ...spec, display: { ...display, window_s: link.window } } : spec);

	/** Columns before the panes are known: a time plot's lanes are full width. */
	const cols0 = $derived(time ? 1 : spec.arrange === 'overlay' ? 1 : 2);
	/** The body's width: an envelope keeps about one interval per pixel. */
	let width = $state(600);
	const presented = $derived(present(shown, drawnSpec, type, { band, channels, buckets: Math.max(50, Math.round(width / Math.max(1, cols0))) }));
	const panes = $derived(presented.panes);
	const readings = $derived(presented.readings);
	const drawn = $derived(panes.some((p) => p.view.series.length > 0) || readings.length > 0);
	const cols = $derived(presented.layout === 'lanes' || panes.length <= 1 ? 1 : panes.length <= 4 ? 2 : 3);
	const title = $derived(plotTitle(spec, type));

	let paneRefs = $state<(SpectrumPane | undefined)[]>([]);
	let bodyEl: HTMLDivElement;

	function change(next: Partial<PlotDisplay>) {
		onspec?.({ ...spec, display: { ...display, ...next } });
	}

	/** An axis held where a zoom left it, or let go by a double-click. */
	function zoomed(axis: AxisDisplay, to: [number, number] | null | undefined): AxisDisplay {
		if (to === undefined) return axis;
		if (to) return { ...axis, range: 'held', min: to[0], max: to[1] };
		return axis.range === 'held' ? { ...axis, range: 'auto', min: null, max: null } : axis;
	}

	function zoom(z: ZoomChange) {
		// a time plot stops on what it shows while zoomed, and runs again when the zoom is let go
		if (time && z.x) frozen = true;
		if (time && z.x === null) frozen = false;
		if (linked) {
			// the time axis is the link's: every plot in it shows the zoom
			if (z.x !== undefined) timeLinks.set(linked, { x: z.x, frozen: z.x !== null });
			if (z.y !== undefined) change({ y: zoomed(display.y, z.y) });
			return;
		}
		change({ x: zoomed(display.x, z.x), y: zoomed(display.y, z.y) });
	}

	/** Run again: a time plot's zoom on the frozen times is let go, since the times move on. */
	function resume() {
		frozen = false;
		if (linked) timeLinks.set(linked, { x: null, frozen: false });
		if (time && display.x.range === 'held') change({ x: zoomed(display.x, null) });
	}

	function toggleTrace(name: string) {
		const was: TraceStyle = display.traces[name] ?? { visible: true, width: null, dash: null, color: null };
		change({ traces: { ...display.traces, [name]: { ...was, visible: !was.visible } } });
	}

	function pick(x: number, logX: boolean) {
		if (display.cursor !== 'one' && display.cursor !== 'two' && display.cursor !== 'harmonics') return;
		change({ cursorAt: placeCursor(display.cursorAt ?? [null, null], x, display.cursor, logX) });
	}

	function moveNote(id: string, fx: number, fy: number) {
		change({ notes: display.notes.map((n) => (n.id === id ? { ...n, fx, fy } : n)) });
	}

	// Held chosen in the options with no range yet: hold the range on screen now (once the plot
	// is drawn, trying again for a short while).
	let tries = $state(0);
	$effect(() => {
		const wantX = display.x.range === 'held' && (display.x.min === null || display.x.max === null);
		const wantY = display.y.range === 'held' && (display.y.min === null || display.y.max === null);
		if (!wantX && !wantY) return;
		void tries;
		const first = paneRefs.find((p) => p);
		const now = untrack(() => first?.ranges());
		if (!now?.x || !now.y) {
			if (untrack(() => tries) < 30) requestAnimationFrame(() => tries++);
			return;
		}
		untrack(() => {
			tries = 0;
			change({
				x: wantX ? { ...display.x, min: now.x![0], max: now.x![1] } : display.x,
				y: wantY ? { ...display.y, min: now.y![0], max: now.y![1] } : display.y
			});
		});
	});

	/** The lines the plot draws, by name and role: for the options' trace list. */
	export function traces(): { name: string; role: string }[] {
		return traceList(presented);
	}

	let status = $state<string | null>(null);
	let statusTimer: ReturnType<typeof setTimeout> | undefined;
	function say(text: string) {
		status = text;
		clearTimeout(statusTimer);
		statusTimer = setTimeout(() => (status = null), 1800);
	}
	onDestroy(() => clearTimeout(statusTimer));

	/** A readings view as rows of text, for an image. */
	function readingRows(r: ReadingView): string[] {
		return r.rows.map((row) => `${row.label}  ${typeof row.value === 'number' ? formatNumber(row.value, 4) : row.value}${row.unit ? ` ${row.unit}` : ''}`);
	}

	/** The plot as one image: its title, then each pane as it is drawn, in the panes' grid. */
	function compose(): HTMLCanvasElement | null {
		const r = devicePixelRatio || 1;
		const parts = paneRefs.slice(0, panes.length).map((p) => p?.imageCanvas() ?? null);
		const canvases = parts.filter((c): c is HTMLCanvasElement => c !== null);
		const textRows = readings.flatMap(readingRows);
		if (!canvases.length && !textRows.length) return null;
		const head = Math.round(24 * r);
		const paneHead = panes.length > 1 ? Math.round(18 * r) : 0;
		const across = Math.max(1, Math.min(cols, canvases.length));
		const cellW = Math.max(Math.round(320 * r), ...canvases.map((c) => c.width));
		const cellH = Math.max(0, ...canvases.map((c) => c.height)) + (canvases.length ? paneHead : 0);
		const rows = Math.ceil(canvases.length / across);
		const textH = textRows.length ? Math.round((textRows.length * 17 + 12) * r) : 0;
		const out = document.createElement('canvas');
		out.width = cellW * across;
		out.height = head + rows * cellH + textH;
		const ctx = out.getContext('2d');
		if (!ctx) return null;
		ctx.fillStyle = plotBackground(bodyEl);
		ctx.fillRect(0, 0, out.width, out.height);
		ctx.fillStyle = cssVar('--text-1');
		ctx.textBaseline = 'middle';
		ctx.font = `600 ${Math.round(13 * r)}px ${cssVar('--font-ui')}`;
		ctx.fillText(title, 8 * r, head / 2);
		ctx.font = `${Math.round(11.5 * r)}px ${cssVar('--font-ui')}`;
		let k = 0;
		parts.forEach((c, i) => {
			if (!c) return;
			const x = (k % across) * cellW;
			const y = head + Math.floor(k / across) * cellH;
			if (paneHead) {
				ctx.fillStyle = cssVar('--text-2');
				ctx.fillText(panes[i].view.title, x + 8 * r, y + paneHead / 2);
			}
			ctx.drawImage(c, x, y + paneHead);
			k++;
		});
		ctx.font = `${Math.round(11.5 * r)}px ${cssVar('--font-mono')}`;
		ctx.fillStyle = cssVar('--text-1');
		textRows.forEach((t, j) => ctx.fillText(t, 8 * r, head + rows * cellH + (12 + j * 17) * r));
		return out;
	}

	/** The plot, with its legend, marks and notes, as an image on the clipboard. */
	export async function copyImage(): Promise<void> {
		const canvas = compose();
		if (!canvas) return;
		const done = await copyCanvas(canvas, plotBackground(bodyEl), undefined, safeFileName(title));
		if (done) say(done === 'copied' ? 'Copied' : 'Saved as PNG');
	}

	/** The plot as a PNG file, saved where the person chooses (downloaded in a browser). */
	export async function saveImage(name: string): Promise<void> {
		const canvas = compose();
		if (!canvas) return;
		const blob = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, 'image/png'));
		if (!blob) return;
		try {
			const done = await saveFile(`${safeFileName(name || title)}.png`, blob, { name: 'PNG image', extensions: ['png'] }, 'Save Image');
			if (done !== 'cancelled') say('Saved');
		} catch (e) {
			say(`Not saved: ${e instanceof Error ? e.message : String(e)}`);
			throw e;
		}
	}

	/** The plot's lines (or its numbers) as CSV, saved where the person chooses (downloaded in a browser). */
	export async function saveCsv(name: string): Promise<void> {
		let text = '';
		if (panes.length) text = toCsv(panes.map((p) => p.view), (vi, si) => isShown(panes[vi].styles, si));
		if (readings.length) {
			const rows = readings.flatMap((r) => r.rows.map((row) => [row.label, String(row.value), row.unit ?? ''].map(csvCell).join(',')));
			text += (text ? '\n' : '') + ['Reading,Value,Unit', ...rows].join('\n') + '\n';
		}
		if (!text) return;
		try {
			// the mark at the start tells a spreadsheet the text is UTF-8 (units such as g²/Hz)
			const done = await saveFile(`${safeFileName(name || title)}.csv`, `﻿${text}`, { name: 'CSV', extensions: ['csv'] }, 'Save CSV');
			if (done !== 'cancelled') say('Saved');
		} catch (e) {
			say(`Not saved: ${e instanceof Error ? e.message : String(e)}`);
			throw e;
		}
	}
</script>

<div class="body" bind:this={bodyEl} bind:clientWidth={width} data-quantity={type?.id ?? spec.quantity} data-testid="plot-body">
	{#if !drawn}
		<p class="empty">{empty}</p>
	{:else}
		{#if panes.length}
			<div class="panes" class:lanes={presented.layout === 'lanes'} style:--cols={cols}>
				{#each panes as p, i (p.key)}
					{@const logX = p.view.x.scale === 'log'}
					<div class="pane" data-testid="plot-pane">
						<SpectrumPane
							bind:this={paneRefs[i]}
							view={p.view}
							{live}
							followX={time}
							showTitle={panes.length > 1}
							minHeight={60}
							copyButton={false}
							grid={display.grid}
							legend={display.legend}
							legendCorner={display.legendCorner}
							readout={display.readout}
							styles={p.styles}
							onlegend={onspec ? (k) => toggleTrace(p.view.series[k].name) : undefined}
							xFixed={link?.x ?? p.xFixed}
							yFixed={p.yFixed}
							zoomY
							onZoom={onspec ? zoom : undefined}
							wheelZoom={time}
							{frozen}
							marks={paneMarks(p.view, display, (k) => isShown(p.styles, k))}
							notes={i === 0 ? display.notes : []}
							onnote={onspec ? moveNote : undefined}
							xTicks={display.x.ticks}
							yTicks={display.y.ticks}
							xLabel={display.x.label}
							yLabel={display.y.label}
							envelope={time && display.timeRender === 'envelope'}
							onPick={(x) => pick(x, logX)}
						/>
					</div>
				{/each}
			</div>
		{/if}
		{#each readings as r, i (i)}
			<div class="readings"><ReadingPane view={r} showTitle={panes.length > 0 || readings.length > 1} /></div>
		{/each}
	{/if}
	{#if frozen}
		<div class="chip frozen" role="status">
			<span>Frozen</span>
			<button class="btn ghost sm" onclick={resume}>Resume</button>
		</div>
	{/if}
	{#if status}
		<div class="chip said" role="status">{status}</div>
	{/if}
</div>

<style>
	.body {
		position: relative;
		display: flex;
		flex: 1 1 0;
		flex-direction: column;
		gap: var(--sp-2);
		min-height: 0;
		height: 100%;
	}

	.panes {
		flex: 1 1 0;
		min-height: 0;
		display: grid;
		grid-template-columns: repeat(var(--cols), minmax(0, 1fr));
		grid-auto-rows: minmax(0, 1fr);
		gap: var(--sp-2);
	}

	.panes.lanes {
		gap: 2px;
	}

	.pane {
		min-height: 0;
		min-width: 0;
	}

	.readings {
		flex: 1 1 auto;
		min-height: 0;
		overflow: auto;
	}

	.panes ~ .readings {
		flex: none;
		max-height: 35%;
	}

	.empty {
		margin: auto;
		color: var(--text-3);
		font-size: 13px;
	}

	.chip {
		position: absolute;
		z-index: 3;
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
		padding: 2px 4px 2px 8px;
		font-size: var(--text-xs);
		color: var(--text-1);
		background: var(--bg-3);
		border: 1px solid var(--line-2);
		border-radius: var(--radius-sm);
	}

	.chip.frozen {
		left: 4px;
		bottom: 4px;
	}

	.chip.said {
		right: 6px;
		bottom: 6px;
		padding: 2px 8px;
	}

	.chip .btn {
		height: 20px;
	}
</style>
