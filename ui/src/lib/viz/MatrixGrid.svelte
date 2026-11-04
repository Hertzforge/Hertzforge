<script lang="ts">
	/**
	 * An n by n grid of small plots on one canvas: the MIMO matrix grid. The diagonal shows each
	 * control's spectrum against its reference with its alarm and abort lines, drawn as steps over
	 * the service's bins; above the diagonal the coherence of the two controls (0 to 1) against the
	 * reference's, below it their phase against the reference's, or on both sides the
	 * cross-spectrum magnitude. Every cell shares the frequency axis, and the cursor is one
	 * frequency across all of them; the readout above names the cell and gives its numbers.
	 *
	 * Keyboard: the arrow keys move the selected cell, `[` and `]` move the cursor's frequency,
	 * Enter opens the cell full size, P pins or unpins the selected row's control, Page Up and Page
	 * Down scroll the window. Line style carries each role (solid measured, dashed reference,
	 * dotted alarm, dash-dot abort); color only follows.
	 */
	import type { SdmView, SpectrumView } from '$lib/protocol/types';
	import { cssVar, theme } from '$lib/theme/theme.svelte';
	import { formatFixed, formatNumber, formatTick } from './format';
	import { logTicks } from './logTicks';
	import {
		cellAtPoint,
		CELL_WORDS,
		curveValueAt,
		frac,
		frequencyRange,
		gridCells,
		gridGeometry,
		prepareGrid,
		unfrac,
		type CellData,
		type OffDiagonal,
		type PreparedCell
	} from './matrixGrid';
	import { ROLE_STYLES, roleColorVar } from './roles';

	interface Props {
		/** The window's controls, by name, in window order. */
		labels: string[];
		/** Which window controls are pinned, by window index. */
		pinned?: boolean[];
		sdm: SdmView | null;
		/** Each window control's own spectrum, or null until it comes. */
		spectra: (SpectrumView | null)[];
		off?: OffDiagonal;
		/** The selected cell. */
		selected?: { r: number; c: number } | null;
		/** The unit of a control's spectrum, for the readout. */
		unit?: string;
		onopen?: (r: number, c: number) => void;
		onpin?: (k: number) => void;
		onscroll?: (by: number) => void;
	}
	let { labels, pinned = [], sdm, spectra, off = 'coh_phase', selected = $bindable(null), unit = 'g²/Hz', onopen, onpin, onscroll }: Props = $props();

	const GUTTER = { left: 96, top: 26, bottom: 30 };

	let host = $state<HTMLDivElement | null>(null);
	let canvas = $state<HTMLCanvasElement | null>(null);
	let width = $state(0);
	let height = $state(0);
	let hover = $state<{ r: number; c: number } | null>(null);
	let cursorF = $state<number | null>(null);
	let drawMs = $state(0);

	const n = $derived(labels.length);
	const cells = $derived<CellData[]>(gridCells(n, off, sdm, spectra));
	const xr = $derived(frequencyRange(cells));
	const geom = $derived(gridGeometry(width, height, n, GUTTER));
	const focus = $derived(hover ?? selected);

	/** The cell the readout describes and its numbers at the cursor's frequency. */
	const reading = $derived.by(() => {
		if (!focus || cursorF === null) return null;
		const cell = cells.find((x) => x.r === focus.r && x.c === focus.c);
		if (!cell) return null;
		const val = (role: string, which = 0) => {
			const cv = cell.curves.filter((c) => c.role === role)[which];
			return cv ? curveValueAt(cv, cursorF!) : null;
		};
		const fmt = (v: number | null) => (v === null ? '-' : cell.kind === 'coh' ? formatFixed(v, 3) : cell.kind === 'phase' ? `${formatFixed(v, 3)} deg` : `${formatNumber(v, 3)}`);
		const name = cell.r === cell.c ? labels[cell.r] : `${labels[cell.r]} with ${labels[cell.c]}`;
		const parts: string[] = [`${CELL_WORDS[cell.kind]} ${fmt(val('measured'))}${cell.kind === 'psd' || cell.kind === 'csd' ? ` ${unit}` : ''}`, `reference ${fmt(val('reference'))}`];
		if (cell.kind === 'psd') {
			const tol = cell.curves.filter((c) => c.role === 'tolerance').map((c) => curveValueAt(c, cursorF!));
			const ab = cell.curves.filter((c) => c.role === 'abort').map((c) => curveValueAt(c, cursorF!));
			if (tol.length === 2 && tol[0] !== null && tol[1] !== null) parts.push(`alarm ${formatNumber(Math.min(tol[0], tol[1]), 3)} to ${formatNumber(Math.max(tol[0], tol[1]), 3)}`);
			if (ab.length === 2 && ab[0] !== null && ab[1] !== null) parts.push(`abort ${formatNumber(Math.min(ab[0], ab[1]), 3)} to ${formatNumber(Math.max(ab[0], ab[1]), 3)}`);
		}
		return { name, f: cursorF, parts };
	});

	let frame = 0;
	function schedule() {
		if (frame) return;
		frame = requestAnimationFrame(() => {
			frame = 0;
			draw();
		});
	}

	function strokeFor(role: keyof typeof ROLE_STYLES) {
		return cssVar(roleColorVar(role, 0));
	}

	function draw() {
		if (!canvas || !width || !height) return;
		const t0 = performance.now();
		const dpr = Math.min(2, devicePixelRatio || 1);
		if (canvas.width !== Math.round(width * dpr) || canvas.height !== Math.round(height * dpr)) {
			canvas.width = Math.round(width * dpr);
			canvas.height = Math.round(height * dpr);
			canvas.style.width = `${width}px`;
			canvas.style.height = `${height}px`;
		}
		const ctx = canvas.getContext('2d');
		if (!ctx) return;
		ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
		ctx.clearRect(0, 0, width, height);
		if (!n || geom.cell < 8) return;

		const prepared: PreparedCell[] = prepareGrid(cells, geom, xr);
		const grid = cssVar('--plot-grid');
		const line1 = cssVar('--line-1');
		const line3 = cssVar('--line-3');
		const text1 = cssVar('--text-1');
		const text2 = cssVar('--text-2');
		const axisColor = cssVar('--plot-axis');
		const fontUi = cssVar('--font-ui');
		const fontMono = cssVar('--font-mono');
		const cell = geom.cell;
		const cellH = geom.cellH;
		const ticks = logTicks(xr.lo, xr.hi, [1]);

		for (const p of prepared) {
			const { rect } = p;
			const diag = p.cell.r === p.cell.c;
			// frame and decade lines
			ctx.fillStyle = cssVar(diag ? '--bg-3' : '--bg-plot');
			ctx.fillRect(rect.x, rect.y, rect.w, rect.h);
			ctx.strokeStyle = grid;
			ctx.lineWidth = 1;
			ctx.beginPath();
			for (const t of ticks) {
				const x = Math.round(rect.x + frac(t, xr) * rect.w) + 0.5;
				ctx.moveTo(x, rect.y);
				ctx.lineTo(x, rect.y + rect.h);
			}
			if (p.cell.kind === 'coh') {
				const y = Math.round(rect.y + rect.h - frac(1, p.yr) * rect.h) + 0.5;
				ctx.moveTo(rect.x, y);
				ctx.lineTo(rect.x + rect.w, y);
			} else if (p.cell.kind === 'phase') {
				const y = Math.round(rect.y + rect.h / 2) + 0.5;
				ctx.moveTo(rect.x, y);
				ctx.lineTo(rect.x + rect.w, y);
			}
			ctx.stroke();

			// curves, low roles first so the measured line stays on top
			const order = [...p.paths].sort((a, b) => ROLE_STYLES[a.role].z - ROLE_STYLES[b.role].z);
			ctx.save();
			ctx.beginPath();
			ctx.rect(rect.x, rect.y, rect.w, rect.h);
			ctx.clip();
			for (const path of order) {
				const st = ROLE_STYLES[path.role] ?? ROLE_STYLES.other;
				ctx.strokeStyle = strokeFor(path.role);
				ctx.lineWidth = Math.max(1, st.width * (cell < 90 ? 0.8 : 1));
				ctx.setLineDash(st.dash);
				ctx.beginPath();
				let pen = false;
				const pts = path.pts;
				for (let i = 0; i < pts.length; i += 2) {
					const x = pts[i];
					const y = pts[i + 1];
					if (Number.isNaN(x) || Number.isNaN(y)) {
						pen = false;
						continue;
					}
					if (pen) ctx.lineTo(x, y);
					else ctx.moveTo(x, y);
					pen = true;
				}
				ctx.stroke();
			}
			ctx.setLineDash([]);
			ctx.restore();

			ctx.strokeStyle = diag ? line3 : line1;
			ctx.lineWidth = diag ? 1.5 : 1;
			ctx.strokeRect(rect.x + 0.5, rect.y + 0.5, rect.w - 1, rect.h - 1);
			if (!p.cell.curves.length) {
				ctx.fillStyle = text2;
				ctx.font = `11px ${fontUi}`;
				ctx.textAlign = 'center';
				ctx.textBaseline = 'middle';
				ctx.fillText('waiting', rect.x + rect.w / 2, rect.y + rect.h / 2);
			}
		}

		// the cursor frequency across every cell
		if (cursorF !== null && cursorF >= xr.lo && cursorF <= xr.hi) {
			ctx.strokeStyle = cssVar('--plot-cursor');
			ctx.lineWidth = 1;
			ctx.beginPath();
			for (let c = 0; c < n; c++) {
				const x = Math.round(geom.x0 + c * (cell + geom.gap) + frac(cursorF, xr) * cell) + 0.5;
				ctx.moveTo(x, geom.y0);
				ctx.lineTo(x, geom.y0 + n * (cellH + geom.gap) - geom.gap);
			}
			ctx.stroke();
		}

		// the selected and hovered cells
		const outline = (r: number, c: number, w: number, dash: number[]) => {
			const x = geom.x0 + c * (cell + geom.gap);
			const y = geom.y0 + r * (cellH + geom.gap);
			ctx.strokeStyle = text1;
			ctx.lineWidth = w;
			ctx.setLineDash(dash);
			ctx.strokeRect(x - 1.5, y - 1.5, cell + 3, cellH + 3);
			ctx.setLineDash([]);
		};
		if (selected) outline(selected.r, selected.c, 2, []);
		if (hover && (!selected || hover.r !== selected.r || hover.c !== selected.c)) outline(hover.r, hover.c, 1, [3, 2]);

		// names: rows on the left, columns on top; a pinned control carries a pin mark
		ctx.textBaseline = 'middle';
		const size = cell < 70 ? 11 : 12;
		for (let k = 0; k < n; k++) {
			const y = geom.y0 + k * (cellH + geom.gap) + cellH / 2;
			const x = geom.x0 + k * (cell + geom.gap) + cell / 2;
			const strong = focus && (focus.r === k || focus.c === k);
			ctx.font = `${strong ? 640 : 400} ${size}px ${fontUi}`;
			ctx.fillStyle = strong ? text1 : text2;
			ctx.textAlign = 'right';
			const pinW = pinned[k] ? 12 : 0;
			ctx.fillText(fit(ctx, labels[k], GUTTER.left - 12 - pinW), GUTTER.left - 8 - pinW, y);
			if (pinned[k]) pinMark(ctx, GUTTER.left - 14, y, text2);
			ctx.textAlign = 'center';
			ctx.fillText(fit(ctx, labels[k], cell - 4), x, GUTTER.top - 12);
		}

		// the shared frequency axis under the last row
		const yAxis = geom.y0 + n * (cellH + geom.gap) - geom.gap;
		ctx.fillStyle = axisColor;
		ctx.strokeStyle = axisColor;
		ctx.font = `10.5px ${fontMono}`;
		ctx.textAlign = 'center';
		ctx.textBaseline = 'top';
		for (let c = 0; c < n; c++) {
			const x0 = geom.x0 + c * (cell + geom.gap);
			let lastRight = -Infinity;
			for (const t of ticks) {
				const x = x0 + frac(t, xr) * cell;
				const label = formatTick(t);
				const w = ctx.measureText(label).width;
				if (x - w / 2 < lastRight + 4 || x < x0 - 1 || x > x0 + cell + 1) continue;
				ctx.beginPath();
				ctx.moveTo(x, yAxis);
				ctx.lineTo(x, yAxis + 3);
				ctx.stroke();
				ctx.fillText(label, x, yAxis + 5);
				lastRight = x + w / 2;
			}
		}
		ctx.textAlign = 'left';
		ctx.font = `11px ${fontUi}`;
		ctx.fillText('Hz', geom.x0 + n * (cell + geom.gap) - geom.gap + 4, yAxis + 5);
		drawMs = performance.now() - t0;
	}

	function pinMark(ctx: CanvasRenderingContext2D, x: number, y: number, color: string) {
		ctx.save();
		ctx.fillStyle = color;
		ctx.beginPath();
		ctx.arc(x, y - 2, 3, 0, Math.PI * 2);
		ctx.fill();
		ctx.fillRect(x - 0.6, y, 1.2, 5);
		ctx.restore();
	}

	function fit(ctx: CanvasRenderingContext2D, s: string, w: number): string {
		if (w <= 0) return '';
		if (ctx.measureText(s).width <= w) return s;
		let t = s;
		while (t.length > 1 && ctx.measureText(`${t}...`).width > w) t = t.slice(0, -1);
		return `${t}...`;
	}

	$effect(() => {
		void cells;
		void width;
		void height;
		void hover;
		void selected;
		void cursorF;
		void pinned;
		void theme.version;
		schedule();
	});

	$effect(() => {
		if (!host) return;
		const measure = () => {
			width = host!.clientWidth;
			height = host!.clientHeight;
		};
		const obs = new ResizeObserver(measure);
		obs.observe(host);
		measure();
		return () => {
			obs.disconnect();
			if (frame) cancelAnimationFrame(frame);
		};
	});

	function pointAt(e: MouseEvent) {
		if (!canvas) return null;
		const r = canvas.getBoundingClientRect();
		return cellAtPoint(geom, e.clientX - r.left, e.clientY - r.top);
	}

	function onMove(e: MouseEvent) {
		const at = pointAt(e);
		hover = at ? { r: at.r, c: at.c } : null;
		if (at) cursorF = unfrac(at.fx, xr);
	}

	function onClick(e: MouseEvent) {
		const at = pointAt(e);
		if (!at) return;
		selected = { r: at.r, c: at.c };
		cursorF = unfrac(at.fx, xr);
		onopen?.(at.r, at.c);
	}

	function onKey(e: KeyboardEvent) {
		if (!n) return;
		const s = selected ?? { r: 0, c: 0 };
		const stepF = (by: number) => {
			const t = cursorF === null ? 0.5 : frac(cursorF, xr);
			cursorF = unfrac(Math.min(1, Math.max(0, t + by / Math.max(16, geom.cell))), xr);
		};
		switch (e.key) {
			case 'ArrowUp':
				selected = { r: Math.max(0, s.r - 1), c: s.c };
				break;
			case 'ArrowDown':
				selected = { r: Math.min(n - 1, s.r + 1), c: s.c };
				break;
			case 'ArrowLeft':
				selected = { r: s.r, c: Math.max(0, s.c - 1) };
				break;
			case 'ArrowRight':
				selected = { r: s.r, c: Math.min(n - 1, s.c + 1) };
				break;
			case '[':
				stepF(e.shiftKey ? -10 : -1);
				break;
			case ']':
				stepF(e.shiftKey ? 10 : 1);
				break;
			case 'Enter':
				onopen?.(s.r, s.c);
				break;
			case 'p':
			case 'P':
				onpin?.(s.r);
				break;
			case 'PageDown':
				onscroll?.(1);
				break;
			case 'PageUp':
				onscroll?.(-1);
				break;
			default:
				return;
		}
		if (cursorF === null) cursorF = unfrac(0.5, xr);
		hover = null;
		e.preventDefault();
	}
</script>

<div class="grid" data-testid="matrix-grid" data-n={n} data-draw-ms={drawMs.toFixed(2)}>
	<div class="readout num" aria-live="polite" data-testid="grid-readout">
		{#if reading}
			<span class="who">{reading.name}</span>
			<span>{formatFixed(reading.f, 4)} Hz</span>
			{#each reading.parts as part, i (i)}<span class:val={i === 0}>{part}</span>{/each}
		{:else}
			<span class="dim">Point at a cell to read it; click it, or select it with the arrow keys and press Enter, to open it full size.</span>
		{/if}
	</div>
	<div class="canvasbox" bind:this={host}>
		<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
		<canvas
			bind:this={canvas}
			tabindex="0"
			aria-label="Matrix grid of {n} controls: spectra on the diagonal, {off === 'csd' ? 'cross-spectrum magnitude off it' : 'coherence above and phase below'}. Arrow keys select a cell, Enter opens it, P pins its row, Page Up and Page Down scroll."
			data-testid="grid-canvas"
			onmousemove={onMove}
			onmouseleave={() => (hover = null)}
			onclick={onClick}
			onkeydown={onKey}
		></canvas>
	</div>
	<div class="legend" aria-hidden="true">
		<span><svg width="24" height="8"><line x1="0" y1="4" x2="24" y2="4" class="m" /></svg>measured</span>
		<span><svg width="24" height="8"><line x1="0" y1="4" x2="24" y2="4" class="r" /></svg>reference</span>
		<span><svg width="24" height="8"><line x1="0" y1="4" x2="24" y2="4" class="t" /></svg>alarm</span>
		<span><svg width="24" height="8"><line x1="0" y1="4" x2="24" y2="4" class="a" /></svg>abort</span>
		<span class="sep">{off === 'csd' ? 'Off the diagonal: cross-spectrum magnitude' : 'Above the diagonal: coherence, 0 to 1. Below: phase, -180 to 180 deg.'}</span>
	</div>
</div>

<style>
	.grid {
		display: flex;
		flex-direction: column;
		height: 100%;
		min-height: 280px;
		padding: var(--sp-2);
		background: var(--bg-plot);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-md);
	}

	.readout {
		display: flex;
		flex-wrap: wrap;
		gap: 4px 14px;
		min-height: 22px;
		padding: 0 4px 4px;
		font-size: var(--text-sm);
		color: var(--text-2);
	}

	.readout .who {
		font-family: var(--font-ui);
		font-weight: var(--weight-strong);
		color: var(--text-1);
	}

	.readout .val {
		color: var(--text-1);
	}

	.readout .dim {
		font-family: var(--font-ui);
		color: var(--text-3);
	}

	.canvasbox {
		position: relative;
		flex: 1;
		min-height: 0;
	}

	canvas {
		position: absolute;
		inset: 0;
		display: block;
		cursor: crosshair;
	}

	canvas:focus-visible {
		outline: 2px solid var(--focus);
		outline-offset: 1px;
	}

	.legend {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 4px 14px;
		padding: 4px 4px 0;
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	.legend span {
		display: inline-flex;
		align-items: center;
		gap: 5px;
	}

	.legend line {
		stroke-width: 1.6;
	}

	.legend .m {
		stroke: var(--series-measured);
	}

	.legend .r {
		stroke: var(--series-reference);
		stroke-dasharray: 7 4;
	}

	.legend .t {
		stroke: var(--series-tolerance);
		stroke-dasharray: 1.5 3.5;
	}

	.legend .a {
		stroke: var(--series-abort);
		stroke-dasharray: 9 3.5 1.5 3.5;
	}

	.legend .sep {
		color: var(--text-3);
	}
</style>
