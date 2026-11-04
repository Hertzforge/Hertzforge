<script lang="ts">
	/**
	 * Rows of values against frequency, drawn as a raster on one canvas: the MIMO error heatmap.
	 * Each row is a control, each column a frequency the service picked for the plot's width, each
	 * cell the control's error against its reference in dB (or a coherence error from 0 to 1). The
	 * scale is ordered by lightness and breaks at each row's alarm and abort bands; cells beyond
	 * abort are hatched as well, and the right-hand column gives each row's rms error with a mark
	 * and a count for lines beyond alarm and abort, so nothing rests on color. The cursor always
	 * reads the cell's number; the keyboard moves it (arrows, Shift for ten columns, Home and End),
	 * and Enter or a click opens the row's control.
	 */
	import type { HeatmapView } from '$lib/protocol/types';
	import { cssVar, theme } from '$lib/theme/theme.svelte';
	import { formatDb, formatFixed, formatTick } from './format';
	import { abortRuns, cellAt, labelStride, rasterize, rowOrder, summaries, type HeatMode } from './heatmap';
	import { logTicks } from './logTicks';
	import { BAND_WORDS, bandClass, DEFAULT_BAND, errorGradient, sequentialGradient, type RowBand } from './scales';

	interface Props {
		view: HeatmapView | null;
		mode?: HeatMode;
		/** Rows as the setup lists them, or the largest rms error first. */
		order?: 'setup' | 'worst';
		/** Open a row's control (its index in the view's rows, and its name). */
		onopen?: (row: number, label: string) => void;
		/** Shown while there is no data yet. */
		empty?: string;
	}
	let { view, mode = 'error', order = 'setup', onopen, empty = 'Waiting for the run.' }: Props = $props();

	const LEFT = 92;
	const RIGHT = 132;
	const MIN_ROW = 13;
	const AXIS = 30;

	let host = $state<HTMLDivElement | null>(null);
	let scroller = $state<HTMLDivElement | null>(null);
	let canvas = $state<HTMLCanvasElement | null>(null);
	let axis = $state<HTMLCanvasElement | null>(null);
	let width = $state(0);
	let height = $state(0);
	let cursor = $state<{ row: number; col: number } | null>(null);
	let drawMs = $state(0);

	const rows = $derived(view ? view.rows.ticks.length : 0);
	const cols = $derived(view ? view.x_values.length : 0);
	const data = $derived(view ? { rows, cols, values: view.values, bands: view.row_bands as RowBand[] } : null);
	const sums = $derived(data ? summaries(data, mode) : []);
	const ord = $derived(rowOrder(rows, order, sums));
	const rowPx = $derived(rows ? Math.max(MIN_ROW, Math.floor(Math.max(0, height - 4) / rows)) : MIN_ROW);
	const cellW = $derived(Math.max(0, width - LEFT - RIGHT));
	const bodyH = $derived(rows * rowPx);
	const tk = $derived<'light' | 'dark'>(theme.resolved);
	const firstBand = $derived((view?.row_bands[0] as RowBand | undefined) ?? DEFAULT_BAND);
	const gradient = $derived(mode === 'error' ? errorGradient(firstBand, tk) : sequentialGradient(tk));

	/** What the cursor reads, in words and numbers. */
	const reading = $derived.by(() => {
		if (!view || !cursor || !data) return null;
		const r = ord[cursor.row];
		const label = view.rows.ticks[r];
		const f = view.x_values[cursor.col];
		const v = view.values[r * cols + cursor.col];
		const band = (view.row_bands[r] as RowBand | undefined) ?? DEFAULT_BAND;
		const value = !Number.isFinite(v) ? 'no value' : mode === 'error' ? `${formatDb(v, 2)}, ${BAND_WORDS[bandClass(v, band)]}` : `${formatFixed(v, 3)}`;
		return { label, f, value, row: r };
	});

	let raster: HTMLCanvasElement | null = null;
	let pixels: ImageData | null = null;
	let frame = 0;

	function schedule() {
		if (frame) return;
		frame = requestAnimationFrame(() => {
			frame = 0;
			draw();
		});
	}

	function draw() {
		if (!canvas || !data || !view) return;
		const t0 = performance.now();
		const dpr = Math.min(2, devicePixelRatio || 1);
		const W = Math.max(1, width);
		const H = Math.max(1, bodyH);
		if (canvas.width !== Math.round(W * dpr) || canvas.height !== Math.round(H * dpr)) {
			canvas.width = Math.round(W * dpr);
			canvas.height = Math.round(H * dpr);
			canvas.style.width = `${W}px`;
			canvas.style.height = `${H}px`;
		}
		const ctx = canvas.getContext('2d');
		if (!ctx) return;
		ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
		ctx.clearRect(0, 0, W, H);

		// The raster: one pixel per cell, scaled up without smoothing.
		raster ??= document.createElement('canvas');
		if (raster.width !== cols || raster.height !== rows) {
			raster.width = Math.max(1, cols);
			raster.height = Math.max(1, rows);
			pixels = null;
		}
		const rctx = raster.getContext('2d');
		if (rctx && cols && rows) {
			pixels ??= rctx.createImageData(cols, rows);
			rasterize(data, mode, tk, ord, pixels.data);
			rctx.putImageData(pixels, 0, 0);
			ctx.imageSmoothingEnabled = false;
			ctx.drawImage(raster, LEFT, 0, cellW, rows * rowPx);
		}

		const text1 = cssVar('--text-1');
		const text2 = cssVar('--text-2');
		const text3 = cssVar('--text-3');
		const line = cssVar('--line-2');
		const warn = cssVar('--status-warn');
		const fail = cssVar('--status-fail');
		const fontUi = cssVar('--font-ui');
		const fontMono = cssVar('--font-mono');
		const colW = cols ? cellW / cols : 0;

		// Beyond abort: hatched, in the lightness opposite the cell's, so it shows on any color.
		if (mode === 'error') {
			ctx.save();
			ctx.strokeStyle = tk === 'light' ? 'rgba(255,255,255,0.8)' : 'rgba(10,14,19,0.75)';
			ctx.lineWidth = 1;
			for (let k = 0; k < rows; k++) {
				const runs = abortRuns(data, ord[k]);
				if (!runs.length) continue;
				const y = k * rowPx;
				ctx.beginPath();
				for (const [a, b] of runs) {
					const x0 = LEFT + a * colW;
					const x1 = LEFT + b * colW;
					ctx.rect(x0, y, Math.max(1, x1 - x0), rowPx);
				}
				ctx.save();
				ctx.clip();
				ctx.beginPath();
				for (let x = LEFT - rowPx; x < LEFT + cellW; x += 5) {
					ctx.moveTo(x, y + rowPx);
					ctx.lineTo(x + rowPx, y);
				}
				ctx.stroke();
				ctx.restore();
			}
			ctx.restore();
		}

		// Row separators when rows are tall enough to need them.
		if (rowPx >= 16) {
			ctx.strokeStyle = cssVar('--bg-plot');
			ctx.globalAlpha = 0.6;
			ctx.beginPath();
			for (let k = 1; k < rows; k++) {
				ctx.moveTo(LEFT, k * rowPx + 0.5);
				ctx.lineTo(LEFT + cellW, k * rowPx + 0.5);
			}
			ctx.stroke();
			ctx.globalAlpha = 1;
		}

		// Row names on the left, the rms column on the right.
		const stride = labelStride(rowPx, 12);
		const size = Math.min(12.5, Math.max(10.5, rowPx - 3));
		ctx.textBaseline = 'middle';
		for (let k = 0; k < rows; k++) {
			const r = ord[k];
			const y = k * rowPx + rowPx / 2;
			const s = sums[r];
			const isCursor = cursor?.row === k;
			if (k % stride === 0 || isCursor) {
				ctx.font = `${isCursor ? 640 : 400} ${size}px ${fontUi}`;
				ctx.fillStyle = isCursor ? text1 : text2;
				ctx.textAlign = 'right';
				ctx.fillText(fit(ctx, view.rows.ticks[r], LEFT - 10), LEFT - 6, y);
			}
			if (k % stride !== 0 && !isCursor) continue;
			// rms
			ctx.textAlign = 'right';
			ctx.font = `${size}px ${fontMono}`;
			ctx.fillStyle = text1;
			const rms = s?.rms;
			const xr = LEFT + cellW + 70;
			ctx.fillText(rms === null || rms === undefined ? '-' : mode === 'error' ? formatFixed(rms, 3) : formatFixed(rms, 2), xr, y);
			// marks: a filled square for lines beyond abort, an open triangle for beyond alarm
			if (mode === 'error' && s) {
				const mx = xr + 10;
				const half = Math.min(5, rowPx / 2 - 1.5);
				if (s.abort > 0) {
					ctx.fillStyle = fail;
					ctx.fillRect(mx, y - half, half * 2, half * 2);
					ctx.fillStyle = text2;
					ctx.textAlign = 'left';
					ctx.font = `${size - 1}px ${fontMono}`;
					ctx.fillText(String(s.abort), mx + half * 2 + 4, y);
				} else if (s.alarm > 0) {
					ctx.strokeStyle = warn;
					ctx.lineWidth = 1.4;
					ctx.beginPath();
					ctx.moveTo(mx + half, y - half);
					ctx.lineTo(mx + half * 2, y + half);
					ctx.lineTo(mx, y + half);
					ctx.closePath();
					ctx.stroke();
					ctx.fillStyle = text2;
					ctx.textAlign = 'left';
					ctx.font = `${size - 1}px ${fontMono}`;
					ctx.fillText(String(s.alarm), mx + half * 2 + 4, y);
				}
			}
		}

		// The cursor: the row outlined, the column marked.
		if (cursor) {
			ctx.strokeStyle = text1;
			ctx.lineWidth = 1.5;
			ctx.strokeRect(LEFT + 0.75, cursor.row * rowPx + 0.75, cellW - 1.5, rowPx - 1.5);
			const cx = LEFT + (cursor.col + 0.5) * colW;
			ctx.setLineDash([3, 3]);
			ctx.beginPath();
			ctx.moveTo(cx, 0);
			ctx.lineTo(cx, rows * rowPx);
			ctx.stroke();
			ctx.setLineDash([]);
		}
		ctx.strokeStyle = line;
		ctx.lineWidth = 1;
		ctx.strokeRect(LEFT - 0.5, -0.5, cellW + 1, rows * rowPx + 1);
		void text3;
		drawAxis(dpr);
		drawMs = performance.now() - t0;
	}

	function fit(ctx: CanvasRenderingContext2D, s: string, w: number): string {
		if (ctx.measureText(s).width <= w) return s;
		let t = s;
		while (t.length > 1 && ctx.measureText(`${t}...`).width > w) t = t.slice(0, -1);
		return `${t}...`;
	}

	function drawAxis(dpr: number) {
		if (!axis || !view || cols < 1) return;
		const W = Math.max(1, width);
		if (axis.width !== Math.round(W * dpr)) {
			axis.width = Math.round(W * dpr);
			axis.height = Math.round(AXIS * dpr);
			axis.style.width = `${W}px`;
			axis.style.height = `${AXIS}px`;
		}
		const ctx = axis.getContext('2d');
		if (!ctx) return;
		ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
		ctx.clearRect(0, 0, W, AXIS);
		const x = view.x_values;
		const f0 = x[0];
		const f1 = x[cols - 1];
		const colW = cellW / cols;
		const at = (f: number) => LEFT + colW / 2 + ((Math.log(f) - Math.log(f0)) / (Math.log(f1) - Math.log(f0) || 1)) * (cellW - colW);
		ctx.fillStyle = cssVar('--plot-axis');
		ctx.strokeStyle = cssVar('--plot-axis');
		ctx.font = `11px ${cssVar('--font-mono')}`;
		ctx.textAlign = 'center';
		ctx.textBaseline = 'top';
		const ticks = f0 > 0 && f1 > f0 ? logTicks(f0, f1, cellW / Math.log10(f1 / f0) > 160 ? [1, 2, 5] : [1]) : [];
		for (const t of ticks) {
			const px = at(t);
			if (px < LEFT - 1 || px > LEFT + cellW + 1) continue;
			ctx.beginPath();
			ctx.moveTo(px, 0);
			ctx.lineTo(px, 4);
			ctx.stroke();
			ctx.fillText(formatTick(t), px, 6);
		}
		ctx.textAlign = 'left';
		ctx.font = `11.5px ${cssVar('--font-ui')}`;
		ctx.fillText('Frequency (Hz)', LEFT + cellW + 8, 6);
	}

	$effect(() => {
		void view;
		void mode;
		void order;
		void width;
		void height;
		void cursor;
		void theme.version;
		schedule();
	});

	$effect(() => {
		const box = scroller;
		if (!host || !box) return;
		const measure = () => {
			width = box.clientWidth;
			height = box.clientHeight;
		};
		const obs = new ResizeObserver(measure);
		obs.observe(host);
		obs.observe(box);
		measure();
		return () => obs.disconnect();
	});

	$effect(() => () => {
		if (frame) cancelAnimationFrame(frame);
	});

	function pointAt(e: MouseEvent) {
		if (!canvas) return null;
		const r = canvas.getBoundingClientRect();
		return cellAt(e.clientX - r.left, e.clientY - r.top, { x: LEFT, y: 0, w: cellW, h: rows * rowPx, rows, cols });
	}

	function onMove(e: MouseEvent) {
		const c = pointAt(e);
		if (c) cursor = c;
	}

	function open(k: number) {
		if (!view || !onopen) return;
		const r = ord[k];
		onopen(r, view.rows.ticks[r]);
	}

	function onClick(e: MouseEvent) {
		const c = pointAt(e);
		if (c) {
			cursor = c;
			open(c.row);
		}
	}

	function onKey(e: KeyboardEvent) {
		if (!rows || !cols) return;
		const c = cursor ?? { row: 0, col: Math.floor(cols / 2) };
		const big = e.shiftKey ? 10 : 1;
		let next = { ...c };
		if (e.key === 'ArrowUp') next.row = Math.max(0, c.row - 1);
		else if (e.key === 'ArrowDown') next.row = Math.min(rows - 1, c.row + 1);
		else if (e.key === 'ArrowLeft') next.col = Math.max(0, c.col - big);
		else if (e.key === 'ArrowRight') next.col = Math.min(cols - 1, c.col + big);
		else if (e.key === 'Home') next.col = 0;
		else if (e.key === 'End') next.col = cols - 1;
		else if (e.key === 'Enter' && cursor) {
			open(cursor.row);
			e.preventDefault();
			return;
		} else return;
		e.preventDefault();
		cursor = next;
		// keep the cursor's row in view
		const top = next.row * rowPx;
		if (scroller && (top < scroller.scrollTop || top + rowPx > scroller.scrollTop + scroller.clientHeight)) scroller.scrollTop = top - scroller.clientHeight / 2;
	}
</script>

<div class="heatmap" bind:this={host} data-testid="heatmap" data-rows={rows} data-cols={cols} data-draw-ms={drawMs.toFixed(2)}>
	<div class="head">
		<div class="readout num" aria-live="polite" data-testid="heatmap-readout">
			{#if reading}
				<span class="who">{reading.label}</span>
				<span>{formatFixed(reading.f, 4)} Hz</span>
				<span class="val">{mode === 'error' ? 'Error' : 'Coherence error'} {reading.value}</span>
			{:else if view}
				<span class="dim">Point at a cell, or focus the map and use the arrow keys, to read its value. Click a row to open it in the grid.</span>
			{/if}
		</div>
		<div class="legend" aria-label="Color scale">
			{#if mode === 'error'}
				<span class="lab">{formatDb(-2 * Math.max(firstBand.abort_db[0], firstBand.abort_db[1]), 0)}</span>
				<span class="bar" style:background={gradient}></span>
				<span class="lab">{formatDb(2 * Math.max(firstBand.abort_db[0], firstBand.abort_db[1]), 0)}</span>
				<span class="key"><span class="sq" aria-hidden="true"></span>beyond abort, hatched</span>
				<span class="key"><span class="tri" aria-hidden="true"></span>beyond alarm</span>
			{:else}
				<span class="lab">0</span>
				<span class="bar" style:background={gradient}></span>
				<span class="lab">1</span>
			{/if}
		</div>
	</div>
	<div class="colhead" style:padding-left="{LEFT}px">
		<span class="grow">{view ? `${rows} ${mode === 'error' ? 'controls' : 'pairs'}, ${cols} columns` : ''}</span>
		<span class="rmshead" style:width="{RIGHT}px">RMS {mode === 'error' ? '(dB)' : ''}</span>
	</div>
	{#if view && rows}
		<div class="scroll" bind:this={scroller}>
			<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
			<canvas
				bind:this={canvas}
				tabindex="0"
				aria-label="{view.title}: {rows} rows against frequency. Arrow keys move the cursor, Enter opens the row."
				data-testid="heatmap-canvas"
				onmousemove={onMove}
				onclick={onClick}
				onkeydown={onKey}
			></canvas>
		</div>
		<canvas class="axis" bind:this={axis} aria-hidden="true"></canvas>
	{:else}
		<div class="empty" bind:this={scroller}>{empty}</div>
	{/if}
</div>

<style>
	.heatmap {
		display: flex;
		flex-direction: column;
		height: 100%;
		min-height: 240px;
		padding: var(--sp-2);
		background: var(--bg-plot);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-md);
	}

	.head {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		justify-content: space-between;
		gap: 6px var(--sp-4);
		min-height: 26px;
		padding: 0 4px 6px;
	}

	.readout {
		display: flex;
		flex-wrap: wrap;
		gap: 4px 14px;
		font-size: var(--text-sm);
		color: var(--text-1);
	}

	.readout .who {
		font-family: var(--font-ui);
		font-weight: var(--weight-strong);
	}

	.readout .dim {
		font-family: var(--font-ui);
		color: var(--text-3);
	}

	.legend {
		display: flex;
		align-items: center;
		gap: 6px;
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	.legend .bar {
		width: 180px;
		height: 10px;
		border: 1px solid var(--line-2);
		border-radius: 2px;
	}

	.legend .lab {
		font-family: var(--font-mono);
	}

	.key {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		margin-left: 8px;
	}

	.sq {
		width: 9px;
		height: 9px;
		background: var(--status-fail);
	}

	.tri {
		width: 0;
		height: 0;
		border-right: 5px solid transparent;
		border-bottom: 9px solid var(--status-warn);
		border-left: 5px solid transparent;
	}

	.colhead {
		display: flex;
		padding-bottom: 2px;
		font-size: var(--text-2xs);
		color: var(--text-3);
		text-transform: uppercase;
		letter-spacing: 0.05em;
	}

	.grow {
		flex: 1;
	}

	.rmshead {
		padding-left: 22px;
	}

	.scroll {
		flex: 1;
		min-height: 0;
		overflow-x: hidden;
		overflow-y: auto;
	}

	canvas {
		display: block;
		cursor: crosshair;
	}

	canvas:focus-visible {
		outline: 2px solid var(--focus);
		outline-offset: 1px;
	}

	.axis {
		flex: none;
		cursor: default;
	}

	.empty {
		display: grid;
		flex: 1;
		place-items: center;
		color: var(--text-3);
		font-size: var(--text-sm);
	}
</style>
