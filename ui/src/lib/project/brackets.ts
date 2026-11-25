/**
 * The brackets down the tree's left edge that join the objects of one object group: the Basis blue
 * and bold, each other group a color
 * of its own from the curve palette (never the Basis blue), neighboring groups interleaved on two
 * columns so they read apart, and a named group's name along its line. Brackets never nest: the
 * tree lists a group's members together (`order.ts`).
 */

import type { ProjectSummary } from '$lib/protocol/types';

/** The curve palette; the first is the Basis blue. */
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

/** Each group's color, in the order of `links`: a color per group, counted over every group so colors stay put. */
function groupColors(links: NonNullable<ProjectSummary['links']>): string[] {
	let others = 0;
	return links.map((group) => (group.role === 'Basis' ? BASIS_COLOR : CURVE_COLORS[1 + (others++ % (CURVE_COLORS.length - 1))]));
}

/**
 * The spans to paint for `links`, given the tree's row `order`; groups with one row shown are left
 * out, and so are the groups in `rowed`, which the tree lists under a row of their own (`folders.ts`).
 * Colors are counted over every group, those included, so no other group's color moves.
 */
export function spans(links: ProjectSummary['links'] | undefined, order: string[], rowed: ReadonlySet<unknown> = new Set()): Span[] {
	if (!links) return [];
	const colors = groupColors(links);
	const out: Span[] = [];
	for (const [k, group] of links.entries()) {
		if (rowed.has(group)) continue;
		const basis = group.role === 'Basis';
		const color = colors[k];
		const rows = order.filter((name) => group.members.includes(name));
		if (rows.length < 2) continue;
		out.push({ rows, color, bold: basis, name: group.name, column: 0 });
	}
	out.sort((a, b) => order.indexOf(a.rows[0]) - order.indexOf(b.rows[0]));
	out.forEach((span, k) => (span.column = k % 2));
	return out;
}

export interface Tick {
	/** The Matched Modes object's row. */
	row: string;
	/** Its sets' group colors, first set's first: one color is a solid tick, two a striped one. */
	colors: string[];
}

/**
 * A short bracket on each Matched Modes object's row, striped in the colors of the groups its two
 * shape sets are in, so the row reads as belonging to both (Brandon, 2026-11-14). One color when
 * both sets share a group or only one is grouped; none when neither is, or when the object is in a
 * group of its own (its group's bracket marks it).
 */
export function ticks(links: ProjectSummary['links'] | undefined, objects: ProjectSummary['objects']): Tick[] {
	if (!links) return [];
	const colors = groupColors(links);
	const colorOf = (name: string) => {
		const k = links.findIndex((g) => g.members.includes(name));
		return k < 0 ? null : colors[k];
	};
	const out: Tick[] = [];
	for (const o of objects) {
		if (!o.between || colorOf(o.name)) continue;
		const found = [...new Set(o.between.map(colorOf).filter((c): c is string => c !== null))];
		if (found.length) out.push({ row: o.name, colors: found });
	}
	return out;
}
