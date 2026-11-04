/**
 * The brackets down the tree's left edge that join the objects of one object group, as Visual
 * Dynamics paints them (`gui/project_tree.py`): the Basis blue and bold, each other group a color
 * of its own from the curve palette (never the Basis blue), neighboring groups interleaved on two
 * columns so they read apart, and a named group's name along its line. Brackets never nest: the
 * tree lists a group's members together (`order.ts`), as VD's does.
 */

import type { ProjectSummary } from '$lib/protocol/types';

/** Visual Dynamics' curve palette; the first is the Basis blue. */
export const CURVE_COLORS = ['#4c92d9', '#ff8c2b', '#3fb950', '#e5534b', '#a371f7', '#b07d62', '#e668c3', '#8b949e', '#d2c14e', '#39c5cf'];
export const BASIS_COLOR = CURVE_COLORS[0];

export interface Span {
	/** The members in the tree's order. */
	rows: string[];
	color: string;
	bold: boolean;
	name: string;
	/** 0 or 1: which of the two gutter columns the bracket runs in; neighbors alternate. */
	column: number;
}

/** The spans to paint for `links`, given the tree's row `order`; groups with one row shown are left out. */
export function spans(links: ProjectSummary['links'] | undefined, order: string[]): Span[] {
	if (!links) return [];
	let others = 0;
	const out: Span[] = [];
	for (const group of links) {
		const basis = group.role === 'Basis';
		// a color per group as VD gives them, counted over every group so colors stay put
		const color = basis ? BASIS_COLOR : CURVE_COLORS[1 + (others++ % (CURVE_COLORS.length - 1))];
		const rows = order.filter((name) => group.members.includes(name));
		if (rows.length < 2) continue;
		out.push({ rows, color, bold: basis, name: group.name, column: 0 });
	}
	out.sort((a, b) => order.indexOf(a.rows[0]) - order.indexOf(b.rows[0]));
	out.forEach((span, k) => (span.column = k % 2));
	return out;
}

/** The group `name` is in, by its bracket color, or null. */
export function colorOf(name: string, painted: Span[]): string | null {
	return painted.find((s) => s.rows.includes(name))?.color ?? null;
}
