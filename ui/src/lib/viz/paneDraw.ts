/**
 * Canvas drawing for what a line pane puts over its curves: markers along a line, labelled
 * lines and points, and boxes of text (the legend, a readout, a note) for an exported image.
 * Sizes are given in CSS pixels and scaled by `dpr` here.
 */

import type { Corner } from './paneOptions';
import type { MarkerShape } from './roles';

/** A marker centred at (x, y), canvas pixels, `r` its half-size in canvas pixels. */
export function drawMarker(ctx: CanvasRenderingContext2D, shape: MarkerShape, x: number, y: number, r: number): void {
	ctx.beginPath();
	switch (shape) {
		case 'circle':
			ctx.arc(x, y, r, 0, Math.PI * 2);
			break;
		case 'square':
			ctx.rect(x - r * 0.85, y - r * 0.85, r * 1.7, r * 1.7);
			break;
		case 'triangle':
			ctx.moveTo(x, y - r);
			ctx.lineTo(x + r, y + r * 0.8);
			ctx.lineTo(x - r, y + r * 0.8);
			ctx.closePath();
			break;
		case 'diamond':
			ctx.moveTo(x, y - r);
			ctx.lineTo(x + r, y);
			ctx.lineTo(x, y + r);
			ctx.lineTo(x - r, y);
			ctx.closePath();
			break;
		case 'cross':
			ctx.moveTo(x - r, y - r);
			ctx.lineTo(x + r, y + r);
			ctx.moveTo(x + r, y - r);
			ctx.lineTo(x - r, y + r);
			break;
	}
	ctx.stroke();
}

/** An SVG path for a marker centred at (cx, cy), for a legend swatch. */
export function markerSvgPath(shape: MarkerShape, cx: number, cy: number, r: number): string {
	switch (shape) {
		case 'circle':
			return `M${cx - r},${cy}a${r},${r} 0 1,0 ${2 * r},0a${r},${r} 0 1,0 ${-2 * r},0`;
		case 'square':
			return `M${cx - r * 0.85},${cy - r * 0.85}h${r * 1.7}v${r * 1.7}h${-r * 1.7}z`;
		case 'triangle':
			return `M${cx},${cy - r}L${cx + r},${cy + r * 0.8}L${cx - r},${cy + r * 0.8}z`;
		case 'diamond':
			return `M${cx},${cy - r}L${cx + r},${cy}L${cx},${cy + r}L${cx - r},${cy}z`;
		case 'cross':
			return `M${cx - r},${cy - r}L${cx + r},${cy + r}M${cx + r},${cy - r}L${cx - r},${cy + r}`;
	}
}

/** Text with a halo of the background behind it, so it reads over the lines. */
export function haloText(ctx: CanvasRenderingContext2D, text: string, x: number, y: number, ink: string, halo: string, dpr: number): void {
	ctx.lineJoin = 'round';
	ctx.lineWidth = 3 * dpr;
	ctx.strokeStyle = halo;
	ctx.strokeText(text, x, y);
	ctx.fillStyle = ink;
	ctx.fillText(text, x, y);
}

/** One row of a box: a swatch (a line in a style) before the text, or text alone. */
export interface BoxRow {
	text: string;
	swatch?: { color: string; dash: readonly number[]; width: number; marker: MarkerShape | null };
}

export interface BoxColors {
	ink: string;
	muted: string;
	fill: string;
	edge: string;
}

/** The area a box sits in, canvas pixels. */
export interface Area {
	left: number;
	top: number;
	width: number;
	height: number;
}

/**
 * A box of rows in a corner of `area` (or at `at`, its top-left), as the pane draws its legend,
 * readout and notes on screen. Returns the box's rectangle.
 */
export function drawBox(ctx: CanvasRenderingContext2D, rows: BoxRow[], where: Corner | { x: number; y: number }, area: Area, colors: BoxColors, font: string, dpr: number): Area | null {
	if (!rows.length) return null;
	const pad = 6 * dpr;
	const line = 15 * dpr;
	const sw = 26 * dpr;
	ctx.save();
	ctx.font = font;
	ctx.textBaseline = 'middle';
	const width = Math.max(...rows.map((r) => ctx.measureText(r.text).width + (r.swatch ? sw + 6 * dpr : 0))) + pad * 2;
	const height = rows.length * line + pad * 2 - 2 * dpr;
	const margin = 6 * dpr;
	let x: number;
	let y: number;
	if (typeof where === 'string') {
		x = where.endsWith('left') ? area.left + margin : area.left + area.width - width - margin;
		y = where.startsWith('top') ? area.top + margin : area.top + area.height - height - margin;
	} else {
		x = where.x;
		y = where.y;
	}
	ctx.fillStyle = colors.fill;
	ctx.strokeStyle = colors.edge;
	ctx.lineWidth = dpr;
	ctx.beginPath();
	ctx.rect(x + 0.5, y + 0.5, width, height);
	ctx.fill();
	ctx.stroke();
	rows.forEach((r, k) => {
		const cy = y + pad + line * k + line / 2 - dpr;
		let tx = x + pad;
		if (r.swatch) {
			ctx.strokeStyle = r.swatch.color;
			ctx.lineWidth = (r.swatch.width + 0.4) * dpr;
			ctx.setLineDash(r.swatch.dash.map((d) => d * dpr));
			ctx.beginPath();
			ctx.moveTo(tx, cy);
			ctx.lineTo(tx + sw, cy);
			ctx.stroke();
			ctx.setLineDash([]);
			if (r.swatch.marker) {
				ctx.lineWidth = 1.2 * dpr;
				drawMarker(ctx, r.swatch.marker, tx + sw / 2, cy, 3.2 * dpr);
			}
			tx += sw + 6 * dpr;
		}
		ctx.fillStyle = r.swatch ? colors.ink : k === 0 ? colors.ink : colors.muted;
		ctx.fillText(r.text, tx, cy);
	});
	ctx.restore();
	return { left: x, top: y, width, height };
}
