<script lang="ts">
	/**
	 * The project tree: one row per object in a fixed order by
	 * kind, each wearing its kind's icon, opening into what it holds. A geometry opens into its
	 * nodes, coordinate systems, element families with their element groups, solids and mated
	 * surfaces; a data object into a grid of its records, each cell wearing what that record
	 * measures; a shape set into its modes; a channel table into its channels. What is selected here
	 * is what the rest of the window works on: a click selects one row, Cmd (Ctrl elsewhere) adds or
	 * removes, Shift extends; a cell picks a record and stands for its object. No buttons live on
	 * the rows: the acts on a selection are offered beside the tree. A double-click renames.
	 */
	import type { AppState, SubName } from '$lib/app/state.svelte';
	import type { ProjectChildren, ProjectObject, SolidRow } from '$lib/protocol/types';
	import type { Modifiers } from './selection';
	import { tick } from 'svelte';
	import { spans, ticks } from './brackets';
	import { landsBefore, reordered, treeOrder } from './order';
	import Glyph from './Glyph.svelte';
	import { objectIcon, solidIcon } from './icons';
	import { partIsPicked } from './selection';
	import { mark, marked } from '$lib/viz/marks.svelte';
	import { markedBy, markedRun } from '$lib/viz/pick';
	import { canDragOut } from './files';

	interface Props {
		app: AppState;
	}
	let { app }: Props = $props();

	const p = $derived(app.project);
	/** The objects in the tree's order: each group together, the Basis first, then the rest by kind. */
	const objects = $derived(treeOrder(p.summary?.objects ?? [], p.summary?.links));
	const order = $derived(objects.map((o) => o.name));
	const isMac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);

	const ROLE_WORD = { reference: 'ref', response: 'resp', monitor: 'mon' } as const;

	function mods(e: MouseEvent) {
		return { toggle: isMac ? e.metaKey : e.ctrlKey, extend: e.shiftKey };
	}

	/** A list of sub-items as a grid one column wide, so Shift picks the run between two of them as it does a grid's block. */
	function column(indices: number[]): number[][] {
		return indices.map((i) => [i]);
	}

	/** Picking the object a combination in the action table needs: the tree answers that, and nothing else. */
	const picking = $derived(app.analyze.picking);
	function pick(o: ProjectObject): boolean {
		if (!picking) return false;
		if (picking.row.candidates?.includes(o.name)) app.analyze.join(o.name, picking.row);
		return true;
	}

	/** A part clicked: the one a combination of parts is waiting on, while one is; else a pick. */
	function clickSub(name: string, kind: string, id: string | number, m: Modifiers = {}, ids: (string | number)[] = []) {
		if (picking) {
			if (subCandidate(kind, id)) app.analyze.joinPart(name, kind, id, picking.row);
			return;
		}
		p.clickPart(name, kind, id, m, ids);
	}

	/** The rows opened onto what they list (a geometry's nodes, its coordinate systems, a group's elements), each by its key. */
	let listsOpen = $state<string[]>([]);
	const listKey = (name: string, kind: string, id: string | number) => `${name}\u0000${kind}\u0000${id}`;
	const toggleList = (key: string) => (listsOpen = listsOpen.includes(key) ? listsOpen.filter((k) => k !== key) : [...listsOpen, key]);
	/** What a geometry lists one a row, and marks one at a time. */
	type Listed = 'nodes' | 'systems' | 'elements';
	/** The link a list's marks are kept under, the service's `part_link`: the 3-D view and the node table mark with the same one. */
	const partLink = (kind: Listed, name: string) => `${kind}:${name}`;
	/** Where a Shift run starts in each list: the last row clicked there without Shift. */
	const anchors: Record<string, number> = {};

	/**
	 * A listed row clicked (a node, a coordinate system, an element): it is marked, in the 3-D
	 * view and for a node in the node table with it. Cmd (Ctrl) puts it in or takes it out, Shift
	 * marks the run from the last one clicked in that list. What holds it (the nodes, the systems,
	 * the element's group) is what is selected, unless it already is (its table may be open).
	 */
	function clickListed(name: string, kind: Listed, holder: { kind: string; id: string | number }, id: number, m: Modifiers, order: number[]) {
		if (picking) return;
		if (!partPicked(name, holder.kind, holder.id)) p.clickPart(name, holder.kind, holder.id);
		const link = partLink(kind, name);
		const from = anchors[link];
		if (m.extend && from !== undefined) mark(link, markedRun(order, from, id));
		else {
			anchors[link] = id;
			// the row it stands at among them, which a long table scrolls to
			mark(link, markedBy(marked(link), id, !!m.toggle), m.toggle ? undefined : order.indexOf(id));
		}
	}

	function subCandidate(kind: string, id: string | number): boolean {
		return !!picking?.part && picking.part === kind && !!picking.row.candidates?.includes(String(id));
	}

	function rowClick(e: MouseEvent, o: ProjectObject) {
		if (p.renaming === o.name || justDragged || pick(o)) return;
		p.clickObject(o.name, order, mods(e));
	}

	function rowKey(e: KeyboardEvent, o: ProjectObject) {
		if (e.key === 'Enter' || e.key === ' ') {
			e.preventDefault();
			if (pick(o)) return;
			p.clickObject(o.name, order, { toggle: isMac ? e.metaKey : e.ctrlKey, extend: e.shiftKey });
		} else if (e.key === 'ArrowRight' && o.opens && !p.expanded.has(o.name)) p.toggleExpanded(o.name);
		else if (e.key === 'ArrowLeft' && p.expanded.has(o.name)) p.toggleExpanded(o.name);
		else if (e.key === 'F2') startRename(o.name);
		else if ((e.key === 'Delete' || e.key === 'Backspace') && p.selection.objects.length) {
			// Delete deletes: the journal keeps what went
			e.preventDefault();
			void p.removeSelected();
		}
	}

	/** Cmd-C copies the selected objects, Cmd-V pastes them as copies; typing in a field is left alone. */
	function treeKey(e: KeyboardEvent) {
		const target = e.target as HTMLElement | null;
		if (target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable)) return;
		const cmd = isMac ? e.metaKey : e.ctrlKey;
		if (!cmd || e.altKey || e.shiftKey) return;
		if (e.key === 'c' || e.key === 'C') {
			if (p.copySelection()) e.preventDefault();
		} else if (e.key === 'v' || e.key === 'V') {
			e.preventDefault();
			void p.paste();
		}
	}

	/**
	 * A row dragged with the pointer: past a few pixels it carries the selection it is in (or itself),
	 * the row under the pointer lights as the drop target, and letting go there moves the dragged
	 * objects into that object's group; letting go on the tree's empty part takes them out of theirs.
	 * The pointer, not HTML5 drag and drop, since the desktop window takes native drags for files.
	 */
	let dragging = $state<string[] | null>(null);
	let dropOn = $state<string | null>(null);
	let dropOut = $state(false);
	/** The row the bar sits above, where the dragged objects will land: null at the bottom, undefined when letting go moves nothing. */
	const place = $derived.by(() => {
		if (!dragging) return undefined;
		const links = p.summary?.links;
		if (dropOn) return landsBefore(p.summary?.objects ?? [], links, dragging, dropOn);
		if (dropOut && (links ?? []).some((g) => g.members.some((m) => dragging?.includes(m)))) return landsBefore(p.summary?.objects ?? [], links, dragging, null);
		return undefined;
	});
	/** A solid being dragged among its siblings (the drag in the tree): a subtraction cuts the rest
	 * from the first. */
	let solidDrag: { name: string; id: number; within: number | null; ids: number[]; y: number; moved: boolean } | null = null;
	let solidPlace = $state<{ name: string; within: number | null; before: number | null } | null>(null);
	let solidDragged = false;
	function solidDown(e: PointerEvent, name: string, id: number, within: number | null, ids: number[]) {
		if (e.button !== 0 || ids.length < 2) return;
		e.stopPropagation();
		solidDrag = { name, id, within, ids, y: e.clientY, moved: false };
		(e.currentTarget as HTMLElement).setPointerCapture?.(e.pointerId);
	}
	function solidMove(e: PointerEvent) {
		const drag = solidDrag;
		if (!drag) return;
		e.stopPropagation();
		if (!drag.moved && Math.abs(e.clientY - drag.y) < 6) return;
		drag.moved = true;
		const row = document.elementFromPoint(e.clientX, e.clientY)?.closest<HTMLElement>('[data-solid]');
		if (!row || row.dataset.solidOf !== `${drag.name}|${drag.within ?? ''}`) {
			solidPlace = null;
			return;
		}
		const over = Number(row.dataset.solid);
		const box = row.getBoundingClientRect();
		const after = e.clientY > box.top + box.height / 2;
		const next = drag.ids[drag.ids.indexOf(over) + 1] ?? null;
		solidPlace = { name: drag.name, within: drag.within, before: after ? next : over };
	}
	function solidUp(e: PointerEvent) {
		const drag = solidDrag;
		const place = solidPlace;
		solidDrag = null;
		solidPlace = null;
		if (!drag) return;
		e.stopPropagation();
		if (!drag.moved) return;
		solidDragged = true;
		setTimeout(() => (solidDragged = false), 0);
		if (!place) return;
		const order = reordered(drag.ids, drag.id, place.before);
		if (order.some((id, k) => id !== drag.ids[k])) void p.reorderSolids(drag.name, order, drag.within);
	}
	let dragFrom: { name: string; x: number; y: number; id: number } | null = null;
	let justDragged = false;
	function dragDown(e: PointerEvent, o: ProjectObject) {
		if (e.button !== 0 || p.renaming === o.name) return;
		dragFrom = { name: o.name, x: e.clientX, y: e.clientY, id: e.pointerId };
	}
	/** Objects dragged out of the window, while the button stays down: their files are being written. */
	let outHeld = false;
	/** Whether the pointer has reached the window's edge, where a drag in the tree becomes a drag out of it. */
	function atEdge(e: PointerEvent): boolean {
		return e.clientX <= 1 || e.clientY <= 1 || e.clientX >= innerWidth - 2 || e.clientY >= innerHeight - 2;
	}
	function dragMove(e: PointerEvent) {
		if (!dragFrom) return;
		if (!dragging) {
			if (Math.hypot(e.clientX - dragFrom.x, e.clientY - dragFrom.y) < 6) return;
			dragging = p.isSelected(dragFrom.name) ? [...p.selection.objects] : [dragFrom.name];
			(e.currentTarget as HTMLElement).setPointerCapture?.(dragFrom.id);
		}
		if (atEdge(e) && canDragOut()) {
			// out of the window: each object saved where it is let go (Brandon, 2026-11-11)
			const names = dragging;
			(e.currentTarget as HTMLElement).releasePointerCapture?.(dragFrom.id);
			dragging = null;
			dropOn = null;
			dropOut = false;
			dragFrom = null;
			outHeld = true;
			void p.dragOut({ names }, () => outHeld).finally(() => (outHeld = false));
			return;
		}
		const row = document.elementFromPoint(e.clientX, e.clientY)?.closest<HTMLElement>('[data-testid^="object-"]');
		const name = row?.dataset.testid?.slice('object-'.length) ?? null;
		dropOn = name && !dragging.includes(name) ? name : null;
		dropOut = !name && overBlank(e);
	}
	/** Whether the pointer is on the panel's blank space, from the tree's top down, where letting go takes the dragged objects out of their groups. */
	function overBlank(e: PointerEvent): boolean {
		if (!treeBox || e.clientY < treeBox.getBoundingClientRect().top) return false;
		return (treeBox.closest('aside') ?? treeBox).contains(document.elementFromPoint(e.clientX, e.clientY));
	}
	function dragUp(e: PointerEvent) {
		outHeld = false;
		const moving = dragging;
		const target = dropOn;
		const inside = overBlank(e);
		dragging = null;
		dropOn = null;
		dropOut = false;
		dragFrom = null;
		if (!moving) return;
		justDragged = true;
		setTimeout(() => (justDragged = false), 0);
		if (target) void p.moveInto(moving, target);
		else if (inside) void p.moveInto(moving, null);
	}

	/** A DOF label in a grid being typed over: the object and the row, and the text. */
	let dofEdit = $state<{ name: string; row: number } | null>(null);
	let dofText = $state('');
	function startDof(name: string, row: number, dof: string) {
		dofEdit = { name, row };
		dofText = dof;
	}
	async function commitDof(kids: Extract<ProjectChildren, { kind: 'grid' }>) {
		const edit = dofEdit;
		dofEdit = null;
		if (!edit) return;
		const was = kids.rows[edit.row];
		const next = dofText.trim();
		if (!was || !next || next === was.dof) return;
		await p.renameDof(edit.name, was.dof, next, was.quantity === 'unknown' ? '' : was.quantity);
	}

	let newName = $state('');
	function startRename(name: string) {
		p.renaming = name;
		newName = name;
	}
	async function commitRename() {
		const name = p.renaming;
		const to = newName.trim();
		p.renaming = null;
		if (name && to && to !== name) await p.rename(name, to);
	}
	/** A photo's name typed over in its row (double-click, or F2 on the picked photo). */
	function startPartRename(name: string, index: number, current: string) {
		p.renamingPart = { name, index };
		newName = current;
	}
	async function commitPartRename(current: string) {
		const part = p.renamingPart;
		const to = newName.trim();
		p.renamingPart = null;
		if (part && to && to !== current) await p.renamePart(part.name, part.index, to);
	}
	// anything with a name of its own is named on its row: a double-click, or F2 on it (Brandon, 2026-11-17)
	let subDone: ((to: string) => Promise<unknown>) | null = null;
	function startSub(name: string, kind: SubName, id: number, current: string, done?: (to: string) => Promise<unknown>) {
		p.renamingSub = { name, kind, id };
		newName = current;
		subDone = done ?? ((to) => p.renameSub(name, kind as Exclude<SubName, 'channel'>, id, to));
	}
	async function commitSub(current: string) {
		const done = subDone;
		const to = newName.trim();
		const was = p.renamingSub;
		p.renamingSub = null;
		subDone = null;
		if (was && done && to && to !== current) await done(to);
	}
	const naming = (name: string, kind: SubName, id: number) => p.renamingSub?.name === name && p.renamingSub.kind === kind && p.renamingSub.id === id;
	/** F2 on a sub-item's row starts its name, as a double-click does. */
	function subKey(e: KeyboardEvent, start: () => void, enter: () => void) {
		if (e.key === 'Enter') enter();
		else if (e.key === 'F2') {
			e.stopPropagation();
			start();
		}
	}
	function focusAndSelect(node: HTMLInputElement) {
		node.focus();
		node.select();
	}

	/** The link brackets: each group's members as rows, and where those rows sit, measured after each render. */
	const painted = $derived(spans(p.summary?.links, order));
	/** The striped brackets on Matched Modes rows, in the colors of both sets' groups. */
	const striped = $derived(ticks(p.summary?.links, p.summary?.objects ?? []));
	let treeBox = $state<HTMLDivElement | null>(null);
	const rowBoxes = new Map<string, HTMLDivElement>();
	let centers = $state<Record<string, number>>({});
	let treeHeight = $state(0);
	function rowBox(node: HTMLDivElement, name: string) {
		rowBoxes.set(name, node);
		return {
			update(next: string) {
				rowBoxes.delete(name);
				name = next;
				rowBoxes.set(name, node);
			},
			destroy() {
				rowBoxes.delete(name);
			}
		};
	}
	function measure() {
		if (!treeBox) return;
		const top = treeBox.getBoundingClientRect().top;
		const next: Record<string, number> = {};
		for (const [name, node] of rowBoxes) {
			const r = node.getBoundingClientRect();
			next[name] = r.top - top + r.height / 2;
		}
		centers = next;
		treeHeight = treeBox.scrollHeight;
	}
	$effect(() => {
		// rows move when objects, open rows or their children change
		void [p.summary, p.expanded, p.children, painted, striped];
		void tick().then(measure);
	});
	$effect(() => {
		if (!treeBox) return;
		const watch = new ResizeObserver(() => measure());
		watch.observe(treeBox);
		return () => watch.disconnect();
	});
	const issueOf = (name: string) => p.summary?.objects.find((o) => o.name === name)?.issue;
	const bad = (name: string, index: number) => (issueOf(name)?.sub_items ?? []).includes(index);

	const picked = (name: string, index: number) => (p.selection.records[name] ?? []).includes(index);
	const partPicked = (name: string, kind: string, id: string | number) => partIsPicked(p.selection, name, kind, id);
	const held = (name: string): ProjectChildren | undefined => p.children[name];
	const roleWord = (role?: string) => (role ? ROLE_WORD[role as keyof typeof ROLE_WORD] : '');
</script>

<!-- what a row of a geometry lists, one a row: each is marked by a click, with its list's link -->
{#snippet entries(name: string, kind: Listed, holder: { kind: string; id: string | number }, ids: number[], label: (id: number, i: number) => string, glyph: string, depth: number, more: string)}
	{@const on = new Set(marked(partLink(kind, name)))}
	<div class="entries" role="group" data-testid="{kind}-{name}{holder.kind === 'group' ? `-${holder.id}` : ''}">
		{#each ids as id, i (id)}
			<div class="sub" class:picked={on.has(id)} style="--depth: {depth}" role="treeitem" aria-selected={on.has(id)} tabindex="-1" data-testid="{kind}-{name}-{id}" onclick={(e) => clickListed(name, kind, holder, id, mods(e), ids)} onkeydown={(e) => e.key === 'Enter' && clickListed(name, kind, holder, id, {}, ids)}>
				<Glyph name={glyph} of="child" parent="geometry" size={14} />
				<span class="label">{label(id, i)}</span>
			</div>
		{/each}
		{#if more}<p class="note" style="padding-left: {8 + 14 * depth}px">{more}</p>{/if}
	</div>
{/snippet}

{#snippet subField(what: string, current: string)}
	<input
		class="field rename"
		aria-label="New name for {what}"
		data-testid="rename-sub-field"
		bind:value={newName}
		use:focusAndSelect
		onclick={(e) => e.stopPropagation()}
		ondblclick={(e) => e.stopPropagation()}
		onpointerdown={(e) => e.stopPropagation()}
		onkeydown={(e) => {
			e.stopPropagation();
			if (e.key === 'Enter') void commitSub(current);
			if (e.key === 'Escape') p.renamingSub = null;
		}}
		onblur={() => (p.renamingSub = null)}
	/>
{/snippet}

{#snippet solidRows(name: string, solids: SolidRow[], depth: number, within: number | null = null)}
	{#each solids as solid (solid.id)}
		<div
			class="sub"
			class:picked={partPicked(name, 'solid', solid.id)}
			class:candidate={subCandidate('solid', solid.id)}
			class:place-solid={solidPlace?.within === within && solidPlace.name === name && solidPlace.before === solid.id}
			class:place-solid-end={solidPlace?.within === within && solidPlace.name === name && solidPlace.before === null && solid === solids[solids.length - 1]}
			style="--depth: {depth}"
			role="treeitem"
			aria-selected={partPicked(name, 'solid', solid.id)}
			tabindex="-1"
			data-testid="solid-{name}-{solid.id}"
			data-solid-of="{name}|{within ?? ''}"
			data-solid={solid.id}
			onpointerdown={(e) => solidDown(e, name, solid.id, within, solids.map((x) => x.id))}
			onpointermove={solidMove}
			onpointerup={solidUp}
			onpointercancel={() => { solidDrag = null; solidPlace = null; }}
			onclick={(e) => { if (solidDragged) { e.stopPropagation(); return; } clickSub(name, 'solid', solid.id, mods(e), solids.map((x) => x.id)); }}
			ondblclick={() => startSub(name, 'solid', solid.id, solid.label)}
			onkeydown={(e) => subKey(e, () => startSub(name, 'solid', solid.id, solid.label), () => clickSub(name, 'solid', solid.id))}
		>
			<Glyph name={solidIcon(solid.kind)} of="child" parent="geometry" size={14} />
			{#if naming(name, 'solid', solid.id)}{@render subField(solid.label, solid.label)}{:else}<span class="label">{solid.label}</span>{/if}
		</div>
		{#if solid.children.length}{@render solidRows(name, solid.children, depth + 1, solid.id)}{/if}
	{/each}
{/snippet}

<!-- svelte-ignore a11y_no_noninteractive_element_interactions (the keys and the pointer act on the rows inside) -->
<div class="tree" class:bracketed={painted.length > 0 || striped.length > 0} class:dragging={!!dragging} class:place-end={place === null} class:picking={!!picking} role="tree" tabindex="-1" aria-label="Project objects" data-testid="project-tree" bind:this={treeBox} onkeydown={treeKey} onpointermove={dragMove} onpointerup={dragUp} onpointercancel={() => { dragging = null; dropOn = null; dragFrom = null; }}>
	{#if painted.length || striped.length}
		<svg class="gutter" width="14" height={treeHeight} aria-hidden="true" data-testid="link-brackets">
			{#each painted as span (span.rows.join('\u0000'))}
				{@const ys = span.rows.map((r) => centers[r]).filter((y) => y !== undefined)}
				{#if ys.length >= 2}
					{@const x = 3 + 3 * span.column}
					{@const top = Math.min(...ys)}
					{@const bottom = Math.max(...ys)}
					<g stroke={span.color} stroke-width={span.bold ? 3 : 1.5} stroke-linecap="round" data-group={span.name || null}>
						<line x1={x} y1={top} x2={x} y2={bottom} />
						{#each ys as y, k (k)}<line x1={x} y1={y} x2={x + 4} y2={y} />{/each}
					</g>
					{#if span.name && bottom - top > 40}
						<text x={x - 1} y={bottom - 4} transform="rotate(-90 {x - 1} {bottom - 4})" fill={span.color} font-size="8" textLength={Math.min(bottom - top - 8, span.name.length * 5)} lengthAdjust="spacingAndGlyphs">{span.name}</text>
					{/if}
				{/if}
			{/each}
			{#each striped as t (t.row)}
				{@const y = centers[t.row]}
				{#if y !== undefined}
					<!-- a short bracket on its one row, each color a dash in turn, so it reads as both groups' -->
					<g stroke-width="2" stroke-linecap="butt" data-matched={t.row}>
						{#each t.colors as color, k (k)}
							<line x1="3" y1={y - 7} x2="3" y2={y + 7} stroke={color} stroke-dasharray={t.colors.length > 1 ? '3.5 3.5' : undefined} stroke-dashoffset={k * 3.5} />
							<line x1="3" y1={y} x2="7" y2={y} stroke={color} stroke-dasharray={t.colors.length > 1 ? '2 2' : undefined} stroke-dashoffset={k * 2} />
						{/each}
					</g>
				{/if}
			{/each}
		</svg>
	{/if}
	{#each objects as o (o.name)}
		{@const open = p.expanded.has(o.name)}
		{@const on = p.isSelected(o.name)}
		{@const active = o.name === p.summary?.active_geometry}
		{@const icon = objectIcon(o.kind, o.banded)}
		<div class="row" class:on class:issue={!!o.issue} class:drop={dropOn === o.name} class:place={place === o.name} class:candidate={picking?.row.candidates?.includes(o.name)} role="treeitem" aria-selected={on} aria-expanded={o.opens ? open : undefined} tabindex="0" data-testid="object-{o.name}" title={o.issue?.message} use:rowBox={o.name} onpointerdown={(e) => dragDown(e, o)} onclick={(e) => rowClick(e, o)} onkeydown={(e) => rowKey(e, o)} ondblclick={() => startRename(o.name)}>
			{#if o.opens}
				<button class="twist" class:open aria-label={open ? `Close ${o.name}` : `Open ${o.name}`} tabindex="-1" data-testid="expand-{o.name}" onclick={(e) => { e.stopPropagation(); p.toggleExpanded(o.name); }}>
					<svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true"><path d="M3 1.5l4 3.5-4 3.5" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" /></svg>
				</button>
			{:else}
				<span class="twist blank"></span>
			{/if}
			<Glyph name={icon} badge={o.units_defined === false} title={o.units_defined === false ? 'Units not declared' : undefined} />
			{#if p.renaming === o.name}
				<input
					class="field mono rename"
					aria-label="New name for {o.name}"
					data-testid="rename-field"
					bind:value={newName}
					use:focusAndSelect
					onclick={(e) => e.stopPropagation()}
					onkeydown={(e) => {
						e.stopPropagation();
						if (e.key === 'Enter') void commitRename();
						if (e.key === 'Escape') p.renaming = null;
					}}
					onblur={() => (p.renaming = null)}
				/>
			{:else}
				<span class="name">{o.name}</span>
				{#if active}<span class="mark" title="Data is drawn on this geometry when nothing else names one">active</span>{/if}
				{#if o.stale}<span class="mark stale" data-testid="stale-{o.name}" title={o.stale}>stale</span>{/if}
				<span class="holds" title={o.holds}>{o.holds ?? o.kind.replace(/_/g, ' ')}</span>
			{/if}
		</div>
		{#if open}
			{@const kids = held(o.name)}
			<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
			<div class="kids" role="group" data-testid="children-{o.name}" onkeydown={(e) => { if ((e.key === 'Delete' || e.key === 'Backspace') && p.selection.objects.length && !(e.target as HTMLElement).closest('input')) { e.preventDefault(); if (p.partDeletion) void p.removeSelected(); else if (!p.selection.part) p.confirming = true; } }}>
				{#if !kids}
					<p class="note">Opening.</p>
				{:else if kids.kind === 'geometry'}
					{#each kids.parts as part (part.id)}
						{@const lists = !!part.ids?.length || !!part.rows?.length}
						{@const listed = lists && listsOpen.includes(listKey(o.name, 'part', part.id))}
						<div class="sub" class:picked={partPicked(o.name, 'part', part.id)} role="treeitem" aria-selected={partPicked(o.name, 'part', part.id)} aria-expanded={lists ? listed : undefined} tabindex="-1" data-testid="part-{o.name}-{part.id}" onclick={() => clickSub(o.name, 'part', part.id)} onkeydown={(e) => e.key === 'Enter' && clickSub(o.name, 'part', part.id)}>
							{#if lists}
								<button class="twist inner" class:open={listed} aria-label={listed ? `Close the ${part.label.toLowerCase()} of ${o.name}` : `List the ${part.label.toLowerCase()} of ${o.name}`} tabindex="-1" data-testid="expand-{part.id}-{o.name}" onclick={(e) => { e.stopPropagation(); toggleList(listKey(o.name, 'part', part.id)); }}>
									<svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true"><path d="M3 1.5l4 3.5-4 3.5" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" /></svg>
								</button>
							{/if}
							<Glyph name={part.id} of="child" parent="geometry" empty={!part.count} size={14} />
							<span class="label">{part.label} ({part.count})</span>
						</div>
						{#if listed && part.id === 'nodes' && part.ids}
							{@render entries(o.name, 'nodes', { kind: 'part', id: part.id }, part.ids, (id) => `Node ${id}`, 'nodes', 1, part.count > part.ids.length ? `${part.count - part.ids.length} more not listed: they are in the nodes' table (Edit).` : '')}
						{:else if listed && part.rows}
							{@const rows = part.rows}
							{@render entries(o.name, 'systems', { kind: 'part', id: part.id }, part.ids ?? [], (_id, i) => rows[i], part.id, 1, '')}
						{/if}
					{/each}
					{#each kids.families as family (family.id)}
						<div class="sub" class:picked={partPicked(o.name, 'family', family.id)} role="treeitem" aria-selected={partPicked(o.name, 'family', family.id)} tabindex="-1" data-testid="family-{o.name}-{family.id}" onclick={() => clickSub(o.name, 'family', family.id)} onkeydown={(e) => e.key === 'Enter' && clickSub(o.name, 'family', family.id)}>
							<Glyph name={family.id} of="child" parent="geometry" empty={!family.count} size={14} />
							<span class="label" class:dim={!family.count}>{family.label} ({family.count})</span>
						</div>
						{#each family.groups as group (group.id)}
							<div class="sub" class:picked={partPicked(o.name, 'group', group.id)} class:candidate={subCandidate('group', group.id)} style="--depth: 1" role="treeitem" aria-selected={partPicked(o.name, 'group', group.id)} tabindex="-1" data-testid="group-{o.name}-{group.id}" onclick={(e) => clickSub(o.name, 'group', group.id, mods(e), kids.families.flatMap((f) => f.groups.map((g) => g.id)))} ondblclick={() => startSub(o.name, 'group', group.id, group.label)} onkeydown={(e) => subKey(e, () => startSub(o.name, 'group', group.id, group.label), () => clickSub(o.name, 'group', group.id))}>
								{#if group.elements?.length}
									{@const shown = listsOpen.includes(listKey(o.name, 'group', group.id))}
									<button class="twist inner" class:open={shown} aria-label={shown ? `Close the elements of ${group.label}` : `List the elements of ${group.label}`} tabindex="-1" data-testid="expand-group-{o.name}-{group.id}" onclick={(e) => { e.stopPropagation(); toggleList(listKey(o.name, 'group', group.id)); }}>
										<svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true"><path d="M3 1.5l4 3.5-4 3.5" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" /></svg>
									</button>
								{/if}
								<Glyph name="groups" of="child" parent="geometry" size={14} />
								{#if naming(o.name, 'group', group.id)}{@render subField(group.label, group.label)}{:else}<span class="label">{group.label} ({group.count})</span>{/if}
							</div>
							{#if group.elements?.length && listsOpen.includes(listKey(o.name, 'group', group.id))}
								{@render entries(o.name, 'elements', { kind: 'group', id: group.id }, group.elements, (id) => `Element ${id}`, family.id, 2, group.count > group.elements.length ? `${group.count - group.elements.length} more not listed: they are in the group's table (Edit).` : '')}
							{/if}
						{/each}
					{/each}
					<div class="sub" class:picked={partPicked(o.name, 'solids', 0)} role="treeitem" aria-selected={partPicked(o.name, 'solids', 0)} tabindex="-1" data-testid="solids-{o.name}" title="Shapes to build parts from, before they are meshed" onclick={() => clickSub(o.name, 'solids', 0)} onkeydown={(e) => e.key === 'Enter' && clickSub(o.name, 'solids', 0)}>
						<Glyph name="solid_block" of="child" parent="geometry" size={14} />
						<span class="label" class:dim={!kids.solids.length}>Solids ({kids.solids.length})</span>
					</div>
					{@render solidRows(o.name, kids.solids, 1)}
					{#if kids.mates.length}
						<div class="sub" class:picked={partPicked(o.name, 'mates', 0)} role="treeitem" aria-selected={partPicked(o.name, 'mates', 0)} tabindex="-1" data-testid="mates-{o.name}" onclick={() => clickSub(o.name, 'mates', 0)} onkeydown={(e) => e.key === 'Enter' && clickSub(o.name, 'mates', 0)}>
							<Glyph name="mates" of="child" parent="geometry" size={14} />
							<span class="label">Mated Surfaces ({kids.mates.length})</span>
						</div>
						{#each kids.mates as mate (mate.id)}
							<div class="sub" class:picked={partPicked(o.name, 'mate', mate.id)} style="--depth: 1" role="treeitem" aria-selected={partPicked(o.name, 'mate', mate.id)} tabindex="-1" data-testid="mate-{o.name}-{mate.id}" onclick={() => clickSub(o.name, 'mate', mate.id)} ondblclick={() => startSub(o.name, 'mate', mate.id, mate.name)} onkeydown={(e) => subKey(e, () => startSub(o.name, 'mate', mate.id, mate.name), () => clickSub(o.name, 'mate', mate.id))}>
								<Glyph name="mates" of="child" parent="geometry" size={14} />
								{#if naming(o.name, 'mate', mate.id)}{@render subField(mate.name, mate.name)}{:else}<span class="label">{mate.label}</span>{/if}
							</div>
						{/each}
					{/if}
				{:else if kids.kind === 'grid'}
					{@const headed = kids.columns.some((c) => c)}
					{@const roles = kids.rows.some((r) => r.role)}
					<div class="gridbox">
						<table class="grid" data-testid="grid-{o.name}">
							{#if headed}
								<thead>
									<tr>
										<th></th>
										{#if roles}<th class="role"></th>{/if}
										{#each kids.columns as column, c (c)}<th class="col" title={column}>{column}</th>{/each}
									</tr>
								</thead>
							{/if}
							<tbody>
								{#each kids.rows as row, r (r)}
									<tr>
										{#if dofEdit && dofEdit.name === o.name && dofEdit.row === r}
											<th class="dof editing">
												<input class="field mono dofedit" aria-label="New DOF for {row.dof}" data-testid="dof-field" bind:value={dofText} use:focusAndSelect
													onclick={(e) => e.stopPropagation()}
													onkeydown={(e) => { e.stopPropagation(); if (e.key === 'Enter') void commitDof(kids); if (e.key === 'Escape') dofEdit = null; }}
													onblur={() => void commitDof(kids)} />
											</th>
										{:else}
											<th class="dof" title={(row.quantity ? `${row.dof}: ${row.quantity.replace(/_/g, ' ')}` : row.dof) + '. Double-click to rename the DOF.'} ondblclick={(e) => { e.stopPropagation(); startDof(o.name, r, row.dof); }}>{row.label}</th>
										{/if}
										{#if roles}
											<td class="role" role="button" tabindex="-1" title="{row.role}: click to change" data-testid="role-{o.name}-{r}" onclick={(e) => { e.stopPropagation(); const index = kids.cells[r].find((i) => i >= 0); if (index !== undefined && row.role) void p.cycleRole(o.name, index, row.role); }} onkeydown={(e) => e.key === 'Enter' && e.stopPropagation()}>{roleWord(row.role)}</td>
										{/if}
										{#each kids.cells[r] as index, c (c)}
											{#if index < 0}
												<td class="hole" aria-hidden="true"></td>
											{:else}
												<td
													class="cell"
													class:picked={picked(o.name, index)}
													class:bad={bad(o.name, index)}
													role="button"
													tabindex="-1"
													aria-pressed={picked(o.name, index)}
													aria-label="{row.label}{headed ? ` against ${kids.columns[c]}` : ''}"
													data-testid="cell-{o.name}-{index}"
													onclick={(e) => { e.stopPropagation(); p.clickRecord(o.name, index, mods(e), kids.cells); }}
												>
													<Glyph name={kids.icons[index]} of="record" parent={icon} badge={kids.undefined.includes(index)} size={14} />
												</td>
											{/if}
										{/each}
									</tr>
								{/each}
							</tbody>
						</table>
					</div>
				{:else if kids.kind === 'modes'}
					{#each kids.modes as mode (mode.index)}
						<div class="sub" class:picked={picked(o.name, mode.index - 1)} role="treeitem" aria-selected={picked(o.name, mode.index - 1)} tabindex="-1" data-testid="mode-{o.name}-{mode.index}" onclick={(e) => p.clickRecord(o.name, mode.index - 1, mods(e), column(kids.modes.map((m) => m.index - 1)))} ondblclick={() => startSub(o.name, 'mode', mode.index, mode.description)} onkeydown={(e) => subKey(e, () => startSub(o.name, 'mode', mode.index, mode.description), () => p.clickRecord(o.name, mode.index - 1))}>
							<Glyph name="mode" of="child" parent="shape_set" size={14} />
							<span class="label mono">M{mode.index}</span>
							<span class="num">{mode.frequency.toPrecision(5)} Hz</span>
							<span class="num dim">{(mode.damping * 100).toFixed(2)} %</span>
							{#if naming(o.name, 'mode', mode.index)}{@render subField(`mode ${mode.index}`, mode.description)}{:else if mode.description}<span class="dim desc" title={mode.description}>{mode.description}</span>{/if}
						</div>
					{/each}
				{:else if kids.kind === 'channels'}
					{#each kids.channels as channel, i (i)}
						<div class="sub" class:picked={picked(o.name, i)} role="treeitem" aria-selected={picked(o.name, i)} tabindex="-1" data-testid="channel-{o.name}-{i}" onclick={(e) => p.clickRecord(o.name, i, mods(e), column(kids.channels.map((_, k) => k)))} ondblclick={() => startSub(o.name, 'channel', i, channel.dof, (to) => p.renameDof(o.name, channel.dof, to, channel.quantity))} onkeydown={(e) => subKey(e, () => startSub(o.name, 'channel', i, channel.dof, (to) => p.renameDof(o.name, channel.dof, to, channel.quantity)), () => p.clickRecord(o.name, i))}>
							<Glyph name={channel.quantity} of="record" parent="channel_table" size={14} />
							{#if naming(o.name, 'channel', i)}{@render subField(`channel ${i + 1}`, channel.dof)}{:else}<span class="label mono">{channel.dof || `channel ${i + 1}`}</span>{/if}
						</div>
					{/each}
				{:else if kids.kind === 'photos'}
					{#each kids.photos as photo, i (i)}
						<div class="sub" class:picked={picked(o.name, i)} role="treeitem" aria-selected={picked(o.name, i)} tabindex="-1" data-testid="photo-{o.name}-{i}" onclick={(e) => p.clickRecord(o.name, i, mods(e), column(kids.photos.map((_, k) => k)))} ondblclick={() => startPartRename(o.name, i, photo)} onkeydown={(e) => { if (e.key === 'Enter') p.clickRecord(o.name, i); else if (e.key === 'F2') { e.stopPropagation(); startPartRename(o.name, i, photo); } }}>
							<Glyph name="photos" size={14} />
							{#if p.renamingPart?.name === o.name && p.renamingPart.index === i}
								<input
									class="field rename"
									aria-label="New name for the photo {photo}"
									data-testid="rename-photo-field"
									bind:value={newName}
									use:focusAndSelect
									onclick={(e) => e.stopPropagation()}
									ondblclick={(e) => e.stopPropagation()}
									onkeydown={(e) => {
										e.stopPropagation();
										if (e.key === 'Enter') void commitPartRename(photo);
										if (e.key === 'Escape') p.renamingPart = null;
									}}
									onblur={() => (p.renamingPart = null)}
								/>
							{:else}
								<span class="label">{photo}</span>
							{/if}
						</div>
					{/each}
				{:else if kids.kind === 'matches'}
					{#each kids.matches as match, i (i)}
						<div class="sub" class:picked={picked(o.name, i)} role="treeitem" aria-selected={picked(o.name, i)} tabindex="-1" data-testid="match-{o.name}-{i}" onclick={(e) => p.clickRecord(o.name, i, mods(e), column(kids.matches.map((_, k) => k)))} onkeydown={(e) => e.key === 'Enter' && p.clickRecord(o.name, i)}>
							<Glyph name="mode" of="child" parent="matched_modes" size={14} />
							<span class="label mono">M{match.first} to M{match.second}</span>
							<span class="num dim">MAC {match.mac.toFixed(3)}</span>
						</div>
					{/each}
				{:else if kids.kind === 'items'}
					{#each kids.items as item, i (i)}
						<div class="sub" class:picked={picked(o.name, i)} role="treeitem" aria-selected={picked(o.name, i)} tabindex="-1" data-testid="item-{o.name}-{i}" onclick={(e) => p.clickRecord(o.name, i, mods(e), column(kids.items.map((_, k) => k)))} onkeydown={(e) => e.key === 'Enter' && p.clickRecord(o.name, i)}>
							<Glyph name={icon} size={14} />
							<span class="label">{item.label}</span>
							{#if item.detail}<span class="dim desc" title={item.detail}>{item.detail}</span>{/if}
						</div>
					{/each}
				{:else}
					<p class="note">Nothing to open.</p>
				{/if}
			</div>
		{/if}
	{/each}
</div>

<style>
	.tree.picking .row:not(.candidate) {
		opacity: 0.35;
		cursor: not-allowed;
	}
	.tree.picking .sub:not(.candidate) {
		opacity: 0.35;
		cursor: not-allowed;
	}
	.tree.picking .sub.candidate {
		box-shadow: inset 2px 0 0 var(--accent);
	}
	.tree.picking .row.candidate {
		box-shadow: inset 2px 0 0 var(--accent);
	}
	.tree {
		position: relative;
		margin-top: var(--sp-2);
		font-size: var(--text-xs);
		user-select: none;
	}

	/* room for the brackets down the left edge */
	.tree.bracketed {
		padding-left: 12px;
	}

	.gutter {
		position: absolute;
		top: 0;
		left: 0;
		pointer-events: none;
		overflow: visible;
	}

	.row.issue .name {
		color: var(--status-bad, #e5534b);
	}

	.grid td.cell.bad {
		box-shadow: inset 0 0 0 1px var(--status-bad, #e5534b);
	}

	.row {
		position: relative;
		display: flex;
		align-items: center;
		gap: 5px;
		padding: 3px 6px 3px 2px;
		border-radius: var(--radius-sm);
		cursor: default;
	}

	.row:hover {
		background: var(--bg-3);
	}

	.row.on {
		background: var(--blue-weak);
		box-shadow: inset 0 0 0 1px var(--blue-line);
	}

	.row.drop {
		box-shadow: inset 0 0 0 1px var(--accent);
	}

	/* where the dragged objects will land */
	.sub[data-solid] {
		position: relative;
		touch-action: none;
	}
	.sub.place-solid::before,
	.sub.place-solid-end::after {
		content: '';
		position: absolute;
		right: 0;
		left: 0;
		height: 2px;
		border-radius: 1px;
		background: var(--accent);
		pointer-events: none;
	}
	.sub.place-solid::before {
		top: -1px;
	}
	.sub.place-solid-end::after {
		bottom: -1px;
	}

	.row.place::before,
	.tree.place-end::after {
		content: '';
		position: absolute;
		right: 0;
		height: 2px;
		border-radius: 1px;
		background: var(--accent);
		pointer-events: none;
	}
	.row.place::before {
		top: -1px;
		left: 0;
	}
	.tree.place-end::after {
		bottom: -3px;
		left: 12px;
	}

	.tree.dragging {
		cursor: grabbing;
		user-select: none;
	}

	.row:focus-visible,
	.sub:focus-visible {
		outline: 1px solid var(--accent);
		outline-offset: -1px;
	}

	.twist {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 14px;
		height: 14px;
		padding: 0;
		color: var(--text-4);
		background: none;
		border: none;
		border-radius: 2px;
		cursor: pointer;
		transition: transform var(--dur-fast) var(--ease);
	}

	.twist.open {
		transform: rotate(90deg);
	}

	.twist:hover {
		color: var(--text-1);
	}

	.twist.blank {
		cursor: default;
	}

	.name {
		color: var(--text-1);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.holds {
		margin-left: auto;
		padding-left: 6px;
		color: var(--text-4);
		font-size: 10px;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		max-width: 45%;
	}

	.mark {
		font-size: 10px;
		color: var(--text-3);
	}

	.mark.stale {
		color: var(--status-warn);
	}

	.rename {
		flex: 1;
		min-width: 0;
		font-size: var(--text-xs);
	}

	.kids {
		margin-left: 14px;
		padding-left: 8px;
		border-left: 1px solid var(--line-1);
	}

	.sub {
		display: flex;
		align-items: center;
		gap: 5px;
		padding: 2px 6px 2px calc(2px + 14px * var(--depth, 0));
		border-radius: var(--radius-sm);
		color: var(--text-2);
		cursor: default;
	}

	.sub:hover {
		background: var(--bg-3);
	}

	.sub.picked {
		background: var(--blue-weak);
		box-shadow: inset 0 0 0 1px var(--blue-line);
	}

	.twist.inner {
		margin-left: -2px;
	}

	/* what a row lists, one a row (nodes, coordinate systems, a group's elements): past its height the list scrolls rather than the tree */
	.entries {
		max-height: 260px;
		overflow-y: auto;
	}

	.label {
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.dim {
		color: var(--text-4);
	}

	.desc {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.num {
		font-variant-numeric: tabular-nums;
		white-space: nowrap;
	}

	.note {
		padding: 2px 6px;
		color: var(--text-4);
	}

	/* the record grid: rows by DOF, columns by reference or capture, a 14 px icon in every cell;
	   past its limits it scrolls rather than the tree */
	.gridbox {
		max-height: 320px;
		max-width: 460px;
		overflow: auto;
		margin: 2px 0 4px;
		border: 1px solid var(--line-1);
		border-radius: var(--radius-sm);
	}

	.grid {
		border-collapse: collapse;
		font-size: 10px;
	}

	.grid th,
	.grid td {
		padding: 0;
		text-align: center;
		vertical-align: middle;
		line-height: 1;
	}

	.grid thead th {
		position: sticky;
		top: 0;
		padding: 3px 4px;
		font-weight: var(--weight-medium);
		color: var(--text-3);
		background: var(--bg-1);
		white-space: nowrap;
	}

	.grid th.dof {
		position: sticky;
		left: 0;
		padding: 0 6px;
		font-family: var(--font-mono);
		font-weight: normal;
		color: var(--text-2);
		text-align: right;
		background: var(--bg-1);
		white-space: nowrap;
	}

	.grid td.role {
		padding: 0 4px;
		color: var(--text-4);
		cursor: pointer;
	}

	.grid td.role:hover {
		color: var(--text-1);
	}

	.grid th.dof.editing {
		padding: 0 2px;
	}

	.dofedit {
		width: 7ch;
		padding: 0 2px;
		font-size: 10px;
	}

	.grid td.cell {
		width: 22px;
		height: 20px;
		cursor: default;
	}

	.grid td.cell:hover {
		background: var(--bg-3);
	}

	.grid td.cell.picked {
		background: var(--blue-weak);
		box-shadow: inset 0 0 0 1px var(--blue-line);
	}

	.grid td.hole {
		width: 22px;
		height: 20px;
		background: repeating-linear-gradient(45deg, transparent 0 3px, var(--line-1) 3px 4px);
	}
</style>
