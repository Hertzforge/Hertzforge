<script lang="ts">
	import { copyCanvas, plotBackground } from './copyImage';
	/**
	 * One colored cell per value (a MAC, say), drawn on a 2-D canvas: a few thousand cells at most,
	 * redrawn only on resize, theme change or hover. Color is the sequential colormap over the
	 * view's pinned range, with a numeric legend; each cell writes its value when it is large
	 * enough to hold the text. Hovering reads the cell's row, column and value. A matrix that
	 * `selects` a shape view marks the chosen cell in the overlay's orange; a click or the arrow
	 * keys choose another. The grid zooms and pans as a data plot does: the wheel zooms about the
	 * pointer, over the row ticks the rows alone and over the column ticks the columns alone; a
	 * drag pans; a right-drag zooms; a double-click or A shows the whole matrix again. A large
	 * MAC's squares are too small to read or to pick until it is zoomed (Brandon, 2026-11-16).
	 */
	import { onDestroy, onMount } from 'svelte';
	import type { MatrixView } from '$lib/protocol/types';
	import { cssVar, theme } from '$lib/theme/theme.svelte';
	import { colormap, colormapGradient } from './colormap';
	import { CHECKER, MARK_RED, PICK_BLUE } from './macBars';
	import { cellAt, cellText, inkOn, isZoomed, matrixLayout, panWindow, wholeWindow, zoomWindow, type MatrixLayout, type MatrixWindow } from './matrix';

	interface Props {
		view: MatrixView;
		showTitle?: boolean;
		/** The cell whose pair the linked shape view shows. */
		selected?: { row: number; column: number } | null;
		/** Every picked cell, outlined in blue; the selected one is among them. */
		picks?: { row: number; column: number }[];
		/** A click chose a cell; `extend` with the modifier held, to add it to the picks. */
		onselect?: (cell: { row: number; column: number }, extend?: boolean) => void;
	}

	let { view, showTitle = true, selected = null, picks = [], onselect }: Props = $props();

	const ROWS = $derived(view.rows.ticks.length);
	const COLUMNS = $derived(view.columns.ticks.length);
	/** Room for the row ticks and the rotated row label, and for the column ticks and label. */
	const LEFT = 96;
	const BOTTOM = 44;

	let host: HTMLDivElement;
	let canvas: HTMLCanvasElement;
	let size = $state({ width: 0, height: 0 });
	let hover = $state<{ row: number; column: number } | null>(null);
	let layout: MatrixLayout | null = null;
	let resizeObs: ResizeObserver | null = null;
	/** The part of the matrix drawn: the whole of it until it is zoomed. */
	let win = $state<MatrixWindow>({ r0: 0, r1: 0, c0: 0, c1: 0 });
	const zoomed = $derived(isZoomed(win, ROWS, COLUMNS));
	// another matrix, or one of another size, is shown whole
	$effect(() => {
		win = wholeWindow(ROWS, COLUMNS);
	});

	const [low, high] = $derived(view.value_range);
	const legendTicks = $derived([low, (low + high) / 2, high].map((t) => String(+t.toFixed(2))));

	function value(row: number, column: number): number {
		return view.values[row * COLUMNS + column];
	}

	function draw() {
		const ctx = canvas?.getContext('2d');
		if (!ctx || !size.width || !size.height) return;
		const dpr = window.devicePixelRatio || 1;
		canvas.width = Math.round(size.width * dpr);
		canvas.height = Math.round(size.height * dpr);
		ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
		ctx.clearRect(0, 0, size.width, size.height);
		const mono = cssVar('--font-mono');
		// column ticks lie flat when they fit their cells and tilt 45 degrees when not, so every
		// column keeps its tick until even tilted ones would overlap
		ctx.font = `10.5px ${mono}`;
		const widest = Math.max(...view.columns.ticks.map((t) => ctx.measureText(t).width), 1);
		// the cells are sized for the part shown: the whole matrix, or what a zoom leaves
		const spanR = win.r1 - win.r0 || ROWS;
		const spanC = win.c1 - win.c0 || COLUMNS;
		let l = matrixLayout(size.width, size.height, spanR, spanC, LEFT, BOTTOM);
		const tilted = widest + 8 > l.cellW;
		if (tilted) l = matrixLayout(size.width, size.height, spanR, spanC, LEFT, widest * Math.SQRT1_2 + 34);
		l = { ...l, rows: ROWS, columns: COLUMNS, shown: { ...win } };
		layout = l;
		const X = (column: number) => l.x0 + (column - win.c0) * l.cellW;
		const Y = (row: number) => l.y0 + (row - win.r0) * l.cellH;
		const firstR = Math.max(0, Math.floor(win.r0));
		const lastR = Math.min(ROWS, Math.ceil(win.r0 + spanR));
		const firstC = Math.max(0, Math.floor(win.c0));
		const lastC = Math.min(COLUMNS, Math.ceil(win.c0 + spanC));
		// a zoom cuts cells at the grid's edges
		ctx.save();
		ctx.beginPath();
		ctx.rect(l.x0, l.y0, spanC * l.cellW, spanR * l.cellH);
		ctx.clip();
		const text2 = cssVar('--text-2');
		const text3 = cssVar('--text-3');
		const rgb: [number, number, number] = [0, 0, 0];
		const span = high - low || 1;
		const showText = l.cellW >= 34 && l.cellH >= 16;
		ctx.font = `11px ${mono}`;
		ctx.textAlign = 'center';
		ctx.textBaseline = 'middle';
		for (let r = firstR; r < lastR; r++) {
			for (let c = firstC; c < lastC; c++) {
				const v = value(r, c);
				colormap((v - low) / span, rgb);
				ctx.fillStyle = Number.isFinite(v)
					? `rgb(${rgb[0] * 255} ${rgb[1] * 255} ${rgb[2] * 255})`
					: cssVar('--bg-3');
				const x = X(c);
				const y = Y(r);
				ctx.fillRect(x, y, Math.ceil(l.cellW), Math.ceil(l.cellH));
				if (showText) {
					ctx.fillStyle = inkOn(rgb) === 'dark' ? '#111' : '#f4f4f4';
					ctx.fillText(cellText(v), x + l.cellW / 2, y + l.cellH / 2);
				}
			}
		}
		// committed matches wear a red checker
		for (const [r, c] of view.matched ?? []) {
			if (r >= ROWS || c >= COLUMNS) continue;
			const x = X(c);
			const y = Y(r);
			const k = CHECKER;
			ctx.fillStyle = MARK_RED;
			for (let i = 0; i < k; i++) {
				for (let j = 0; j < k; j++) {
					if ((i + j) % 2) continue;
					ctx.fillRect(x + (i * l.cellW) / k, y + (j * l.cellH) / k, l.cellW / k + 0.5, l.cellH / k + 0.5);
				}
			}
		}
		for (const p of picks) {
			if (p.row >= ROWS || p.column >= COLUMNS) continue;
			ctx.strokeStyle = PICK_BLUE;
			ctx.lineWidth = 2;
			ctx.strokeRect(X(p.column) + 1, Y(p.row) + 1, l.cellW - 2, l.cellH - 2);
		}
		if (hover) {
			ctx.strokeStyle = cssVar('--text-1');
			ctx.lineWidth = 2;
			ctx.strokeRect(X(hover.column) + 1, Y(hover.row) + 1, l.cellW - 2, l.cellH - 2);
		}
		if (selected) {
			// the overlay's orange, so the cell reads as the pair the shape view draws
			ctx.strokeStyle = '#e69f00';
			ctx.lineWidth = 3;
			ctx.strokeRect(X(selected.column) + 1.5, Y(selected.row) + 1.5, l.cellW - 3, l.cellH - 3);
		}
		ctx.restore();
		// ticks, thinned to every nth where they would overlap
		ctx.fillStyle = text2;
		ctx.font = `10.5px ${mono}`;
		const rowEvery = Math.max(1, Math.ceil(13 / l.cellH));
		ctx.textAlign = 'right';
		// a tick is written while the middle of its cell is on the grid
		for (let r = Math.ceil(firstR / rowEvery) * rowEvery; r < lastR; r += rowEvery) {
			if (r + 0.5 < win.r0 || r + 0.5 > win.r0 + spanR) continue;
			ctx.fillText(view.rows.ticks[r], l.x0 - 6, Y(r + 0.5));
		}
		const below = l.y0 + spanR * l.cellH + 5;
		const columnEvery = Math.max(1, Math.ceil((tilted ? 13 * Math.SQRT2 : widest + 8) / l.cellW));
		for (let c = Math.ceil(firstC / columnEvery) * columnEvery; c < lastC; c += columnEvery) {
			if (c + 0.5 < win.c0 || c + 0.5 > win.c0 + spanC) continue;
			const x = X(c + 0.5);
			if (tilted) {
				ctx.save();
				ctx.translate(x, below);
				ctx.rotate(-Math.PI / 4);
				ctx.textAlign = 'right';
				ctx.textBaseline = 'middle';
				ctx.fillText(view.columns.ticks[c], 0, 0);
				ctx.restore();
			} else {
				ctx.textAlign = 'center';
				ctx.textBaseline = 'top';
				ctx.fillText(view.columns.ticks[c], x, below);
			}
		}
		// axis labels
		ctx.fillStyle = text3;
		ctx.font = `11.5px ${mono}`;
		ctx.textAlign = 'center';
		ctx.textBaseline = 'top';
		ctx.fillText(view.columns.label, l.x0 + (spanC * l.cellW) / 2, below + (tilted ? widest * Math.SQRT1_2 + 8 : 16));
		ctx.save();
		ctx.translate(12, l.y0 + (spanR * l.cellH) / 2);
		ctx.rotate(-Math.PI / 2);
		ctx.textBaseline = 'middle';
		ctx.fillText(view.rows.label, 0, 0);
		ctx.restore();
	}

	/** Where a point on the canvas is in cells, on or off the grid, and which of its axes it is over. */
	function pointIn(e: MouseEvent): { row: number; column: number; axes: { rows: boolean; columns: boolean } } | null {
		if (!layout || !layout.cellW || !layout.cellH) return null;
		const box = canvas.getBoundingClientRect();
		const across = (e.clientX - box.left - layout.x0) / layout.cellW;
		const down = (e.clientY - box.top - layout.y0) / layout.cellH;
		// left of the grid is the rows' ticks and under it the columns': the wheel there zooms that axis alone
		const overRows = across < 0;
		const overColumns = down > win.r1 - win.r0;
		return {
			row: win.r0 + Math.min(win.r1 - win.r0, Math.max(0, down)),
			column: win.c0 + Math.min(win.c1 - win.c0, Math.max(0, across)),
			axes: { rows: overRows || !overColumns, columns: overColumns || !overRows }
		};
	}

	/** A drag under way: where it began, the window then, and whether it has moved far enough to be one. */
	let drag: { x: number; y: number; from: MatrixWindow; at: { row: number; column: number }; zoom: boolean; moved: boolean } | null = null;
	/** A drag just ended, so the click that follows it picks nothing. */
	let dragged = false;

	function onDown(e: PointerEvent) {
		const at = pointIn(e);
		if (!at || (e.button !== 0 && e.button !== 2)) return;
		drag = { x: e.clientX, y: e.clientY, from: { ...win }, at, zoom: e.button === 2, moved: false };
		dragged = false;
		canvas.setPointerCapture(e.pointerId);
	}

	function onMove(e: PointerEvent) {
		if (!layout) return;
		if (drag) {
			const dx = e.clientX - drag.x;
			const dy = e.clientY - drag.y;
			if (!drag.moved && Math.hypot(dx, dy) < 4) return;
			drag.moved = true;
			hover = null;
			if (drag.zoom) {
				// right-drag: right and down zoom in, left and up out, about where the drag began
				const columns = zoomWindow(drag.from, ROWS, COLUMNS, Math.exp(-dx / 150), drag.at, { rows: false, columns: true });
				win = zoomWindow(columns, ROWS, COLUMNS, Math.exp(-dy / 150), drag.at, { rows: true, columns: false });
			} else win = panWindow(drag.from, ROWS, COLUMNS, -dy / layout.cellH, -dx / layout.cellW);
			return;
		}
		const box = canvas.getBoundingClientRect();
		const at = cellAt(e.clientX - box.left, e.clientY - box.top, layout);
		if (at?.row !== hover?.row || at?.column !== hover?.column) hover = at;
	}

	function onUp(e: PointerEvent) {
		if (!drag) return;
		dragged = drag.moved;
		drag = null;
		if (canvas.hasPointerCapture(e.pointerId)) canvas.releasePointerCapture(e.pointerId);
	}

	function onWheel(e: WheelEvent) {
		const at = pointIn(e);
		if (!at) return;
		e.preventDefault();
		win = zoomWindow(win, ROWS, COLUMNS, Math.exp(e.deltaY * 0.0015), at, at.axes);
	}

	function viewAll() {
		win = wholeWindow(ROWS, COLUMNS);
	}

	onMount(() => {
		resizeObs = new ResizeObserver(([entry]) => {
			size = { width: entry.contentRect.width, height: entry.contentRect.height };
		});
		resizeObs.observe(host);
		// not passive, so the wheel zooms the grid and does not scroll the page under it
		canvas.addEventListener('wheel', onWheel, { passive: false });
	});

	function onClick(e: MouseEvent) {
		// the release of a pan or a zoom is not a pick
		if (dragged) {
			dragged = false;
			return;
		}
		if (!layout || !onselect) return;
		const box = canvas.getBoundingClientRect();
		const at = cellAt(e.clientX - box.left, e.clientY - box.top, layout);
		if (at) onselect(at, e.metaKey || e.ctrlKey || e.shiftKey);
	}

	function onKey(e: KeyboardEvent) {
		if ((e.key === 'a' || e.key === 'A') && !e.metaKey && !e.ctrlKey && !e.altKey) {
			e.preventDefault();
			viewAll();
			return;
		}
		if (!onselect || !selected) return;
		const step: Record<string, [number, number]> = { ArrowUp: [-1, 0], ArrowDown: [1, 0], ArrowLeft: [0, -1], ArrowRight: [0, 1] };
		const d = step[e.key];
		if (!d) return;
		e.preventDefault();
		onselect({
			row: Math.min(ROWS - 1, Math.max(0, selected.row + d[0])),
			column: Math.min(COLUMNS - 1, Math.max(0, selected.column + d[1]))
		});
	}

	onDestroy(() => resizeObs?.disconnect());

	$effect(() => {
		void size;
		void hover;
		void selected;
		void picks;
		void theme.version;
		void view;
		void win;
		draw();
	});

	let copiedImage = $state<'copied' | 'saved' | null>(null);
	/** The cells, as an image on the clipboard. */
	async function copyImage() {
		if (!canvas) return;
		copiedImage = await copyCanvas(canvas, plotBackground(canvas), undefined, view.title);
		if (copiedImage) setTimeout(() => (copiedImage = null), 1500);
	}
</script>

<section class="matrix" data-testid="view-matrix" aria-label={view.title}>
	<div class="mhead">
		{#if showTitle}<h3 class="title">{view.title}</h3>{/if}
		{#if zoomed}<button class="btn ghost sm" data-testid="matrix-view-all" onclick={viewAll} title="Show the whole matrix (A, or a double-click)">View All</button>{/if}
		<button class="btn ghost sm copyimg" data-testid="matrix-copy-image" onclick={copyImage} title="Copy the matrix as an image">{copiedImage === 'copied' ? 'Copied' : copiedImage === 'saved' ? 'Saved as PNG' : 'Copy Image'}</button>
	</div>
	<div class="plot" bind:this={host}>
		<canvas
			bind:this={canvas}
			data-testid="matrix-canvas"
			style:width="{size.width}px"
			style:height="{size.height}px"
			class:selectable={!!onselect}
			onpointerdown={onDown}
			onpointermove={onMove}
			onpointerup={onUp}
			onpointercancel={onUp}
			onpointerleave={() => (hover = null)}
			onclick={onClick}
			ondblclick={viewAll}
			oncontextmenu={(e) => e.preventDefault()}
			onkeydown={onKey}
			tabindex={0}
			title="Wheel to zoom, over an axis that axis alone; drag to pan; right-drag zooms; double-click or A shows it all"
			aria-label="{view.value_label} of {ROWS} {view.rows.label} rows by {COLUMNS} {view.columns.label} columns{onselect && selected
				? `. Showing ${view.rows.ticks[selected.row]} against ${view.columns.ticks[selected.column]}; click a cell or use the arrow keys for another pair`
				: ''}"
		></canvas>
	</div>
	<div class="bottom">
		<div class="legend" aria-label="Color legend">
			<div class="ltitle">{view.value_label}</div>
			<div class="bar" style:background={colormapGradient()}></div>
			<div class="ticks num">
				{#each legendTicks as t, i (i)}<span>{t}</span>{/each}
			</div>
		</div>
		<div class="readout num" data-testid="matrix-readout" aria-live="polite">
			{#if hover}
				{view.rows.label} {view.rows.ticks[hover.row]} · {view.columns.label} {view.columns.ticks[hover.column]} ·
				{view.value_label} {cellText(value(hover.row, hover.column))}
			{/if}
		</div>
	</div>
</section>

<style>
	.mhead {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-2);
	}

	.matrix {
		display: flex;
		flex-direction: column;
		height: 100%;
		min-height: 260px;
		padding: var(--sp-2) var(--sp-3) var(--sp-3);
		background: var(--bg-plot);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-md);
	}

	.title {
		margin-bottom: var(--sp-2);
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
	}

	.plot {
		position: relative;
		flex: 1;
		min-height: 0;
	}

	canvas {
		position: absolute;
		inset: 0;
		display: block;
	}

	canvas.selectable {
		cursor: pointer;
	}

	.bottom {
		display: flex;
		align-items: flex-end;
		justify-content: space-between;
		gap: var(--sp-3);
		padding-top: var(--sp-2);
	}

	.legend {
		width: 200px;
	}

	.ltitle {
		margin-bottom: 4px;
		font-size: var(--text-2xs);
		color: var(--text-2);
	}

	.bar {
		height: 9px;
		border-radius: 2px;
		box-shadow: inset 0 0 0 1px rgba(0, 0, 0, 0.25);
	}

	.ticks {
		display: flex;
		justify-content: space-between;
		margin-top: 3px;
		font-size: var(--text-2xs);
		color: var(--text-2);
	}

	.readout {
		font-size: var(--text-xs);
		color: var(--text-2);
		text-align: right;
	}
</style>
