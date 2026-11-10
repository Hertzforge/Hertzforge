/**
 * The project tree's selection: which objects, and within a data object which
 * records. A plain click selects only what was clicked, Cmd (Ctrl
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
	/**
	 * A part of a geometry picked: a family, an element group, a solid, a mate, the nodes. Element
	 * groups picked together are `ids`, `id` being where a Shift range starts.
	 */
	part: { name: string; kind: string; id: string | number; ids?: (string | number)[] } | null;
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

/**
 * A part of a geometry clicked: the geometry is selected, with the part named. Element groups take
 * the modifiers as objects do, within one geometry: Cmd (Ctrl) adds or takes one away, Shift picks
 * the run from the last one picked plainly. `order` is the geometry's groups as the tree lists them.
 */
/** The parts of a geometry picked together with Cmd (Ctrl) or Shift: element groups, and solids. */
const TOGETHER = new Set(['group', 'solid']);

export function clickPart(
	selection: Selection,
	name: string,
	kind: string,
	id: string | number,
	mods: Modifiers = {},
	order: (string | number)[] = []
): Selection {
	const was = selection.part;
	if (TOGETHER.has(kind) && was?.kind === kind && was.name === name && (mods.toggle || mods.extend)) {
		const held = was.ids ?? [was.id];
		let ids: (string | number)[];
		if (mods.extend) {
			const a = order.indexOf(was.id);
			const b = order.indexOf(id);
			ids = a < 0 || b < 0 ? [...held, id] : order.slice(Math.min(a, b), Math.max(a, b) + 1);
		} else ids = held.includes(id) ? held.filter((g) => g !== id) : [...held, id];
		if (!ids.length) return { objects: [name], records: {}, part: null, anchor: name };
		const anchor = ids.includes(was.id) ? was.id : ids[0];
		return { objects: [name], records: {}, part: { name, kind, id: anchor, ...(ids.length > 1 ? { ids } : {}) }, anchor: name };
	}
	return { objects: [name], records: {}, part: { name, kind, id }, anchor: name };
}

/** Whether a part is picked: the one part, or one of the element groups or solids picked together. */
export function partIsPicked(selection: Selection, name: string, kind: string, id: string | number): boolean {
	const part = selection.part;
	return !!part && part.name === name && part.kind === kind && (part.ids ? part.ids.includes(id) : part.id === id);
}

/** The picked part as the service reads it: `{kind, id}`, and `ids` for groups or solids picked together. */
export function partForService(selection: Selection): { kind: string; id: string | number; ids?: (string | number)[] } | null {
	const part = selection.part;
	if (!part) return null;
	return part.ids ? { kind: part.kind, id: part.id, ids: part.ids } : { kind: part.kind, id: part.id };
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
