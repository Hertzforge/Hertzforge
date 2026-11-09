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
	import { onDestroy, onMount } from 'svelte';
	import type { Series, SpectrumView, SpectrumHandle } from '$lib/protocol/types';
	import { cssVar, theme } from '$lib/theme/theme.svelte';
	import { alignSeries, valueAt } from './align';
	import { cursors, playheadGroup, playheads } from './cursor';
	import { formatFixed, formatTick } from './format';
	import { decadeStride, logRange, logTicks, mantissasFor } from './logTicks';
	import { dashArray, seriesLooks } from './roles';
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
	}

	let { view, live = false, followX = false, minHeight = 180, showTitle = true, yFixed = null, scope, onPick, onRange, marker = null, guides = [], onMarkerDrag, onhandle, onspan }: Props = $props();

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

	/** Per-series style, with measured series stepping through the blue family and followers dashed. */
	const styled = $derived.by(() => {
		const looks = seriesLooks(view.series);
		return view.series.map((s, i) => ({ series: s, index: i, ...looks[i] }));
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
		return JSON.stringify([v.x, v.y, xs, v.regions ?? [], theme.version]);
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

	/** Pixels each decade gets on this axis. */
	function pxPerDecade(u: uPlot, axis: 0 | 1, min: number, max: number): number {
		const px = (axis === 0 ? u.bbox.width : u.bbox.height) / devicePixelRatio;
		const decades = Math.max(0.3, Math.log10(Math.max(max, 1e-300) / Math.max(min, 1e-300)));
		return px / decades;
	}

	/** Ticks for a log axis, as dense as its pixels allow and never denser. */
	function logSplits(u: uPlot, axis: 0 | 1, min: number, max: number): number[] {
		const ppd = pxPerDecade(u, axis, min, max);
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
			focus: { alpha: 0.25 },
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
					splits: logX ? (u, _i, min, max) => logSplits(u, 0, min, max) : undefined,
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
				...ordered.map(({ series, index, style, colorVar, color }) => ({
					label: series.name,
					stroke: color ?? cssVar(colorVar),
					width: style.width,
					dash: style.dash,
					spanGaps: true,
					points: { show: false },
					paths: series.shape === 'steps' && series.edges ? stepsPath(() => view.series[index], logX, logY) : undefined
				}))
			],
			hooks: {
				draw: [placePlayLine, placeMarker, placeRegions, placeHandles],
				setScale: [
					(u, key) => {
						if (key === 'x' && onRange && u.scales.x.min != null && u.scales.x.max != null) onRange(u.scales.x.min, u.scales.x.max);
					}
				],
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
		if (kept) plot.setScale('x', kept);
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
		const up = (ev: PointerEvent) => {
			grip.removeEventListener('pointermove', move);
			grip.removeEventListener('pointerup', up);
			grip.removeEventListener('pointercancel', up);
			report(ev, true);
		};
		grip.addEventListener('pointermove', move);
		grip.addEventListener('pointerup', up);
		grip.addEventListener('pointercancel', up);
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
		const up = (ev: PointerEvent) => {
			el.removeEventListener('pointermove', move);
			el.removeEventListener('pointerup', up);
			el.removeEventListener('pointercancel', up);
			if (Math.abs(ev.clientY - e.clientY) > 3) onhandle?.(h, decibels(ev));
			else placeHandles();
		};
		el.addEventListener('pointermove', move);
		el.addEventListener('pointerup', up);
		el.addEventListener('pointercancel', up);
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

	$effect(() => {
		// Track the view and the theme; rebuild or update in place.
		void view;
		void theme.version;
		void yFixed;
		if (plot) update();
		bindCursor(view.link);
		bindPlayhead(view.link ? playheadGroup(view.link, scope) : undefined);
	});

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
		<button class="btn ghost sm copyimg" data-testid="spectrum-copy-image" onclick={copyImage} title="Copy the plot as an image">{copiedImage === 'copied' ? 'Copied' : copiedImage === 'saved' ? 'Saved as PNG' : 'Copy Image'}</button>
	</div>
	<ul class="legend" aria-label="Legend">
		{#each styled as { series, style, colorVar, color, follows }, i (i)}
			{#if !follows}
			<li title={legendLabel(series, style.styleName)} onmouseenter={() => focusSeries(i)} onmouseleave={() => focusSeries(null)}>
				<svg class="swatch" viewBox="0 0 26 8" width="26" height="8" aria-hidden="true">
					<line
						x1="1"
						y1="4"
						x2="25"
						y2="4"
						stroke={color ?? `var(${colorVar})`}
						stroke-width={style.width + 0.4}
						stroke-dasharray={dashArray(series.role)}
						stroke-linecap="butt"
					/>
				</svg>
				<span class="name">{series.name}</span>
				<span class="style">{style.styleName}</span>
				<span class="val num">{readoutText(readouts[i])}</span>
			</li>
			{/if}
		{/each}
	</ul>
	<div class="plot" bind:this={plotEl} style:min-height="{minHeight}px"></div>
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
</style>
