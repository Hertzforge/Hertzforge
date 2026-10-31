<script lang="ts">
	/**
	 * x-y line view (spectra and anything else on two axes), drawn with uPlot.
	 *
	 * Log or linear axes come from the view. Each series' role sets its dash pattern and width;
	 * color is the second cue, and the legend names the line style in words. Panes with the same
	 * `link` share a cursor on x through the CursorHub. Live updates keep the axes still: the
	 * plot is rebuilt only when the structure changes, and the y range only moves when the data
	 * leaves it.
	 */
	import uPlot from 'uplot';
	import 'uplot/dist/uPlot.min.css';
	import { onDestroy, onMount } from 'svelte';
	import type { Series, SpectrumView } from '$lib/protocol/types';
	import { cssVar, theme } from '$lib/theme/theme.svelte';
	import { alignSeries, valueAt } from './align';
	import { cursors } from './cursor';
	import { formatFixed, formatTick } from './format';
	import { logRange, logTicks, mantissasFor } from './logTicks';
	import { ROLE_STYLES, dashArray, roleColorVar } from './roles';
	import { stepsPath } from './steps';

	interface Props {
		view: SpectrumView;
		/** Hold the y range steady across updates (live data). */
		live?: boolean;
		/** Minimum plot height in px. */
		minHeight?: number;
		/** Show the view's title above the legend. */
		showTitle?: boolean;
	}

	let { view, live = false, minHeight = 180, showTitle = true }: Props = $props();

	const paneId = `sp-${Math.random().toString(36).slice(2, 9)}`;

	let plotEl: HTMLDivElement;
	let plot: uPlot | null = null;
	let structureKey = '';
	let yRange: [number, number] | null = null;
	let resizeObs: ResizeObserver | null = null;
	let unsubscribeCursor: (() => void) | null = null;
	let subscribedLink: string | undefined;

	let cursorX = $state<number | null>(null);
	let cursorIsLocal = $state(false);

	const logX = $derived(view.x.scale === 'log');
	const logY = $derived(view.y.scale === 'log');

	/** Per-series style, with measured series stepping through the blue family. */
	const styled = $derived.by(() => {
		const counts: Record<string, number> = {};
		return view.series.map((s, i) => {
			const nth = (counts[s.role] = (counts[s.role] ?? -1) + 1);
			return { series: s, index: i, style: ROLE_STYLES[s.role] ?? ROLE_STYLES.other, colorVar: roleColorVar(s.role, nth) };
		});
	});

	/** Plot order: draw low-z roles first so the measured line stays on top. */
	const drawOrder = $derived([...styled].sort((a, b) => a.style.z - b.style.z || a.index - b.index));

	const readouts = $derived(
		styled.map(({ series }) => (cursorX === null ? null : valueAt(series, cursorX, logX, logY)))
	);

	function axisTitle(label: string, unit: string | null) {
		return unit ? `${label} (${unit})` : label;
	}

	function keyOf(v: SpectrumView): string {
		const xs = v.series.map((s) => [s.name, s.role, s.shape ?? 'line', s.x.length, s.x[0], s.x[s.x.length - 1]]);
		return JSON.stringify([v.x, v.y, xs, theme.version]);
	}

	function finiteExtent(values: ArrayLike<number | null>[], positive: boolean): [number, number] | null {
		let lo = Infinity;
		let hi = -Infinity;
		for (const col of values) {
			for (let i = 0; i < col.length; i++) {
				const v = col[i];
				if (v === null || !Number.isFinite(v) || (positive && v <= 0)) continue;
				if (v < lo) lo = v;
				if (v > hi) hi = v;
			}
		}
		return lo <= hi ? [lo, hi] : null;
	}

	function linearRange(lo: number, hi: number): [number, number] {
		if (lo === hi) {
			const pad = Math.abs(lo) * 0.1 || 1;
			return [lo - pad, hi + pad];
		}
		const span = hi - lo;
		const mag = 10 ** Math.floor(Math.log10(span));
		const step = span / mag > 5 ? mag : span / mag > 2 ? mag / 2 : mag / 5;
		const a = lo >= 0 && lo - span * 0.04 < 0 ? 0 : Math.floor((lo - span * 0.04) / step) * step;
		const b = Math.ceil((hi + span * 0.04) / step) * step;
		return [a, b];
	}

	/** y range for data, snapped; with `live`, only moves when the data leaves it or shrinks far inside. */
	function nextYRange(cols: ArrayLike<number | null>[]): [number, number] {
		const ext = finiteExtent(cols, logY) ?? (logY ? [1e-4, 1] : [0, 1]);
		const target = logY ? logRange(ext[0], ext[1]) : linearRange(ext[0], ext[1]);
		if (!live || !yRange) return target;
		const [a, b] = yRange;
		const inside = ext[0] >= a && ext[1] <= b;
		const tooLoose = logY ? Math.log10(b / a) > Math.log10(target[1] / target[0]) + 1.01 : b - a > (target[1] - target[0]) * 2.5;
		return inside && !tooLoose ? yRange : target;
	}

	function font(size: number, mono = true) {
		return `${Math.round(size)}px ${mono ? cssVar('--font-mono') : cssVar('--font-ui')}`;
	}

	/** Pick tick density from the pixels each decade gets on this axis. */
	function logMantissas(u: uPlot, axis: 0 | 1, min: number, max: number): number[] {
		const px = (axis === 0 ? u.bbox.width : u.bbox.height) / devicePixelRatio;
		const decades = Math.max(0.3, Math.log10(Math.max(max, 1e-300) / Math.max(min, 1e-300)));
		return mantissasFor(px / decades);
	}

	let measureCtx: CanvasRenderingContext2D | null = null;
	function textWidth(text: string, f: string): number {
		measureCtx ??= document.createElement('canvas').getContext('2d');
		if (!measureCtx) return text.length * 7;
		measureCtx.font = f;
		return measureCtx.measureText(text).width;
	}

	function build() {
		destroyPlot();
		const data = alignSeries(view.series, logX, logY);
		const [xs, ...cols] = data;
		yRange = nextYRange(cols);
		const axisColor = cssVar('--plot-axis');
		const grid = cssVar('--plot-grid');
		const tickFont = font(11);
		// Whole pixels only: uPlot scales the integer part of the size for HiDPI.
		const labelFont = font(12, false);

		const ordered = drawOrder;
		const plotData = [xs, ...ordered.map((o) => cols[o.index])] as uPlot.AlignedData;

		const opts: uPlot.Options = {
			width: Math.max(100, plotEl.clientWidth),
			height: Math.max(minHeight, plotEl.clientHeight),
			pxAlign: false,
			legend: { show: false },
			padding: [10, 24, 0, 0],
			cursor: {
				y: false,
				points: { show: false },
				drag: { x: true, y: false, setScale: true },
				focus: { prox: -1 }
			},
			scales: {
				x: {
					time: false,
					distr: logX ? 3 : 1,
					log: 10,
					range: (_u, min, max) => {
						if (logX) {
							const lo = Number.isFinite(min) && min > 0 ? min : 1;
							const hi = Number.isFinite(max) && max > lo ? max : lo * 10;
							return [lo, hi];
						}
						return min === max ? [min - 1, max + 1] : [min, max];
					}
				},
				y: {
					distr: logY ? 3 : 1,
					log: 10,
					range: (_u, min, max) => {
						// uPlot passes the data extent on its own resets; zooms come through setScale.
						if (yRange && (!Number.isFinite(min) || !Number.isFinite(max))) return yRange;
						return yRange ?? [min, max];
					}
				}
			},
			axes: [
				{
					scale: 'x',
					stroke: axisColor,
					font: tickFont,
					labelFont,
					label: axisTitle(view.x.label, view.x.unit),
					labelSize: 22,
					labelGap: 2,
					size: 34,
					gap: 4,
					space: logX ? 36 : 56,
					grid: { stroke: grid, width: 1 },
					ticks: { stroke: grid, width: 1, size: 4 },
					splits: logX ? (u, _i, min, max) => logTicks(min, max, logMantissas(u, 0, min, max)) : undefined,
					filter: logX ? (_u, splits) => splits : undefined,
					values: (_u, splits) => splits.map((v) => (v == null ? '' : formatTick(v)))
				},
				{
					scale: 'y',
					stroke: axisColor,
					font: tickFont,
					labelFont,
					label: axisTitle(view.y.label, view.y.unit),
					labelSize: 20,
					labelGap: 0,
					gap: 6,
					space: 28,
					grid: { stroke: grid, width: 1 },
					ticks: { stroke: grid, width: 1, size: 4 },
					splits: logY ? (u, _i, min, max) => logTicks(min, max, logMantissas(u, 1, min, max)) : undefined,
					filter: logY ? (_u, splits) => splits : undefined,
					values: (_u, splits) => splits.map((v) => (v == null ? '' : formatTick(v))),
					size: (_u, values) => {
						const widest = Math.max(24, ...(values ?? []).map((v) => textWidth(String(v), tickFont)));
						return Math.ceil(widest + 14);
					}
				}
			],
			series: [
				{},
				...ordered.map(({ series, index, style, colorVar }) => ({
					label: series.name,
					stroke: cssVar(colorVar),
					width: style.width,
					dash: style.dash,
					spanGaps: true,
					points: { show: false },
					paths: series.shape === 'steps' && series.edges ? stepsPath(() => view.series[index], logX, logY) : undefined
				}))
			],
			hooks: {
				setCursor: [
					(u) => {
						const left = u.cursor.left ?? -1;
						if (left < 0) {
							cursorIsLocal = false;
							if (view.link) cursors.clear(view.link, paneId);
							else cursorX = null;
							return;
						}
						const x = u.posToVal(left, 'x');
						cursorIsLocal = true;
						if (view.link) cursors.set(view.link, x, paneId);
						else cursorX = x;
					}
				]
			}
		};

		plot = new uPlot(opts, plotData, plotEl);
		structureKey = keyOf(view);
		resizeObs = new ResizeObserver(() => {
			if (!plot) return;
			const w = Math.max(100, plotEl.clientWidth);
			const h = Math.max(minHeight, plotEl.clientHeight);
			if (w !== plot.width || h !== plot.height) plot.setSize({ width: w, height: h });
		});
		resizeObs.observe(plotEl);
	}

	function update() {
		if (!plot) return;
		if (keyOf(view) !== structureKey) return build();
		const [xs, ...cols] = alignSeries(view.series, logX, logY);
		const next = nextYRange(cols);
		const plotData = [xs, ...drawOrder.map((o) => cols[o.index])] as uPlot.AlignedData;
		plot.setData(plotData, false);
		if (!yRange || next[0] !== yRange[0] || next[1] !== yRange[1]) {
			yRange = next;
			plot.setScale('y', { min: next[0], max: next[1] });
		}
	}

	function destroyPlot() {
		resizeObs?.disconnect();
		resizeObs = null;
		plot?.destroy();
		plot = null;
	}

	function bindCursor(link: string | undefined) {
		if (link === subscribedLink) return;
		unsubscribeCursor?.();
		unsubscribeCursor = null;
		subscribedLink = link;
		if (!link) return;
		unsubscribeCursor = cursors.subscribe(link, (state) => {
			cursorX = state.x;
			if (!plot || state.source === paneId) return;
			cursorIsLocal = false;
			if (state.x === null) {
				plot.setCursor({ left: -10, top: -10 }, false);
				return;
			}
			const left = plot.valToPos(state.x, 'x');
			const inside = left >= 0 && left <= plot.bbox.width / devicePixelRatio;
			plot.setCursor({ left: inside ? left : -10, top: inside ? 10 : -10 }, false);
		});
	}

	onMount(() => {
		build();
		document.fonts?.ready.then(() => plot?.redraw(false, true));
	});

	$effect(() => {
		// Track the view and the theme; rebuild or update in place.
		void view;
		void theme.version;
		if (plot) update();
		bindCursor(view.link);
	});

	onDestroy(() => {
		if (view.link) cursors.clear(view.link, paneId);
		unsubscribeCursor?.();
		destroyPlot();
	});

	function readoutText(v: number | null): string {
		return v === null ? '-' : formatFixed(v, 4);
	}

	const xReadout = $derived(cursorX === null ? null : formatFixed(cursorX, 4));

	function legendLabel(s: Series, styleName: string) {
		return `${s.name}, ${s.role} line, ${styleName}`;
	}
</script>

<div class="spectrum" class:live data-testid="view-spectrum">
	<div class="head">
		{#if showTitle}
			<h3 class="title">{view.title}</h3>
		{/if}
		<div class="xread num" aria-live="off" class:active={xReadout !== null} class:linked={xReadout !== null && !cursorIsLocal}>
			{#if view.link}
				<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true">
					<path d="M5 7l2-2M4.2 8.8a1.8 1.8 0 01-2.5-2.5l1.4-1.4M7.8 3.2a1.8 1.8 0 012.5 2.5L8.9 7.1" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" />
				</svg>
			{/if}
			<span class="xlabel">{view.x.label}</span>
			<span class="xval">{xReadout ?? '-'}</span>
			<span class="xunit">{view.x.unit ?? ''}</span>
		</div>
	</div>
	<ul class="legend" aria-label="Legend">
		{#each styled as { series, style, colorVar }, i (i)}
			<li title={legendLabel(series, style.styleName)}>
				<svg class="swatch" viewBox="0 0 26 8" width="26" height="8" aria-hidden="true">
					<line
						x1="1"
						y1="4"
						x2="25"
						y2="4"
						stroke="var({colorVar})"
						stroke-width={style.width + 0.4}
						stroke-dasharray={dashArray(series.role)}
						stroke-linecap="butt"
					/>
				</svg>
				<span class="name">{series.name}</span>
				<span class="style">{style.styleName}</span>
				<span class="val num">{readoutText(readouts[i])}</span>
			</li>
		{/each}
	</ul>
	<div class="plot" bind:this={plotEl} style:min-height="{minHeight}px"></div>
</div>

<style>
	.spectrum {
		display: flex;
		flex-direction: column;
		height: 100%;
		min-height: 0;
		min-width: 0;
	}

	.head {
		display: flex;
		align-items: baseline;
		gap: var(--sp-3);
		padding: 0 var(--sp-1) var(--sp-1);
		min-height: 22px;
	}

	.title {
		flex: 1;
		min-width: 0;
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
		color: var(--text-1);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.xread {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		margin-left: auto;
		font-size: var(--text-xs);
		color: var(--text-4);
		white-space: nowrap;
	}

	.xread.active {
		color: var(--text-2);
	}

	.xread.linked .xval {
		color: var(--blue-1);
	}

	.xread svg {
		color: var(--text-3);
		align-self: center;
	}

	.xlabel {
		font-family: var(--font-ui);
	}

	.xval {
		min-width: 6ch;
		text-align: right;
		color: var(--text-1);
	}

	.xread:not(.active) .xval {
		color: var(--text-4);
	}

	.legend {
		display: flex;
		flex-wrap: wrap;
		gap: 2px var(--sp-4);
		margin: 0;
		padding: 0 var(--sp-1) var(--sp-2);
		list-style: none;
		font-size: var(--text-xs);
	}

	.legend li {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		min-width: 0;
	}

	.swatch {
		flex: none;
	}

	.name {
		color: var(--text-1);
		white-space: nowrap;
	}

	.style {
		color: var(--text-3);
		font-size: var(--text-2xs);
	}

	.val {
		min-width: 7.5ch;
		color: var(--text-2);
		text-align: right;
	}

	.plot {
		position: relative;
		flex: 1;
		min-height: 0;
		background: var(--bg-plot);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-md);
		overflow: hidden;
	}

	.plot :global(.uplot) {
		font-family: var(--font-mono);
		position: absolute;
		inset: 0;
	}

	.plot :global(.u-cursor-x) {
		border-right: 1px solid var(--plot-cursor);
	}

	.plot :global(.u-select) {
		background: var(--blue-weak);
		border-left: 1px solid var(--blue-line);
		border-right: 1px solid var(--blue-line);
	}

	.plot :global(.u-over) {
		cursor: crosshair;
	}
</style>
