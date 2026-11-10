<script lang="ts">
	import { copyCanvas, plotBackground } from './copyImage';
	/**
	 * x-y line view (spectra and anything else on two axes), drawn with uPlot.
	 *
	 * Log or linear axes come from the view. Each series' role sets its dash pattern and width;
	 * color is the second cue, and the legend names the line style in words. Panes with the same
	 * `link` share a cursor on x through the CursorHub, and a shape view of the same result with
	 * that `link` puts its playback time on the plot as a line that drags to seek. Live updates keep the axes still: the
	 * plot is rebuilt only when the structure changes, and the y range only moves when the data
	 * leaves it.
	 */
	import uPlot from 'uplot';
	import 'uplot/dist/uPlot.min.css';
	import { onDestroy, onMount, untrack } from 'svelte';
	import type { Series, SpectrumView, SpectrumHandle } from '$lib/protocol/types';
	import { cssVar, theme } from '$lib/theme/theme.svelte';
	import { alignSeries, valueAt } from './align';
	import { cursors, playheadGroup, playheads } from './cursor';
	import { formatFixed, formatTick } from './format';
	import { decadeStride, logRange, logTicks, mantissasFor } from './logTicks';
	import { drawBox, drawMarker, haloText, markerSvgPath, type Area, type BoxRow } from './paneDraw';
	import { markerIndices, minorSplits, panBy, TICK_SPACE, zoomAbout, type Corner, type GridMode, type LegendMode, type PaneMarks, type PaneNote, type SeriesOverride, type TickDensity, type ZoomChange } from './paneOptions';
	import { MARKER_WORDS, seriesLooks, type MarkerShape } from './roles';
	import { stepsPath } from './steps';

	interface Props {
		view: SpectrumView;
		/** Hold the y range steady across updates (live data). */
		live?: boolean;
		/**
		 * A time history whose x moves with the data (station time): each frame is drawn over its
		 * own x range, and the range of the frame before is not kept.
		 */
		followX?: boolean;
		/** Minimum plot height in px. */
		minHeight?: number;
		/** Show the view's title above the legend. */
		showTitle?: boolean;
		/** A y range the operator fixed; null lets the pane range itself. */
		yFixed?: [number, number] | null;
		/** The result the view belongs to, which a playhead is scoped to. */
		scope?: string;
		/** A click on the plot (not a drag), with the x it landed on: a cursor the caller places. */
		onPick?: (x: number) => void;
		/** A draggable region released at new edges: its index in `view.regions`, and the edges in x. */
		onspan?: (region: number, x0: number, x1: number) => void;
		/** The x range on screen, each time it changes: a zoom the caller reads as a band. */
		onRange?: (low: number, high: number) => void;
		/** An x to mark with a line, such as the cursor a fit is at; null for none. */
		marker?: number | null;
		/** Thin lines at these x values: a reading's crossings beside the marker. */
		guides?: number[];
		/** The marker can be dragged: `x` where it is, `fraction` of the way down the plot. `done` on release. */
		onMarkerDrag?: (x: number, fraction: number, moved: { x: boolean; y: boolean }, done: boolean) => void;
		/** A handle let go at `decibels`. */
		onhandle?: (handle: SpectrumHandle, decibels: number) => void;

		/** Grid lines at the ticks, with minor lines between them as well, or none. */
		grid?: GridMode;
		/** The legend above the plot, drawn inside it at `legendCorner`, or none. */
		legend?: LegendMode;
		legendCorner?: Corner;
		/** The cursor's readout: the x it is at, and each line's value there. */
		readout?: boolean;
		/** The Copy Image button above the plot. */
		copyButton?: boolean;
		/** Each series' look in place of its role's, by index into `view.series`. */
		styles?: (SeriesOverride | undefined)[];
		/** A legend entry pressed, by series index; with it the entries are buttons. */
		onlegend?: (index: number) => void;
		/** An x range the caller holds; null lets the pane range itself. */
		xFixed?: [number, number] | null;
		/** Box zoom on y as well as x. */
		zoomY?: boolean;
		/**
		 * Zooms (a box, the wheel, a double-click to reset) are reported and not applied: the
		 * caller holds the ranges through `xFixed` and `yFixed`.
		 */
		onZoom?: (change: ZoomChange) => void;
		/** The wheel zooms x about the pointer; sideways, or with Shift, it pans. */
		wheelZoom?: boolean;
		/** Keep drawing the view as it was when this turned on. */
		frozen?: boolean;
		/** Cursor lines, marked points and the lines of their readout. */
		marks?: PaneMarks | null;
		/** Notes over the plot. */
		notes?: PaneNote[];
		/** A note dragged to a new place, at fractions of the plot area. */
		onnote?: (id: string, fx: number, fy: number) => void;
		/** How close the ticks sit on each axis. */
		xTicks?: TickDensity;
		yTicks?: TickDensity;
		/** Axis labels in place of the view's; null keeps the view's. */
		xLabel?: string | null;
		yLabel?: string | null;
		/** Draw a decimated series from its minimum to its maximum, as a band. */
		envelope?: boolean;
	}

	let {
		view: viewIn,
		live = false,
		followX = false,
		minHeight = 180,
		showTitle = true,
		yFixed = null,
		scope,
		onPick,
		onRange,
		marker = null,
		guides = [],
		onMarkerDrag,
		onhandle,
		onspan,
		grid = 'major',
		legend = 'outside',
		legendCorner = 'top-right',
		readout = true,
		copyButton = true,
		styles = [],
		onlegend,
		xFixed = null,
		zoomY = false,
		onZoom,
		wheelZoom = false,
		frozen = false,
		marks = null,
		notes = [],
		onnote,
		xTicks = 'normal',
		yTicks = 'normal',
		xLabel = null,
		yLabel = null,
		envelope = false
	}: Props = $props();

	/** The view as it was when the pane froze; null while live. */
	let frozenView = $state.raw<SpectrumView | null>(null);
	$effect.pre(() => {
		if (!frozen) frozenView = null;
		else if (!untrack(() => frozenView)) frozenView = untrack(() => viewIn);
	});
	const view = $derived(frozenView ?? viewIn);

	/** Zooms are taken in by the pane itself and not left to uPlot: y zooms, wheel zooms, or the caller holds the ranges. */
	const ownZoom = $derived(zoomY || wheelZoom || onZoom !== undefined);
	/** A y range zoomed to on the pane, when the pane holds its own zoom. */
	let yZoom: [number, number] | null = null;

	const paneId = `sp-${Math.random().toString(36).slice(2, 9)}`;

	let plotEl: HTMLDivElement;
	let plot: uPlot | null = null;
	let structureKey = '';
	/** The title of the plot last built, whose x range a rebuild of the same plot keeps. */
	let keptTitle: string | null = null;
	let yRange: [number, number] | null = null;
	let resizeObs: ResizeObserver | null = null;
	let unsubscribeCursor: (() => void) | null = null;
	let subscribedLink: string | undefined;

	/** The playback line, its time, and the group it is shared in. */
	let playLine: HTMLDivElement | null = null;
	let playX: number | null = null;
	let dragging = false;
	let unsubscribePlayhead: (() => void) | null = null;
	let playGroup: string | undefined;

	let cursorX = $state<number | null>(null);
	let cursorIsLocal = $state(false);

	const logX = $derived(view.x.scale === 'log');
	const logY = $derived(view.y.scale === 'log');

	/**
	 * Per-series style, with measured series stepping through the blue family and followers dashed;
	 * then the caller's looks, which keep a dash or a marker beside any colour they give.
	 */
	const styled = $derived.by(() => {
		const looks = seriesLooks(view.series);
		return view.series.map((s, i) => {
			const look = looks[i];
			const o = styles[i];
			const style = o ? { ...look.style, width: o.width ?? look.style.width, dash: o.dash ? [...o.dash] : look.style.dash, styleName: o.dashName ?? look.style.styleName } : look.style;
			const markerShape: MarkerShape | null = o?.marker ?? null;
			const styleWords = markerShape ? `${style.styleName}, ${MARKER_WORDS[markerShape]}` : style.styleName;
			return { series: s, index: i, ...look, style, color: o?.color ?? look.color, markerShape, styleWords, shown: o?.visible !== false };
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

	/** What, beyond the view, changes how the plot is built: a change rebuilds it. */
	const optionsKey = $derived(JSON.stringify([styles, grid, xTicks, yTicks, xLabel, yLabel, envelope, zoomY, ownZoom]));

	function keyOf(v: SpectrumView): string {
		const xs = v.series.map((s) => [s.name, s.role, s.shape ?? 'line', s.x.length, s.x[0], s.x[s.x.length - 1]]);
		return JSON.stringify([v.x, v.y, xs, v.regions ?? [], theme.version, optionsKey]);
	}

	/** The columns of the series drawn, which the y range is taken from. */
	function shownCols(cols: ArrayLike<number | null>[]): ArrayLike<number | null>[] {
		return cols.filter((_, i) => styled[i]?.shown !== false);
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
		if (yFixed) return yFixed;
		if (yZoom) return yZoom;
		const ext = finiteExtent(shownCols(cols), logY) ?? (logY ? [1e-4, 1] : [0, 1]);
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

	/** Pixels each decade gets on this axis. */
	function pxPerDecade(u: uPlot, axis: 0 | 1, min: number, max: number): number {
		const px = (axis === 0 ? u.bbox.width : u.bbox.height) / devicePixelRatio;
		const decades = Math.max(0.3, Math.log10(Math.max(max, 1e-300) / Math.max(min, 1e-300)));
		return px / decades;
	}

	/** Ticks for a log axis, as dense as its pixels (and the tick density asked for) allow and never denser. */
	function logSplits(u: uPlot, axis: 0 | 1, min: number, max: number): number[] {
		const ppd = pxPerDecade(u, axis, min, max) / TICK_SPACE[axis === 0 ? xTicks : yTicks];
		return logTicks(min, max, mantissasFor(ppd), decadeStride(ppd));
	}

	let measureCtx: CanvasRenderingContext2D | null = null;
	function textWidth(text: string, f: string): number {
		measureCtx ??= document.createElement('canvas').getContext('2d');
		if (!measureCtx) return text.length * 7;
		measureCtx.font = f;
		return measureCtx.measureText(text).width;
	}

	let copiedImage = $state<'copied' | 'saved' | null>(null);
	/** The plot, axes and curves, as an image on the clipboard. */
	async function copyImage() {
		if (!plot) return;
		copiedImage = await copyCanvas(plot.ctx.canvas, plotBackground(plotEl), undefined, view.title);
		if (copiedImage) setTimeout(() => (copiedImage = null), 1500);
	}

	/** The marker line, placed at `marker`. */
	let markerLine: HTMLDivElement | null = null;
	/** The view's regions, shaded behind the curves, one element each. */
	let regionEls: HTMLDivElement[] = [];
	/** Where a press on the plot began, to tell a click from a drag. */
	let pressedAt: number | null = null;

	function build() {
		// the same plot rebuilt for a new series (a fit's synthesis arriving) keeps the x range the
		// person zoomed to: the view belongs to them
		const kept = !followX && plot && plot.scales.x.min != null && plot.scales.x.max != null && keptTitle === view.title
			? { min: plot.scales.x.min, max: plot.scales.x.max }
			: null;
		keptTitle = view.title;
		destroyPlot();
		const data = alignSeries(view.series, logX, logY);
		const [xs, ...cols] = data;
		yRange = nextYRange(cols);
		const axisColor = cssVar('--plot-axis');
		const gridColor = cssVar('--plot-grid');
		const tickFont = font(11);
		// Whole pixels only: uPlot scales the integer part of the size for HiDPI.
		const labelFont = font(12, false);

		const ordered = drawOrder;
		const plotData = [xs, ...ordered.map((o) => cols[o.index])] as uPlot.AlignedData;
		const gridShown = grid !== 'none';

		const opts: uPlot.Options = {
			width: Math.max(100, plotEl.clientWidth),
			height: Math.max(minHeight, plotEl.clientHeight),
			pxAlign: false,
			legend: { show: false },
			focus: { alpha: 0.25 },
			padding: [10, 24, 0, 0],
			cursor: {
				y: false,
				points: { show: false },
				drag: ownZoom ? { x: true, y: zoomY, uni: zoomY ? 24 : undefined, setScale: false } : { x: true, y: false, setScale: true },
				focus: { prox: -1 },
				// a double-click resets the zoom the pane holds, or tells the caller to
				bind: ownZoom ? { dblclick: () => () => (resetZoom(), null) } : undefined
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
					label: axisTitle(xLabel ?? view.x.label, view.x.unit),
					labelSize: 22,
					labelGap: 2,
					size: 34,
					gap: 4,
					space: (logX ? 36 : 56) * TICK_SPACE[xTicks],
					grid: { show: gridShown, stroke: gridColor, width: 1 },
					ticks: { stroke: gridColor, width: 1, size: 4 },
					splits: logX ? (u, _i, min, max) => logSplits(u, 0, min, max) : undefined,
					filter: logX ? (_u, splits) => splits : undefined,
					values: (_u, splits) => splits.map((v) => (v == null ? '' : formatTick(v)))
				},
				{
					scale: 'y',
					stroke: axisColor,
					font: tickFont,
					labelFont,
					label: axisTitle(yLabel ?? view.y.label, view.y.unit),
					labelSize: 20,
					labelGap: 0,
					gap: 6,
					space: 28 * TICK_SPACE[yTicks],
					grid: { show: gridShown, stroke: gridColor, width: 1 },
					ticks: { stroke: gridColor, width: 1, size: 4 },
					splits: logY ? (u, _i, min, max) => logSplits(u, 1, min, max) : undefined,
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
				...ordered.map(({ series, index, style, colorVar, color, shown }) => ({
					label: series.name,
					show: shown,
					stroke: color ?? cssVar(colorVar),
					width: style.width,
					dash: style.dash,
					spanGaps: true,
					points: { show: false },
					paths: series.shape === 'steps' && series.edges ? stepsPath(() => view.series[index], logX, logY) : undefined
				}))
			],
			hooks: {
				drawAxes: [drawMinorGrid, drawEnvelopes],
				draw: [placePlayLine, placeMarker, placeGuides, placeRegions, placeHandles, drawSeriesMarkers, drawMarks],
				setScale: [
					(u, key) => {
						if (key === 'x' && onRange && u.scales.x.min != null && u.scales.x.max != null) onRange(u.scales.x.min, u.scales.x.max);
					}
				],
				setSelect: [takeSelection],
				setSize: [measureArea],
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
		playLine = document.createElement('div');
		playLine.className = 'playline';
		playLine.title = 'Drag to move playback';
		// the line's own drag, not the plot's zoom
		playLine.addEventListener('mousedown', (e) => e.stopPropagation());
		playLine.addEventListener('pointerdown', startDrag);
		plot.over.appendChild(playLine);
		placePlayLine();
		markerLine = document.createElement('div');
		markerLine.className = 'marker';
		if (onMarkerDrag) {
			const grip = document.createElement('div');
			grip.className = 'marker-grip';
			grip.title = 'Drag across to move the cursor, up and down to set the damping';
			// the line's own drag, not the plot's zoom
			grip.addEventListener('mousedown', (e) => e.stopPropagation());
			grip.addEventListener('click', (e) => e.stopPropagation());
			grip.addEventListener('pointerdown', startMarkerDrag);
			markerLine.appendChild(grip);
		}
		plot.over.appendChild(markerLine);
		placeMarker();
		guideEls = [];
		placeGuides();
		regionEls = (view.regions ?? []).map((r, k) => {
			const el = document.createElement('div');
			el.className = `region ${r.kind}`;
			if (r.label) el.title = r.label;
			if (r.drag && onspan) {
				// dragged in the top layer, so the press is the span's and not a zoom
				el.classList.add('drag');
				el.addEventListener('mousedown', (e) => e.stopPropagation());
				el.addEventListener('pointerdown', (e) => startSpan(e, k, el));
				plot!.over.appendChild(el);
			} else plot!.under.appendChild(el);
			return el;
		});
		placeRegions();
		handleEls = [];
		placeHandles();
		plot.over.addEventListener('mousedown', (e) => (pressedAt = e.clientX));
		plot.over.addEventListener('click', (e) => {
			const moved = pressedAt === null ? 0 : Math.abs(e.clientX - pressedAt);
			pressedAt = null;
			if (!plot || !onPick || moved > 3) return;
			onPick(plot.posToVal(e.clientX - plot.over.getBoundingClientRect().left, 'x'));
		});
		if (wheelZoom) plot.over.addEventListener('wheel', onWheel, { passive: false });
		if (xFixed) plot.setScale('x', { min: xFixed[0], max: xFixed[1] });
		else if (kept) plot.setScale('x', kept);
		appliedX = xFixed;
		measureArea();
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
		// Without its scale reset, setData only swaps the data and draws nothing; setting the y range
		// (held for live data, or moved) is what draws it. `redraw()` is not used: before the first
		// draw it would set the x range from a scale not yet computed.
		plot.setData(plotData, false);
		yRange = next;
		plot.setScale('y', { min: next[0], max: next[1] });
	}

	/** The plot's drawing area inside the pane, CSS pixels: where the legend, readouts and notes sit. */
	let area = $state<{ left: number; top: number; width: number; height: number } | null>(null);
	function measureArea() {
		if (!plot) return;
		const r = devicePixelRatio || 1;
		const b = plot.bbox;
		const next = { left: b.left / r, top: b.top / r, width: b.width / r, height: b.height / r };
		if (!area || area.left !== next.left || area.top !== next.top || area.width !== next.width || area.height !== next.height) area = next;
	}

	/** The x range last set from `xFixed`, so a release of it puts the data's own range back. */
	let appliedX: [number, number] | null = null;
	function applyXFixed() {
		if (!plot) return;
		if (xFixed) {
			if (!appliedX || appliedX[0] !== xFixed[0] || appliedX[1] !== xFixed[1]) plot.setScale('x', { min: xFixed[0], max: xFixed[1] });
		} else if (appliedX) {
			const xs = plot.data[0];
			if (xs.length) plot.setScale('x', { min: xs[0], max: xs[xs.length - 1] });
		}
		appliedX = xFixed;
	}

	/** A zoom made on the pane: to the caller when it holds the ranges, else applied here. */
	function applyZoom(change: ZoomChange) {
		if (onZoom) return onZoom(change);
		if (!plot) return;
		if (change.x) plot.setScale('x', { min: change.x[0], max: change.x[1] });
		else if (change.x === null) {
			const xs = plot.data[0];
			if (xs.length) plot.setScale('x', { min: xs[0], max: xs[xs.length - 1] });
		}
		if (change.y !== undefined) {
			yZoom = change.y;
			if (!yZoom) yRange = null;
			const [, ...cols] = alignSeries(view.series, logX, logY);
			const next = nextYRange(cols);
			yRange = next;
			plot.setScale('y', { min: next[0], max: next[1] });
		}
	}

	function resetZoom() {
		applyZoom({ x: null, y: zoomY ? null : undefined });
	}

	/** A box dragged on the plot: its x span, and its y span where y zooms too. */
	function takeSelection(u: uPlot) {
		if (!ownZoom) return;
		// a copy: clearing the box below empties uPlot's own
		const sel = { ...u.select };
		const w = u.over.clientWidth;
		const h = u.over.clientHeight;
		const dx = sel.width > 1 && sel.width < w - 1;
		const dy = zoomY && sel.height > 1 && sel.height < h - 1;
		if (!dx && !dy) return;
		u.setSelect({ left: 0, top: 0, width: 0, height: 0 }, false);
		const x: [number, number] | undefined = dx ? [u.posToVal(sel.left, 'x'), u.posToVal(sel.left + sel.width, 'x')] : undefined;
		const y: [number, number] | undefined = dy ? [u.posToVal(sel.top + sel.height, 'y'), u.posToVal(sel.top, 'y')] : undefined;
		applyZoom({ x, y });
	}

	/** The wheel: zoom x about the pointer, or pan it when the wheel turns sideways or with Shift. */
	function onWheel(e: WheelEvent) {
		if (!plot) return;
		const { min, max } = plot.scales.x;
		if (min == null || max == null) return;
		e.preventDefault();
		const scale = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? plot.over.clientWidth : 1;
		const sideways = e.shiftKey || Math.abs(e.deltaX) > Math.abs(e.deltaY);
		const width = Math.max(1, plot.over.clientWidth);
		if (sideways) {
			const d = (e.shiftKey && e.deltaX === 0 ? e.deltaY : e.deltaX) * scale;
			applyZoom({ x: panBy(min, max, d / width, logX) });
			return;
		}
		const at = plot.posToVal(e.clientX - plot.over.getBoundingClientRect().left, 'x');
		const factor = Math.min(4, Math.max(0.25, Math.exp(e.deltaY * scale * 0.002)));
		applyZoom({ x: zoomAbout(min, max, at, factor, logX) });
	}

	/** Lines between the major grid lines, fainter, when the grid has its minor lines. */
	function drawMinorGrid(u: uPlot) {
		if (grid !== 'minor') return;
		const ctx = u.ctx;
		const r = devicePixelRatio || 1;
		const { left, top, width, height } = u.bbox;
		ctx.save();
		ctx.strokeStyle = cssVar('--plot-grid');
		ctx.globalAlpha = 0.55;
		ctx.lineWidth = r;
		ctx.beginPath();
		([0, 1] as const).forEach((k) => {
			const key = k === 0 ? 'x' : 'y';
			const { min, max } = u.scales[key];
			if (min == null || max == null) return;
			const majors = (u.axes[k] as { _splits?: number[] })._splits ?? [];
			for (const v of minorSplits(min, max, k === 0 ? logX : logY, majors)) {
				const p = u.valToPos(v, key, true);
				if (k === 0) {
					ctx.moveTo(p, top);
					ctx.lineTo(p, top + height);
				} else {
					ctx.moveTo(left, p);
					ctx.lineTo(left + width, p);
				}
			}
		});
		ctx.stroke();
		ctx.restore();
	}

	/** A decimated series' band from its minimum to its maximum, under the lines. */
	function drawEnvelopes(u: uPlot) {
		if (!envelope) return;
		const ctx = u.ctx;
		const { left, top, width, height } = u.bbox;
		ctx.save();
		ctx.beginPath();
		ctx.rect(left, top, width, height);
		ctx.clip();
		for (const s of styled) {
			const lows = s.series.y_min;
			if (!s.shown || !lows) continue;
			const n = Math.min(s.series.x.length, s.series.y.length, lows.length);
			const keep = (i: number) => Number.isFinite(s.series.x[i]) && (!logX || s.series.x[i] > 0) && Number.isFinite(s.series.y[i]) && Number.isFinite(lows[i]) && (!logY || lows[i] > 0);
			const idx: number[] = [];
			for (let i = 0; i < n; i++) if (keep(i)) idx.push(i);
			if (idx.length < 2) continue;
			ctx.beginPath();
			idx.forEach((i, k) => {
				const x = u.valToPos(s.series.x[i], 'x', true);
				const y = u.valToPos(s.series.y[i], 'y', true);
				if (k === 0) ctx.moveTo(x, y);
				else ctx.lineTo(x, y);
			});
			for (let k = idx.length - 1; k >= 0; k--) ctx.lineTo(u.valToPos(s.series.x[idx[k]], 'x', true), u.valToPos(lows[idx[k]], 'y', true));
			ctx.closePath();
			ctx.globalAlpha = 0.3;
			ctx.fillStyle = s.color ?? cssVar(s.colorVar);
			ctx.fill();
			ctx.globalAlpha = 1;
		}
		ctx.restore();
	}

	/** Each series' marker, every few dozen pixels along it, staggered between series. */
	function drawSeriesMarkers(u: uPlot) {
		const r = devicePixelRatio || 1;
		const withMarkers = drawOrder.map((o, k) => ({ o, k })).filter(({ o }) => o.markerShape && o.shown);
		if (!withMarkers.length) return;
		const ctx = u.ctx;
		const { left, top, width, height } = u.bbox;
		const xs = u.data[0];
		ctx.save();
		ctx.beginPath();
		ctx.rect(left, top, width, height);
		ctx.clip();
		ctx.setLineDash([]);
		ctx.lineWidth = 1.3 * r;
		withMarkers.forEach(({ o, k }, n) => {
			const col = u.data[k + 1] as (number | null)[];
			ctx.strokeStyle = o.color ?? cssVar(o.colorVar);
			const at = markerIndices(col, (i) => u.valToPos(xs[i], 'x', true), 72 * r, (12 + n * 14) * r);
			for (const i of at) {
				const y = col[i];
				if (y === null || y === undefined) continue;
				drawMarker(ctx, o.markerShape as MarkerShape, u.valToPos(xs[i], 'x', true), u.valToPos(y, 'y', true), 3.6 * r);
			}
		});
		ctx.restore();
	}

	/** Cursor lines and marked points, on the canvas so an image of the plot carries them. */
	function drawMarks(u: uPlot) {
		if (!marks || (!marks.lines.length && !marks.points.length)) return;
		const ctx = u.ctx;
		const r = devicePixelRatio || 1;
		const { left, top, width, height } = u.bbox;
		const ink = cssVar('--text-1');
		const halo = cssVar('--bg-plot');
		ctx.save();
		ctx.beginPath();
		ctx.rect(left, top, width, height);
		ctx.clip();
		ctx.font = `${Math.round(11 * r)}px ${cssVar('--font-mono')}`;
		ctx.textBaseline = 'top';
		ctx.strokeStyle = ink;
		for (const line of marks.lines) {
			const x = u.valToPos(line.x, 'x', true);
			if (!(x >= left && x <= left + width)) continue;
			ctx.save();
			ctx.strokeStyle = ink;
			ctx.globalAlpha = 0.85;
			ctx.lineWidth = 1.2 * r;
			ctx.setLineDash(line.dash.map((d) => d * r));
			ctx.beginPath();
			ctx.moveTo(x, top);
			ctx.lineTo(x, top + height);
			ctx.stroke();
			ctx.restore();
			haloText(ctx, line.label, x + 4 * r, top + 4 * r, ink, halo, r);
		}
		ctx.textBaseline = 'bottom';
		for (const p of marks.points) {
			if (logY && !(p.y > 0)) continue;
			const x = u.valToPos(p.x, 'x', true);
			const y = u.valToPos(p.y, 'y', true);
			// a triangle pointing down at the point, its label above
			ctx.beginPath();
			ctx.moveTo(x, y - 2 * r);
			ctx.lineTo(x - 4.5 * r, y - 10 * r);
			ctx.lineTo(x + 4.5 * r, y - 10 * r);
			ctx.closePath();
			ctx.fillStyle = ink;
			ctx.fill();
			haloText(ctx, p.label, x + 6 * r, y - 8 * r, ink, halo, r);
		}
		ctx.restore();
	}

	function startMarkerDrag(e: PointerEvent) {
		e.preventDefault();
		e.stopPropagation();
		const fromX = e.clientX;
		const fromY = e.clientY;
		const grip = e.currentTarget as HTMLElement;
		grip.setPointerCapture?.(e.pointerId);
		const report = (ev: PointerEvent, done: boolean) => {
			if (!plot || !markerLine) return;
			const box = plot.over.getBoundingClientRect();
			const left = Math.min(box.width, Math.max(0, ev.clientX - box.left));
			const moved = { x: Math.abs(ev.clientX - fromX) > 3, y: Math.abs(ev.clientY - fromY) > 3 };
			if (moved.x) markerLine.style.left = `${left}px`;
			const fraction = Math.min(1, Math.max(0, (ev.clientY - box.top) / box.height));
			onMarkerDrag?.(plot.posToVal(left, 'x'), fraction, moved, done);
		};
		const move = (ev: PointerEvent) => report(ev, false);
		const stop = () => {
			grip.removeEventListener('pointermove', move);
			grip.removeEventListener('pointerup', up);
			grip.removeEventListener('pointercancel', cancel);
		};
		const up = (ev: PointerEvent) => {
			stop();
			report(ev, true);
		};
		const cancel = () => {
			stop();
			placeMarker();
			onMarkerDrag?.(marker ?? 0, 0, { x: false, y: false }, true);
		};
		grip.addEventListener('pointermove', move);
		grip.addEventListener('pointerup', up);
		grip.addEventListener('pointercancel', cancel);
	}

	let guideEls: HTMLDivElement[] = [];
	/** The thin lines at `guides`, in the plot's top layer. */
	function placeGuides() {
		if (!plot) return;
		while (guideEls.length > guides.length) guideEls.pop()!.remove();
		while (guideEls.length < guides.length) {
			const el = document.createElement('div');
			el.className = 'guide';
			plot.over.appendChild(el);
			guideEls.push(el);
		}
		guides.forEach((x, i) => {
			const left = plot!.valToPos(x, 'x');
			const inside = left >= 0 && left <= plot!.over.clientWidth;
			guideEls[i].style.display = inside ? '' : 'none';
			if (inside) guideEls[i].style.left = `${left}px`;
		});
	}

	$effect(() => {
		void guides;
		placeGuides();
	});

	let handleEls: HTMLDivElement[] = [];
	/** The series' value nearest `x`, where a handle sits. */
	function handleValue(h: SpectrumHandle): number | null {
		const series = view.series[h.series];
		if (!series?.x.length) return null;
		let at = 0;
		for (let k = 1; k < series.x.length; k++) {
			if (Math.abs(series.x[k] - h.x) < Math.abs(series.x[at] - h.x)) at = k;
		}
		return series.y[at];
	}
	/** Each band's handle on its edge, in the plot's top layer. */
	function placeHandles() {
		const handles = onhandle ? (view.handles ?? []) : [];
		if (!plot) return;
		while (handleEls.length > handles.length) handleEls.pop()!.remove();
		while (handleEls.length < handles.length) {
			const el = document.createElement('div');
			el.className = 'band-handle';
			const k = handleEls.length;
			// its own drag, not the plot's zoom
			el.addEventListener('mousedown', (e) => e.stopPropagation());
			el.addEventListener('click', (e) => e.stopPropagation());
			el.addEventListener('pointerdown', (e) => startHandle(e, k));
			plot.over.appendChild(el);
			handleEls.push(el);
		}
		handles.forEach((h, k) => {
			const value = handleValue(h);
			const left = plot!.valToPos(h.x, 'x');
			const top = value === null ? -1 : plot!.valToPos(value, 'y');
			const el = handleEls[k];
			const inside = left >= 0 && left <= plot!.over.clientWidth && top >= 0 && top <= plot!.over.clientHeight;
			el.style.display = inside ? '' : 'none';
			if (!inside) return;
			el.style.left = `${left}px`;
			el.style.top = `${top}px`;
			el.title = `${h.label}: ${h.decibels >= 0 ? '+' : ''}${h.decibels.toFixed(2)} dB. Drag up or down`;
		});
	}
	function startHandle(e: PointerEvent, k: number) {
		const h = view.handles?.[k];
		const start = h && handleValue(h);
		if (!h || !start || !plot) return;
		e.preventDefault();
		e.stopPropagation();
		const el = e.currentTarget as HTMLDivElement;
		el.setPointerCapture?.(e.pointerId);
		const decibels = (ev: PointerEvent) => {
			const box = plot!.over.getBoundingClientRect();
			const y = Math.min(box.height, Math.max(0, ev.clientY - box.top));
			el.style.top = `${y}px`;
			return h.decibels + 10 * Math.log10(plot!.posToVal(y, 'y') / start);
		};
		const move = (ev: PointerEvent) => {
			el.title = `${h.label}: ${decibels(ev).toFixed(2)} dB`;
		};
		const stop = () => {
			el.removeEventListener('pointermove', move);
			el.removeEventListener('pointerup', up);
			el.removeEventListener('pointercancel', cancel);
		};
		const up = (ev: PointerEvent) => {
			stop();
			if (Math.abs(ev.clientY - e.clientY) > 3) onhandle?.(h, decibels(ev));
			else placeHandles();
		};
		const cancel = () => {
			stop();
			placeHandles();
		};
		el.addEventListener('pointermove', move);
		el.addEventListener('pointerup', up);
		el.addEventListener('pointercancel', cancel);
	}

	/** Put the marker line at `marker`, or hide it when there is none or it is off the plot. */
	function placeMarker() {
		if (!plot || !markerLine) return;
		const left = marker === null ? -1 : plot.valToPos(marker, 'x');
		const inside = left >= 0 && left <= plot.over.clientWidth;
		markerLine.style.display = inside ? '' : 'none';
		if (inside) markerLine.style.left = `${left}px`;
	}

	$effect(() => {
		void marker;
		placeMarker();
	});

	/** How near an edge, in pixels, a press takes that edge rather than the whole span. */
	const EDGE_PX = 6;

	/**
	 * A draggable region pressed: the middle slides it, an edge (when the span has two) moves that
	 * edge. It follows the pointer and, on release, its new edges in x go to `onspan`.
	 */
	function startSpan(e: PointerEvent, k: number, el: HTMLDivElement) {
		const region = view.regions?.[k];
		if (!plot || !region?.drag || e.button !== 0) return;
		e.preventDefault();
		e.stopPropagation();
		const a = plot.valToPos(region.x0, 'x');
		const b = plot.valToPos(region.x1, 'x');
		const at = e.clientX - plot.over.getBoundingClientRect().left;
		const two = !!region.drag.high;
		const part = two && Math.abs(at - a) <= EDGE_PX ? 'low' : two && Math.abs(at - b) <= EDGE_PX ? 'high' : 'move';
		const x = e.clientX;
		let left = a;
		let right = b;
		const move = (m: PointerEvent) => {
			const d = m.clientX - x;
			if (part === 'move') [left, right] = [a + d, b + d];
			else if (part === 'low') left = Math.min(a + d, right - 2);
			else right = Math.max(b + d, left + 2);
			el.style.left = `${left}px`;
			el.style.width = `${right - left}px`;
		};
		const up = () => {
			window.removeEventListener('pointermove', move);
			window.removeEventListener('pointerup', up);
			if (!plot || (left === a && right === b)) return placeRegions();
			onspan?.(k, plot.posToVal(left, 'x'), plot.posToVal(right, 'x'));
		};
		window.addEventListener('pointermove', move);
		window.addEventListener('pointerup', up);
	}

	/** Shade each region over the part of it on the plot. */
	function placeRegions() {
		if (!plot) return;
		const width = plot.under.clientWidth;
		(view.regions ?? []).forEach((r, i) => {
			const el = regionEls[i];
			if (!el) return;
			const a = Math.max(0, Math.min(width, plot!.valToPos(r.x0, 'x')));
			const b = Math.max(0, Math.min(width, plot!.valToPos(r.x1, 'x')));
			el.style.display = b > a ? '' : 'none';
			el.style.left = `${a}px`;
			el.style.width = `${b - a}px`;
		});
	}

	/** Put the playback line at its time, or hide it when there is none or it is off the plot. */
	function placePlayLine() {
		if (!plot || !playLine) return;
		const left = playX === null ? -1 : plot.valToPos(playX, 'x');
		const inside = left >= 0 && left <= plot.over.clientWidth;
		playLine.style.display = inside ? '' : 'none';
		if (inside) playLine.style.left = `${left}px`;
	}

	function startDrag(e: PointerEvent) {
		if (!plot || !playGroup) return;
		const line = e.currentTarget as HTMLDivElement;
		line.setPointerCapture(e.pointerId);
		dragging = true;
		const move = (ev: PointerEvent) => {
			if (!plot || !playGroup) return;
			const { min, max } = plot.scales.x;
			const x = plot.posToVal(ev.clientX - plot.over.getBoundingClientRect().left, 'x');
			playX = Math.min(Math.max(x, min ?? x), max ?? x);
			placePlayLine();
			playheads.set(playGroup, playX, paneId);
		};
		const end = () => {
			dragging = false;
			line.removeEventListener('pointermove', move);
			line.removeEventListener('pointerup', end);
			line.removeEventListener('pointercancel', end);
		};
		line.addEventListener('pointermove', move);
		line.addEventListener('pointerup', end);
		line.addEventListener('pointercancel', end);
	}

	function bindPlayhead(group: string | undefined) {
		if (group === playGroup) return;
		unsubscribePlayhead?.();
		unsubscribePlayhead = null;
		playGroup = group;
		if (!group) return;
		unsubscribePlayhead = playheads.subscribe(group, (state) => {
			// while dragging the line stays under the pointer, not on the sample playback snapped to
			if (dragging) return;
			playX = state.x;
			placePlayLine();
		});
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

	/** The y range last held by the caller, so its release ranges y on the data again. */
	let heldY: [number, number] | null = null;

	$effect(() => {
		// Track the view, the theme and the look; rebuild or update in place.
		void view;
		void theme.version;
		void yFixed;
		void optionsKey;
		// a caller that holds the zoom lets go of y: range on the data, not on the range let go of
		if (onZoom && heldY && !yFixed) yRange = null;
		heldY = yFixed;
		if (plot) update();
		bindCursor(view.link);
		bindPlayhead(view.link ? playheadGroup(view.link, scope) : undefined);
	});

	$effect(() => {
		void xFixed;
		untrack(applyXFixed);
	});

	const marksKey = $derived(marks ? JSON.stringify(marks) : '');
	$effect(() => {
		void marksKey;
		untrack(() => plot?.redraw(false, false));
	});

	/** The x and y ranges on screen, or null before the plot is drawn. */
	export function ranges(): { x: [number, number] | null; y: [number, number] | null } {
		const get = (key: 'x' | 'y'): [number, number] | null => {
			const sc = plot?.scales[key];
			return sc && sc.min != null && sc.max != null ? [sc.min, sc.max] : null;
		};
		return { x: get('x'), y: get('y') };
	}

	/**
	 * The plot as an image: the axes and curves with the marks drawn on them, and the legend, the
	 * marks' readout and the notes drawn over it as they sit on screen (an outside legend is
	 * drawn inside, in its corner). Null before the plot is drawn.
	 */
	export function imageCanvas(): HTMLCanvasElement | null {
		if (!plot) return null;
		const src = plot.ctx.canvas;
		const out = document.createElement('canvas');
		out.width = src.width;
		out.height = src.height;
		const ctx = out.getContext('2d');
		if (!ctx) return null;
		ctx.fillStyle = plotBackground(plotEl);
		ctx.fillRect(0, 0, out.width, out.height);
		ctx.drawImage(src, 0, 0);
		const r = devicePixelRatio || 1;
		const box: Area = { left: plot.bbox.left, top: plot.bbox.top, width: plot.bbox.width, height: plot.bbox.height };
		const colors = { ink: cssVar('--text-1'), muted: cssVar('--text-2'), fill: plotBackground(plotEl), edge: cssVar('--line-2') };
		const font = `${Math.round(11 * r)}px ${cssVar('--font-ui')}`;
		if (legend !== 'off') {
			const rows: BoxRow[] = legendEntries
				.filter((e) => e.shown)
				.map((e) => ({ text: e.series.name, swatch: { color: e.color ?? cssVar(e.colorVar), dash: e.style.dash, width: e.style.width, marker: e.markerShape } }));
			drawBox(ctx, rows, legendCorner, box, colors, font, r);
		}
		if (marks?.box.length) drawBox(ctx, marks.box.map((text) => ({ text })), marksCorner, box, colors, `${Math.round(11 * r)}px ${cssVar('--font-mono')}`, r);
		for (const n of notes) drawBox(ctx, [{ text: n.text }], { x: box.left + n.fx * box.width, y: box.top + n.fy * box.height }, box, colors, font, r);
		return out;
	}

	onDestroy(() => {
		if (view.link) cursors.clear(view.link, paneId);
		unsubscribeCursor?.();
		unsubscribePlayhead?.();
		destroyPlot();
	});

	function readoutText(v: number | null): string {
		return v === null ? '-' : formatFixed(v, 4);
	}

	const xReadout = $derived(cursorX === null ? null : formatFixed(cursorX, 4));

	/** Pointing at a legend entry brings its line forward and dims the others: a cue besides color. */
	function focusSeries(index: number | null) {
		if (!plot) return;
		const at = index === null ? null : drawOrder.findIndex((o) => o.index === index) + 1;
		plot.setSeries(at && at > 0 ? at : null, { focus: true });
	}

	function legendLabel(s: Series, styleName: string, shown = true) {
		return `${s.name}, ${s.role} line, ${styleName}${shown ? '' : ', hidden'}`;
	}

	/** The series the legend lists: every one but a follower, which reads with its measurement. */
	const legendEntries = $derived(styled.filter((e) => !e.follows));

	/** The marks' readout sits in a top corner the legend does not take. */
	const marksCorner = $derived<Corner>(legend === 'inside' && legendCorner === 'top-left' ? 'top-right' : 'top-left');

	const showHead = $derived(showTitle || copyButton || (legend === 'outside' && readout));

	/** A note being dragged: where it is, as fractions of the plot area. */
	let dragNote = $state<{ id: string; fx: number; fy: number } | null>(null);
	function startNote(e: PointerEvent, n: PaneNote) {
		if (!onnote || !area || e.button !== 0) return;
		e.preventDefault();
		e.stopPropagation();
		const el = e.currentTarget as HTMLElement;
		el.setPointerCapture?.(e.pointerId);
		const box = area;
		const x0 = e.clientX;
		const y0 = e.clientY;
		const at = (ev: PointerEvent) => ({
			id: n.id,
			fx: Math.min(1, Math.max(0, n.fx + (ev.clientX - x0) / Math.max(1, box.width))),
			fy: Math.min(1, Math.max(0, n.fy + (ev.clientY - y0) / Math.max(1, box.height)))
		});
		const move = (ev: PointerEvent) => (dragNote = at(ev));
		const up = (ev: PointerEvent) => {
			el.removeEventListener('pointermove', move);
			el.removeEventListener('pointerup', up);
			el.removeEventListener('pointercancel', cancel);
			const end = at(ev);
			dragNote = null;
			if (Math.abs(ev.clientX - x0) > 2 || Math.abs(ev.clientY - y0) > 2) onnote?.(n.id, end.fx, end.fy);
		};
		const cancel = () => {
			el.removeEventListener('pointermove', move);
			el.removeEventListener('pointerup', up);
			el.removeEventListener('pointercancel', cancel);
			dragNote = null;
		};
		el.addEventListener('pointermove', move);
		el.addEventListener('pointerup', up);
		el.addEventListener('pointercancel', cancel);
	}
</script>

{#snippet swatch(e: (typeof styled)[number])}
	<svg class="swatch" viewBox="0 0 26 8" width="26" height="8" aria-hidden="true">
		<line x1="1" y1="4" x2="25" y2="4" stroke={e.color ?? `var(${e.colorVar})`} stroke-width={e.style.width + 0.4} stroke-dasharray={e.style.dash.length ? e.style.dash.join(' ') : 'none'} stroke-linecap="butt" />
		{#if e.markerShape}
			<path d={markerSvgPath(e.markerShape, 13, 4, 3)} fill="none" stroke={e.color ?? `var(${e.colorVar})`} stroke-width="1.2" />
		{/if}
	</svg>
{/snippet}

{#snippet entry(e: (typeof styled)[number])}
	{@render swatch(e)}
	<span class="name">{e.series.name}</span>
	<span class="style">{e.styleWords}</span>
	{#if readout}<span class="val num">{readoutText(readouts[e.index])}</span>{/if}
{/snippet}

<div class="spectrum" class:live data-testid="view-spectrum">
	{#if showHead}
	<div class="head">
		{#if showTitle}
			<h3 class="title">{view.title}</h3>
		{/if}
		{#if legend === 'outside' && readout}
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
		{/if}
		{#if copyButton}
		<button class="btn ghost sm copyimg" data-testid="spectrum-copy-image" onclick={copyImage} title="Copy the plot as an image">{copiedImage === 'copied' ? 'Copied' : copiedImage === 'saved' ? 'Saved as PNG' : 'Copy Image'}</button>
		{/if}
	</div>
	{/if}
	{#if legend === 'outside'}
	<ul class="legend" aria-label="Legend">
		{#each legendEntries as e (e.index)}
			<li class:off={!e.shown} title={legendLabel(e.series, e.styleWords, e.shown)} onmouseenter={() => focusSeries(e.index)} onmouseleave={() => focusSeries(null)}>
				{#if onlegend}
					<button class="entry" aria-pressed={e.shown} onclick={() => onlegend?.(e.index)}>{@render entry(e)}</button>
				{:else}
					{@render entry(e)}
				{/if}
			</li>
		{/each}
	</ul>
	{/if}
	<div class="plot" bind:this={plotEl} style:min-height="{minHeight}px">
		{#if area}
			<div class="area" style:left="{area.left}px" style:top="{area.top}px" style:width="{area.width}px" style:height="{area.height}px">
				{#if legend === 'inside' && legendEntries.length}
					<ul class="inbox {legendCorner}" aria-label="Legend" data-testid="spectrum-legend-inside">
						{#if readout}
							<li class="xline num" class:linked={xReadout !== null && !cursorIsLocal}>{view.x.label} {xReadout ?? '-'} {view.x.unit ?? ''}</li>
						{/if}
						{#each legendEntries as e (e.index)}
							<li class:off={!e.shown} title={legendLabel(e.series, e.styleWords, e.shown)} onmouseenter={() => focusSeries(e.index)} onmouseleave={() => focusSeries(null)}>
								{#if onlegend}
									<button class="entry" aria-pressed={e.shown} onclick={() => onlegend?.(e.index)}>{@render entry(e)}</button>
								{:else}
									{@render entry(e)}
								{/if}
							</li>
						{/each}
					</ul>
				{:else if legend === 'off' && readout && xReadout !== null}
					<div class="inbox {legendCorner} num" aria-live="off">{view.x.label} {xReadout} {view.x.unit ?? ''}</div>
				{/if}
				{#if marks?.box.length}
					<pre class="inbox marks {marksCorner} num" data-testid="spectrum-marks">{marks.box.join('\n')}</pre>
				{/if}
				{#each notes as n (n.id)}
					{@const at = dragNote?.id === n.id ? dragNote : n}
					<div class="note" class:movable={!!onnote} style:left="{at.fx * 100}%" style:top="{at.fy * 100}%" onpointerdown={(e) => startNote(e, n)} role="note" title={onnote ? 'Drag to move the note' : undefined}>{n.text}</div>
				{/each}
			</div>
		{/if}
	</div>
</div>

<style>
	.copyimg {
		flex: none;
		font-size: var(--text-2xs, 11px);
	}

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

	.plot :global(.playline) {
		position: absolute;
		top: 0;
		bottom: 0;
		width: 9px;
		transform: translateX(-50%);
		cursor: ew-resize;
		z-index: 1;
	}

	/* the overlay's orange: the time the shape view beside it shows */
	.plot :global(.playline)::before {
		content: '';
		position: absolute;
		top: 0;
		bottom: 0;
		left: 3.5px;
		width: 2px;
		background: #e69f00;
	}

	.plot :global(.band-handle) {
		position: absolute;
		width: 12px;
		height: 12px;
		margin: -6px 0 0 -6px;
		border-radius: 50%;
		border: 2px solid var(--accent);
		background: var(--surface-1, #111);
		cursor: ns-resize;
		pointer-events: auto;
		touch-action: none;
		z-index: 2;
	}

	.plot :global(.region) {
		position: absolute;
		top: 0;
		bottom: 0;
		pointer-events: none;
	}

	.plot :global(.region.frame) {
		background: var(--region-frame);
		border-left: 1px solid var(--region-frame-edge);
	}

	.plot :global(.region.cut) {
		background: var(--region-cut);
	}

	.plot :global(.region.span) {
		border-left: 1.5px solid var(--region-frame-edge);
		border-right: 1.5px solid var(--region-frame-edge);
	}

	.plot :global(.region.drag) {
		pointer-events: auto;
		cursor: grab;
	}

	.plot :global(.region.drag:hover) {
		background: var(--region-frame);
	}

	.plot :global(.marker-grip) {
		position: absolute;
		top: 0;
		bottom: 0;
		left: -5px;
		width: 10px;
		cursor: move;
		pointer-events: auto;
		touch-action: none;
	}

	.plot :global(.guide) {
		position: absolute;
		top: 0;
		bottom: 0;
		width: 0;
		border-left: 1px dotted var(--text-3);
		pointer-events: none;
	}

	.plot :global(.marker) {
		position: absolute;
		top: 0;
		bottom: 0;
		width: 0;
		border-left: 1px dashed var(--text-2);
		pointer-events: none;
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

	.legend li.off,
	.inbox li.off {
		opacity: 0.45;
	}

	.legend li.off .name,
	.inbox li.off .name {
		text-decoration: line-through;
	}

	.entry {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		min-width: 0;
		padding: 0 2px;
		font: inherit;
		color: inherit;
		background: none;
		border: 0;
		border-radius: var(--radius-sm);
		cursor: pointer;
	}

	.entry:hover {
		background: var(--bg-3);
	}

	.entry:focus-visible {
		outline: 1px solid var(--focus);
	}

	/* The plot's drawing area: the legend, readouts and notes sit over it */
	.area {
		position: absolute;
		z-index: 2;
		pointer-events: none;
		container-type: size;
	}

	.inbox {
		position: absolute;
		max-width: calc(100% - 12px);
		max-height: calc(100% - 12px);
		overflow: hidden;
		margin: 0;
		padding: 4px 6px;
		list-style: none;
		font-size: var(--text-2xs);
		color: var(--text-2);
		background: color-mix(in srgb, var(--bg-plot) 88%, transparent);
		border: 1px solid var(--line-2);
		border-radius: var(--radius-sm);
		pointer-events: auto;
	}

	.inbox li {
		display: flex;
		align-items: center;
		gap: 6px;
		min-width: 0;
		min-height: 16px;
		white-space: nowrap;
	}

	.inbox .name {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.inbox .entry {
		min-width: 0;
		max-width: 100%;
	}

	/* a narrow plot keeps the swatch, which draws the dash and the marker, and drops their words */
	@container (max-width: 300px) {
		.inbox .style {
			display: none;
		}
	}

	@container (max-width: 180px) {
		.inbox .val {
			display: none;
		}
	}

	/* a plot too short to hold a box over its lines (a lane) keeps its lines clear */
	@container (max-height: 90px) {
		.inbox {
			display: none;
		}
	}

	.inbox .val {
		margin-left: auto;
		min-width: 7ch;
	}

	.inbox .xline {
		color: var(--text-1);
	}

	.inbox .xline.linked {
		color: var(--blue-1);
	}

	.inbox.top-left {
		top: 6px;
		left: 6px;
	}

	.inbox.top-right {
		top: 6px;
		right: 6px;
	}

	.inbox.bottom-left {
		bottom: 6px;
		left: 6px;
	}

	.inbox.bottom-right {
		bottom: 6px;
		right: 6px;
	}

	.inbox.marks {
		overflow: auto;
		white-space: pre;
		line-height: 16px;
		color: var(--text-1);
		font-family: var(--font-mono);
	}

	.note {
		position: absolute;
		max-width: 40%;
		padding: 2px 6px;
		font-size: var(--text-xs);
		color: var(--text-1);
		background: color-mix(in srgb, var(--bg-plot) 88%, transparent);
		border: 1px solid var(--line-3);
		border-radius: var(--radius-sm);
		white-space: pre-wrap;
		pointer-events: auto;
		touch-action: none;
	}

	.note.movable {
		cursor: move;
	}
</style>
