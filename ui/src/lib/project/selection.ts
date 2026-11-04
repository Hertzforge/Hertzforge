/**
 * The project tree's selection: which objects, and within a data object which records, as
 * Visual Dynamics' tree keeps it. A plain click selects only what was clicked, Cmd (Ctrl
 * elsewhere) adds or removes, Shift extends from the anchor through the rows between. Records
 * picked in an object's grid stand for that object: the object is selected with them, and
 * selecting the object's own row releases its picks, so a cell chosen minutes ago never turns a
 * later act on the object into one on that record.
 */

export interface Selection {
	/** The objects selected, in the order they were picked. */
	objects: string[];
	/** The records picked in each selected object's grid, by object; an object absent is whole. */
	records: Record<string, number[]>;
	/** A part of a geometry picked: a family, an element group, a solid, a mate, the nodes. */
	part: { name: string; kind: string; id: string | number } | null;
	/** Where a Shift extension starts: the last row picked without Shift. */
	anchor: string | null;
}

export const EMPTY: Selection = { objects: [], records: {}, part: null, anchor: null };

export interface Modifiers {
	toggle?: boolean;
	extend?: boolean;
}

function without(selection: Selection, name: string): Selection {
	const records = { ...selection.records };
	delete records[name];
	return { ...selection, objects: selection.objects.filter((o) => o !== name), records };
}

/** An object's row clicked. `order` is every object's name as the tree lists them, for Shift. */
export function clickObject(selection: Selection, name: string, order: string[], mods: Modifiers = {}): Selection {
	if (mods.extend && selection.anchor && order.includes(selection.anchor)) {
		const a = order.indexOf(selection.anchor);
		const b = order.indexOf(name);
		// the rows from the anchor through the one clicked, in place of the last extension
		return { ...selection, objects: order.slice(Math.min(a, b), Math.max(a, b) + 1), part: null };
	}
	if (mods.toggle) {
		if (selection.objects.includes(name)) return { ...without(selection, name), part: null, anchor: name };
		return { ...selection, objects: [...selection.objects, name], part: null, anchor: name };
	}
	// the owner row means the whole object: its record picks go
	return { objects: [name], records: {}, part: null, anchor: name };
}

/** A cell of `name`'s grid clicked: record `index`. Shift extends the block from the anchor cell. */
export function clickRecord(
	selection: Selection,
	name: string,
	index: number,
	mods: Modifiers = {},
	block?: { from: number; to: number; grid: number[][] }
): Selection {
	const picked = selection.records[name] ?? [];
	let next: number[];
	if (mods.extend && block) next = Array.from(new Set([...picked, ...rectangle(block.grid, block.from, block.to)]));
	else if (mods.toggle) next = picked.includes(index) ? picked.filter((i) => i !== index) : [...picked, index];
	else next = [index];
	const objects = mods.toggle || mods.extend ? (selection.objects.includes(name) ? selection.objects : [...selection.objects, name]) : [name];
	const records = mods.toggle || mods.extend ? { ...selection.records, [name]: next } : { [name]: next };
	if (next.length === 0) delete records[name];
	return { objects, records, part: null, anchor: name };
}

/** The records in the rectangle of `grid` cells spanned by two record indices; holes left out. */
export function rectangle(grid: number[][], from: number, to: number): number[] {
	const at = (index: number) => {
		for (let r = 0; r < grid.length; r++) {
			const c = grid[r].indexOf(index);
			if (c >= 0) return { r, c };
		}
		return null;
	};
	const a = at(from);
	const b = at(to);
	if (!a || !b) return [to];
	const out: number[] = [];
	for (let r = Math.min(a.r, b.r); r <= Math.max(a.r, b.r); r++)
		for (let c = Math.min(a.c, b.c); c <= Math.max(a.c, b.c); c++) if (grid[r][c] >= 0) out.push(grid[r][c]);
	return out;
}

/** A part of a geometry clicked: the geometry is selected, with the part named. */
export function clickPart(selection: Selection, name: string, kind: string, id: string | number): Selection {
	return { objects: [name], records: {}, part: { name, kind, id }, anchor: name };
}

/** The project's objects changed: anything selected that is gone leaves the selection. */
export function pruned(selection: Selection, names: string[]): Selection {
	const objects = selection.objects.filter((o) => names.includes(o));
	if (objects.length === selection.objects.length && (!selection.part || names.includes(selection.part.name))) return selection;
	const records: Record<string, number[]> = {};
	for (const name of objects) if (selection.records[name]) records[name] = selection.records[name];
	return {
		objects,
		records,
		part: selection.part && names.includes(selection.part.name) ? selection.part : null,
		anchor: selection.anchor && names.includes(selection.anchor) ? selection.anchor : null
	};
}

/** An object renamed: the selection follows it. */
export function renamed(selection: Selection, from: string, to: string): Selection {
	if (!selection.objects.includes(from) && selection.anchor !== from) return selection;
	const records: Record<string, number[]> = {};
	for (const [name, picks] of Object.entries(selection.records)) records[name === from ? to : name] = picks;
	return {
		objects: selection.objects.map((o) => (o === from ? to : o)),
		records,
		part: selection.part?.name === from ? { ...selection.part, name: to } : selection.part,
		anchor: selection.anchor === from ? to : selection.anchor
	};
}
