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

/** The links once `moving` is dragged onto `target`, or out of their groups (null), as `moveInto` leaves them: a group of one is no group, and a new one comes last. */
function linksAfterMove(links: ProjectSummary['links'] | undefined, moving: string[], target: string | null): NonNullable<ProjectSummary['links']> {
	const home = target === null ? undefined : (links ?? []).find((g) => g.members.includes(target));
	const after = (links ?? [])
		.map((g) => (g === home ? { ...g, members: [...new Set([...g.members, ...moving])] } : { ...g, members: g.members.filter((m) => !moving.includes(m)) }))
		.filter((g) => g.members.length >= 2);
	if (target !== null && !home) after.push({ members: [target, ...moving], role: null, name: '' });
	return after;
}

/** The row `moving` will land above when dragged onto `target` (null: out of their groups), or null when it lands at the bottom. */
export function landsBefore(listed: Listed[], links: ProjectSummary['links'] | undefined, moving: string[], target: string | null): string | null {
	const order = treeOrder(listed, linksAfterMove(links, moving, target)).map((o) => o.name);
	const first = order.findIndex((name) => moving.includes(name));
	return order.slice(first).find((name) => !moving.includes(name)) ?? null;
}

/** `ids` with `moving` taken out and put before `before`, or last for null; before itself, it stays. */
export function reordered(ids: number[], moving: number, before: number | null): number[] {
	if (before === moving) return ids;
	const rest = ids.filter((id) => id !== moving);
	const at = before === null ? rest.length : rest.indexOf(before);
	return [...rest.slice(0, at < 0 ? rest.length : at), moving, ...rest.slice(at < 0 ? rest.length : at)];
}
