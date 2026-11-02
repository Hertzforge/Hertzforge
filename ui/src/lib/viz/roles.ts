/**
 * Series roles: line style first, color second, so no meaning rests on hue alone.
 * measured solid, reference dashed, tolerance dotted, abort dash-dot, limit dash-dot-dot (a
 * ceiling the controller holds a channel under; it never aborts), drive thin solid.
 */

import type { Role, Series } from '$lib/protocol/types';

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
	limit: { dash: [10, 3, 1.5, 3, 1.5, 3], width: 1.7, colorVar: '--series-limit', styleName: 'dash-dot-dot', z: 3 },
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

/** How one series of a view is drawn. */
export interface SeriesLook {
	style: RoleStyle;
	colorVar: string;
	/** A follower: drawn, but not listed in the legend. */
	follows: boolean;
}

/** A follower's dash: the reference role's, so it reads as dashed beside its solid measurement. */
const FOLLOWER_DASH = ROLE_STYLES.reference.dash;

/**
 * The look of each of a view's series. Series step through their role's colors in order; a
 * follower (`follows`, an earlier series that is not itself a follower) takes its measurement's
 * color dashed and draws just above it, as Visual Dynamics draws a synthesized FRF. A `follows`
 * that names no such series is ignored.
 */
export function seriesLooks(series: Pick<Series, 'role' | 'follows'>[]): SeriesLook[] {
	const counts: Partial<Record<Role, number>> = {};
	const looks: SeriesLook[] = [];
	series.forEach((s, i) => {
		const followed = s.follows !== undefined && s.follows < i ? looks[s.follows] : undefined;
		if (followed && !followed.follows) {
			const style = { ...followed.style, dash: FOLLOWER_DASH, styleName: 'dashed', z: followed.style.z + 0.5 };
			looks.push({ style, colorVar: followed.colorVar, follows: true });
			return;
		}
		const nth = (counts[s.role] = (counts[s.role] ?? -1) + 1);
		looks.push({ style: ROLE_STYLES[s.role] ?? ROLE_STYLES.other, colorVar: roleColorVar(s.role, nth), follows: false });
	});
	return looks;
}
