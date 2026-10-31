/**
 * Series roles: line style first, color second, so no meaning rests on hue alone.
 * measured solid, reference dashed, tolerance dotted, abort dash-dot, drive thin solid.
 */

import type { Role } from '$lib/protocol/types';

export interface RoleStyle {
	/** Canvas dash pattern in CSS pixels; empty for solid. */
	dash: number[];
	width: number;
	/** CSS custom property holding the color. */
	colorVar: string;
	/** Short words for the legend, naming the line style. */
	styleName: string;
	/** Draw order: higher draws later (on top). */
	z: number;
}

export const ROLE_STYLES: Record<Role, RoleStyle> = {
	measured: { dash: [], width: 1.6, colorVar: '--series-measured', styleName: 'solid', z: 5 },
	reference: { dash: [7, 4], width: 1.6, colorVar: '--series-reference', styleName: 'dashed', z: 4 },
	tolerance: { dash: [1.5, 3.5], width: 1.6, colorVar: '--series-tolerance', styleName: 'dotted', z: 2 },
	abort: { dash: [9, 3.5, 1.5, 3.5], width: 1.5, colorVar: '--series-abort', styleName: 'dash-dot', z: 3 },
	drive: { dash: [], width: 1, colorVar: '--series-drive', styleName: 'thin solid', z: 1 },
	other: { dash: [], width: 1.4, colorVar: '--series-other', styleName: 'solid', z: 1 }
};

/** Extra measured series in one pane step through the blue family, then gray. */
const MEASURED_VARIANTS = ['--series-measured', '--series-measured-2', '--series-measured-3'];

export function roleColorVar(role: Role, nthOfRole: number): string {
	if (role === 'measured' || role === 'other') return MEASURED_VARIANTS[nthOfRole % MEASURED_VARIANTS.length];
	return ROLE_STYLES[role].colorVar;
}

/** SVG `stroke-dasharray` for a legend swatch. */
export function dashArray(role: Role): string {
	const d = ROLE_STYLES[role].dash;
	return d.length ? d.join(' ') : 'none';
}
