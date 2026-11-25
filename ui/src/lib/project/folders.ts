/**
 * The object groups the tree lists as a row of their own: a group of two or more time histories
 * and nothing else, such as the histories an imported file brings or the copies Filter and Rescale
 * make of several. The row stands above its members, opens and closes on them, picks them all with
 * one click and is renamed by a double-click, so two sets of histories are picked as two rows.
 * A group that holds anything else (a run's channel table, a geometry) keeps its bracket.
 */

import type { ProjectSummary } from '$lib/protocol/types';

type Listed = ProjectSummary['objects'][number];
type Link = NonNullable<ProjectSummary['links']>[number];

export interface Folder {
	/** What the row is kept open or closed by: the group's name, else its first member. */
	key: string;
	/** The group's name, or '' when it has none. */
	name: string;
	/** The members in the tree's order. */
	members: string[];
	/** The group as the summary lists it. */
	group: Link;
}

/** The text a folder row shows: the group's name, or what it holds when it has none. */
export function folderLabel(folder: Folder): string {
	return folder.name || 'Time histories';
}

/**
 * The groups of `links` the tree lists as rows, given the objects and the tree's `order`. Every
 * member must be a time history the summary lists, and the members must sit together in the
 * order, as `treeOrder` puts them, so the row can stand above them.
 */
export function historyFolders(objects: Listed[], links: ProjectSummary['links'] | undefined, order: string[]): Folder[] {
	const kinds = new Map(objects.map((o) => [o.name, o.kind]));
	const out: Folder[] = [];
	for (const group of links ?? []) {
		if (group.members.length < 2 || group.members.some((m) => kinds.get(m) !== 'time_history')) continue;
		const members = order.filter((name) => group.members.includes(name));
		if (members.length !== group.members.length) continue;
		const first = order.indexOf(members[0]);
		if (members.some((name, k) => order[first + k] !== name)) continue;
		out.push({ key: group.name || members[0], name: group.name, members, group });
	}
	return out;
}
