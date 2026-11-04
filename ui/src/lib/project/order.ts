/**
 * The tree's order, as Visual Dynamics' `_object_order` gives it: the linked objects first, group
 * by group (the Basis above the others, then the groups as the project lists them), each group's
 * members by kind, then every object in no group, by kind. A group's members sit together, so a
 * link bracket runs down consecutive rows and two brackets never nest (Brandon, 2026-11-04).
 */

import type { ProjectSummary } from '$lib/protocol/types';
import { kindRank } from './icons';

type Listed = ProjectSummary['objects'][number];

/** `listed` in the tree's order under `links`; an object in two groups is listed once. */
export function treeOrder(listed: Listed[], links: ProjectSummary['links'] | undefined): Listed[] {
	const byName = new Map(listed.map((o, i) => [o.name, { o, i }]));
	const byKind = (a: { o: Listed; i: number }, b: { o: Listed; i: number }) => kindRank(a.o.kind) - kindRank(b.o.kind) || a.i - b.i;
	const groups = [...(links ?? [])].sort((a, b) => Number(b.role === 'Basis') - Number(a.role === 'Basis'));
	const out: Listed[] = [];
	const placed = new Set<string>();
	for (const group of groups) {
		const members = group.members
			.map((name) => byName.get(name))
			.filter((m): m is { o: Listed; i: number } => !!m && !placed.has(m.o.name))
			.sort(byKind);
		for (const m of members) {
			placed.add(m.o.name);
			out.push(m.o);
		}
	}
	const rest = listed
		.map((o, i) => ({ o, i }))
		.filter((m) => !placed.has(m.o.name))
		.sort(byKind);
	return [...out, ...rest.map((m) => m.o)];
}
