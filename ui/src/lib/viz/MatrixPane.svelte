<script lang="ts">
	/**
	 * One colored cell per value (a MAC, say), drawn on a 2-D canvas: a few thousand cells at most,
	 * redrawn only on resize, theme change or hover. Color is the sequential colormap over the
	 * view's pinned range, with a numeric legend; each cell writes its value when it is large
	 * enough to hold the text. Hovering reads the cell's row, column and value. A matrix that
	 * `selects` a shape view marks the chosen cell in the overlay's orange; a click or the arrow
	 * keys choose another.
	 */
	import { onDestroy, onMount } from 'svelte';
	import type { MatrixView } from '$lib/protocol/types';
	import { cssVar, theme } from '$lib/theme/theme.svelte';
	import { colormap, colormapGradient } from './colormap';
	import { cellAt, cellText, inkOn, matrixLayout, type MatrixLayout } from './matrix';

	interface Props {
		view: MatrixView;
		showTitle?: boolean;
		/** The cell whose pair the linked shape view shows. */
		selected?: { row: number; column: number } | null;
		onselect?: (cell: { row: number; column: number }) => void;
	}

	let { view, showTitle = true, selected = null, onselect }: Props = $props();

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
		let l = matrixLayout(size.width, size.height, ROWS, COLUMNS, LEFT, BOTTOM);
		const tilted = widest + 8 > l.cellW;
		if (tilted) l = matrixLayout(size.width, size.height, ROWS, COLUMNS, LEFT, widest * Math.SQRT1_2 + 34);
		layout = l;
		const text2 = cssVar('--text-2');
		const text3 = cssVar('--text-3');
		const rgb: [number, number, number] = [0, 0, 0];
		const span = high - low || 1;
		const showText = l.cellW >= 34 && l.cellH >= 16;
		ctx.font = `11px ${mono}`;
		ctx.textAlign = 'center';
		ctx.textBaseline = 'middle';
		for (let r = 0; r < ROWS; r++) {
			for (let c = 0; c < COLUMNS; c++) {
				const v = value(r, c);
				colormap((v - low) / span, rgb);
				ctx.fillStyle = Number.isFinite(v)
					? `rgb(${rgb[0] * 255} ${rgb[1] * 255} ${rgb[2] * 255})`
					: cssVar('--bg-3');
				const x = l.x0 + c * l.cellW;
				const y = l.y0 + r * l.cellH;
				ctx.fillRect(x, y, Math.ceil(l.cellW), Math.ceil(l.cellH));
				if (showText) {
					ctx.fillStyle = inkOn(rgb) === 'dark' ? '#111' : '#f4f4f4';
					ctx.fillText(cellText(v), x + l.cellW / 2, y + l.cellH / 2);
				}
			}
		}
		if (hover) {
			ctx.strokeStyle = cssVar('--text-1');
			ctx.lineWidth = 2;
			ctx.strokeRect(l.x0 + hover.column * l.cellW + 1, l.y0 + hover.row * l.cellH + 1, l.cellW - 2, l.cellH - 2);
		}
		if (selected) {
			// the overlay's orange, so the cell reads as the pair the shape view draws
			ctx.strokeStyle = '#e69f00';
			ctx.lineWidth = 3;
			ctx.strokeRect(l.x0 + selected.column * l.cellW + 1.5, l.y0 + selected.row * l.cellH + 1.5, l.cellW - 3, l.cellH - 3);
		}
		// ticks, thinned to every nth where they would overlap
		ctx.fillStyle = text2;
		ctx.font = `10.5px ${mono}`;
		const rowEvery = Math.max(1, Math.ceil(13 / l.cellH));
		ctx.textAlign = 'right';
		for (let r = 0; r < ROWS; r += rowEvery) {
			ctx.fillText(view.rows.ticks[r], l.x0 - 6, l.y0 + (r + 0.5) * l.cellH);
		}
		const below = l.y0 + ROWS * l.cellH + 5;
		const columnEvery = Math.max(1, Math.ceil((tilted ? 13 * Math.SQRT2 : widest + 8) / l.cellW));
		for (let c = 0; c < COLUMNS; c += columnEvery) {
			const x = l.x0 + (c + 0.5) * l.cellW;
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
		ctx.fillText(view.columns.label, l.x0 + (COLUMNS * l.cellW) / 2, below + (tilted ? widest * Math.SQRT1_2 + 8 : 16));
		ctx.save();
		ctx.translate(12, l.y0 + (ROWS * l.cellH) / 2);
		ctx.rotate(-Math.PI / 2);
		ctx.textBaseline = 'middle';
		ctx.fillText(view.rows.label, 0, 0);
		ctx.restore();
	}

	function onMove(e: PointerEvent) {
		if (!layout) return;
		const box = canvas.getBoundingClientRect();
		const at = cellAt(e.clientX - box.left, e.clientY - box.top, layout);
		if (at?.row !== hover?.row || at?.column !== hover?.column) hover = at;
	}

	onMount(() => {
		resizeObs = new ResizeObserver(([entry]) => {
			size = { width: entry.contentRect.width, height: entry.contentRect.height };
		});
		resizeObs.observe(host);
	});

	function onClick(e: MouseEvent) {
		if (!layout || !onselect) return;
		const box = canvas.getBoundingClientRect();
		const at = cellAt(e.clientX - box.left, e.clientY - box.top, layout);
		if (at) onselect(at);
	}

	function onKey(e: KeyboardEvent) {
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
		void theme.version;
		void view;
		draw();
	});
</script>

<section class="matrix" data-testid="view-matrix" aria-label={view.title}>
	{#if showTitle}<h3 class="title">{view.title}</h3>{/if}
	<div class="plot" bind:this={host}>
		<canvas
			bind:this={canvas}
			data-testid="matrix-canvas"
			style:width="{size.width}px"
			style:height="{size.height}px"
			class:selectable={!!onselect}
			onpointermove={onMove}
			onpointerleave={() => (hover = null)}
			onclick={onClick}
			onkeydown={onKey}
			tabindex={onselect ? 0 : undefined}
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
