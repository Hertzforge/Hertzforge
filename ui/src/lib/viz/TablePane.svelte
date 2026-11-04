<script lang="ts">
	/**
	 * Rows of text under column headings. The headings stay at the top as the rows scroll; a cell
	 * that reads as a number sits at the right of its column. Copy puts the table on the clipboard
	 * as tab-separated text, headings first, so it pastes into a spreadsheet.
	 *
	 * A table with `edit` behaves as Visual Dynamics' tables do, every one alike. A click selects a
	 * cell, a drag or Shift a block, and the arrows move. Typing, Enter or a double-click opens the
	 * cell; Enter commits and moves down, Tab across, Escape cancels. Typed with a block selected,
	 * the value goes to every cell of it (Batch Edit). Delete clears the block. Cmd or Ctrl+C copies
	 * it, and tab-separated text pasted sets the cells it covers. The square at the selection's
	 * corner is the fill handle: dragged, it repeats the block down, up or across; double-clicked,
	 * down to the last row. Each value goes through the table's own operation, so one it refuses
	 * leaves its cell as it was, with the reason under the table.
	 */
	import { tick } from 'svelte';
	import type { TableView } from '$lib/protocol/types';
	import { blockText, fillTargets, flagOn, inside, numeric, parseTsv, pasteTargets, spanned, tableHtml, tableText, type Block } from './table';

	interface Props {
		view: TableView;
		showTitle?: boolean;
		/** A cell typed into; resolves to the refusal's words, or null. */
		onedit?: (row: number, column: string, value: string) => Promise<string | null>;
	}

	let { view, showTitle = true, onedit }: Props = $props();
	let copied = $state(false);
	let anchor = $state<{ row: number; col: number } | null>(null);
	let focus = $state<{ row: number; col: number } | null>(null);
	let editing = $state<{ row: number; col: number; block: Block | null } | null>(null);
	let draft = $state('');
	let refusal = $state<string | null>(null);
	let saving = $state(false);
	let field = $state<HTMLInputElement | null>(null);
	let grid = $state<HTMLDivElement | null>(null);
	let dragging = $state<'select' | 'fill' | null>(null);
	let fillTo = $state<{ row: number; col: number } | null>(null);

	const keys = $derived(onedit && view.edit ? view.edit.columns : []);
	/** How a column is edited: a flag by its check box, a choice from its list, a date in a date field. */
	const kindOf = (col: number) => view.edit?.kinds?.[keys[col] ?? ''] ?? 'text';
	const choicesOf = (col: number) => view.edit?.choices?.[keys[col] ?? ''];

	/** A flag cell clicked: set the other way, with no field to type into. */
	async function toggleFlag(row: number, col: number) {
		if (!onedit || saving || !editable(col)) return;
		select(row, col);
		await apply([{ row, key: keys[col] as string, value: flagOn(view.rows[row]?.[col] ?? '') ? 'False' : 'True' }]);
	}
	const editable = (col: number) => !!keys[col];
	const block = $derived(anchor && focus ? spanned(anchor, focus) : null);
	const fillBlock = $derived(block && fillTo && dragging === 'fill' ? spanned({ row: Math.min(block.r0, fillTo.row), col: Math.min(block.c0, fillTo.col) }, { row: Math.max(block.r1, fillTo.row), col: Math.max(block.c1, fillTo.col) }) : null);

	function select(row: number, col: number, extend = false) {
		if (!extend || !anchor) anchor = { row, col };
		focus = { row, col };
		grid?.focus();
	}

	/** Set each cell in turn through the table's operation, stopping at the first refusal. */
	async function apply(targets: { row: number; key: string; value: string }[]) {
		if (!onedit || !targets.length) return;
		saving = true;
		refusal = null;
		for (const t of targets) {
			refusal = await onedit(t.row, t.key, t.value);
			if (refusal) break;
		}
		saving = false;
		await tick();
		grid?.focus();
	}

	/** The editable cells of a block, each set to `value`: Delete clears with '', Batch Edit types. */
	function everywhere(b: Block, value: string) {
		const out: { row: number; key: string; value: string }[] = [];
		for (let r = b.r0; r <= b.r1; r++) for (let c = b.c0; c <= b.c1; c++) if (keys[c]) out.push({ row: r, key: keys[c] as string, value });
		return out;
	}

	async function open(row: number, col: number, seed?: string) {
		if (!editable(col) || saving || !onedit) return;
		const many = block && (block.r1 > block.r0 || block.c1 > block.c0) && inside(block, row, col) ? block : null;
		editing = { row, col, block: many };
		draft = seed ?? view.rows[row]?.[col] ?? '';
		await tick();
		field?.focus();
		if (seed === undefined) field?.select();
	}

	/** Commit the cell (or, from a block, every cell of it), then move by (dr, dc). */
	async function commit(dr = 0, dc = 0) {
		const at = editing;
		if (!at || !onedit) return;
		editing = null;
		if (at.block) {
			await apply(everywhere(at.block, draft));
			return;
		}
		const was = view.rows[at.row]?.[at.col] ?? '';
		if (draft !== was) {
			await apply([{ row: at.row, key: keys[at.col] as string, value: draft }]);
			if (refusal) return;
		} else refusal = null;
		let row = at.row + dr;
		let col = at.col + dc;
		while (dc && col >= 0 && col < keys.length && !editable(col)) col += dc;
		if (row >= 0 && row < view.rows.length && col >= 0 && col < keys.length) select(row, col);
		else select(at.row, at.col);
	}

	function editKey(e: KeyboardEvent) {
		if (e.key === 'Enter') {
			e.preventDefault();
			void commit(e.shiftKey ? -1 : 1, 0);
		} else if (e.key === 'Tab') {
			e.preventDefault();
			void commit(0, e.shiftKey ? -1 : 1);
		} else if (e.key === 'Escape') {
			editing = null;
			grid?.focus();
		}
	}

	function gridKey(e: KeyboardEvent) {
		if (editing || !focus) return;
		const moves: Record<string, [number, number]> = { ArrowUp: [-1, 0], ArrowDown: [1, 0], ArrowLeft: [0, -1], ArrowRight: [0, 1] };
		const step = moves[e.key];
		if (step) {
			e.preventDefault();
			const row = Math.max(0, Math.min(view.rows.length - 1, focus.row + step[0]));
			const col = Math.max(0, Math.min(view.columns.length - 1, focus.col + step[1]));
			select(row, col, e.shiftKey);
		} else if ((e.key === 'Delete' || e.key === 'Backspace') && block) {
			e.preventDefault();
			void apply(everywhere(block, ''));
		} else if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'c' && block) {
			e.preventDefault();
			void navigator.clipboard.writeText(blockText(view.rows, block));
		} else if (e.key === 'Enter' || e.key === 'F2') {
			e.preventDefault();
			void open(focus.row, focus.col);
		} else if (e.key.length === 1 && !e.metaKey && !e.ctrlKey && !e.altKey) {
			e.preventDefault();
			void open(focus.row, focus.col, e.key);
		}
	}

	/** Pasted on the selection: a block sets the cells it covers; one value goes to every selected cell. */
	async function paste(e: ClipboardEvent) {
		const text = e.clipboardData?.getData('text/plain') ?? '';
		if (!onedit || !block) return;
		const multi = /[\t\n]/.test(text.replace(/\n$/, ''));
		if (editing && !multi) return;
		e.preventDefault();
		editing = null;
		if (multi) await apply(pasteTargets(parseTsv(text), block.r0, block.c0, keys, view.rows.length));
		else await apply(everywhere(block, text.replace(/\n$/, '')));
	}

	function cellAt(x: number, y: number): { row: number; col: number } | null {
		const td = (document.elementFromPoint(x, y) as HTMLElement | null)?.closest('td[data-r]') as HTMLElement | null;
		return td ? { row: Number(td.dataset.r), col: Number(td.dataset.c) } : null;
	}

	function down(e: PointerEvent, row: number, col: number) {
		if (e.button !== 0 || editing) return;
		select(row, col, e.shiftKey);
		dragging = 'select';
	}

	function grabHandle(e: PointerEvent) {
		e.preventDefault();
		e.stopPropagation();
		dragging = 'fill';
		fillTo = focus;
	}

	function moved(e: PointerEvent) {
		if (!dragging) return;
		const at = cellAt(e.clientX, e.clientY);
		if (!at) return;
		if (dragging === 'select') focus = at;
		else fillTo = at;
	}

	function released() {
		const wasFill = dragging === 'fill';
		dragging = null;
		if (wasFill && block && fillTo) {
			const targets = fillTargets(view.rows, block, fillTo.row, fillTo.col, keys);
			const to = fillTo;
			fillTo = null;
			void apply(targets).then(() => {
				anchor = { row: Math.min(block.r0, to.row), col: Math.min(block.c0, to.col) };
				focus = { row: Math.max(block.r1, to.row), col: Math.max(block.c1, to.col) };
			});
		}
	}

	/** Double-click on the handle: the block down to the last row. */
	function fillDown() {
		if (!block) return;
		void apply(fillTargets(view.rows, block, view.rows.length - 1, block.c1, keys));
	}

	/** The whole table on the clipboard as text and as HTML at once, so a spreadsheet pastes cells and a document a table. */
	async function copy() {
		const text = tableText(view);
		try {
			await navigator.clipboard.write([
				new ClipboardItem({
					'text/plain': new Blob([text], { type: 'text/plain' }),
					'text/html': new Blob([tableHtml(view)], { type: 'text/html' })
				})
			]);
		} catch {
			await navigator.clipboard.writeText(text);
		}
		copied = true;
		setTimeout(() => (copied = false), 1500);
	}
</script>

<svelte:window onpointermove={moved} onpointerup={released} />

<section class="table" data-testid="view-table" aria-label={view.title}>
	<header>
		{#if showTitle}<h3 class="title">{view.title}</h3>{/if}
		<button class="btn ghost sm" data-testid="table-copy" onclick={copy} title="Copy the table as tab-separated text">
			{copied ? 'Copied' : 'Copy'}
		</button>
	</header>
	<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
	<div class="scroll" bind:this={grid} tabindex={onedit && view.edit ? 0 : undefined} role={onedit && view.edit ? 'grid' : undefined}
		aria-label={onedit && view.edit ? `${view.title}, editable` : undefined} onkeydown={gridKey} onpaste={paste} data-testid="table-grid">
		<table>
			<thead>
				<tr>
					{#each view.columns as c, i (i)}<th scope="col">{c}</th>{/each}
				</tr>
			</thead>
			<tbody>
				{#each view.rows as row, r (r)}
					<tr>
						{#each row as cell, i (i)}
							{#if editing && editing.row === r && editing.col === i}
								<td class="editing">
									{#if kindOf(i) === 'choice' && choicesOf(i)}
										<select bind:value={draft} class="cell" aria-label="{view.columns[i]}, row {r + 1}" onchange={() => commit()} onkeydown={editKey} onblur={() => commit()} data-testid="table-cell-select">
											{#if !choicesOf(i)?.includes(draft)}<option value={draft}>{draft || '(blank)'}</option>{/if}
											{#each choicesOf(i) ?? [] as choice (choice)}<option value={choice}>{choice}</option>{/each}
										</select>
									{:else if kindOf(i) === 'date'}
										<input bind:this={field} bind:value={draft} type="date" class="cell mono" aria-label="{view.columns[i]}, row {r + 1}" onkeydown={editKey} onblur={() => commit()} data-testid="table-cell-input" />
									{:else}
										<input bind:this={field} bind:value={draft} class="cell mono" list={choicesOf(i) ? 'table-cell-choices' : undefined} aria-label="{view.columns[i]}, row {r + 1}"
											onkeydown={editKey} onblur={() => commit()} data-testid="table-cell-input" />
										{#if choicesOf(i)}
											<datalist id="table-cell-choices">
												{#each choicesOf(i) ?? [] as choice (choice)}<option value={choice}></option>{/each}
											</datalist>
										{/if}
									{/if}
								</td>
							{:else}
								<td data-r={r} data-c={i} class:num={numeric(cell)} class:editable={editable(i)}
									class:picked={onedit && view.edit && inside(block, r, i)} class:filling={inside(fillBlock, r, i) && !inside(block, r, i)}
									onpointerdown={(e) => onedit && view.edit && down(e, r, i)} ondblclick={() => kindOf(i) !== 'flag' && open(r, i)}>
									{#if editable(i) && kindOf(i) === 'flag'}
										<input type="checkbox" class="flag" checked={flagOn(cell)} aria-label="{view.columns[i]}, row {r + 1}" disabled={saving} onclick={(e) => { e.preventDefault(); void toggleFlag(r, i); }} data-testid="table-cell-flag" />
									{:else}
										{cell}
									{/if}
									{#if onedit && view.edit && block && r === block.r1 && i === block.c1 && !saving}
										<span class="handle" role="presentation" title="Drag to fill; double-click to fill down" onpointerdown={grabHandle} ondblclick={fillDown} data-testid="table-fill-handle"></span>
									{/if}
								</td>
							{/if}
						{/each}
					</tr>
				{/each}
			</tbody>
		</table>
		{#if view.rows.length === 0}<p class="empty">No rows.</p>{/if}
	</div>
	{#if saving}<p class="note" role="status">Saving.</p>{/if}
	{#if refusal}<p class="refusal" role="alert" data-testid="table-refusal">{refusal}</p>{/if}
</section>

<style>
	header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-3);
		margin-bottom: var(--sp-2);
	}

	.title {
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
	}

	.scroll {
		max-height: 420px;
		overflow: auto;
		border: 1px solid var(--line-1);
		border-radius: var(--radius-md);
	}

	table {
		width: 100%;
		border-collapse: collapse;
		font-size: var(--text-sm);
	}

	th {
		position: sticky;
		top: 0;
		background: var(--bg-2);
		color: var(--text-2);
		font-weight: var(--weight-strong);
		text-align: left;
		white-space: nowrap;
	}

	th,
	td {
		padding: 5px var(--sp-3);
		border-bottom: 1px solid var(--line-1);
	}

	td {
		color: var(--text-1);
	}

	td.num {
		text-align: right;
		font-variant-numeric: tabular-nums;
		white-space: nowrap;
	}

	.empty {
		padding: var(--sp-3);
		color: var(--text-3);
	}

	td.editable {
		cursor: text;
	}

	td.editable:hover {
		background: var(--bg-2);
	}

	.scroll:focus {
		outline: none;
	}

	td {
		position: relative;
		user-select: none;
	}

	td.picked {
		background: var(--blue-weak);
	}

	td.filling {
		background: color-mix(in srgb, var(--blue-weak) 50%, transparent);
		outline: 1px dashed var(--blue-line, var(--line-2));
	}

	.handle {
		position: absolute;
		right: 0;
		bottom: 0;
		width: 7px;
		height: 7px;
		background: var(--accent, var(--blue-line));
		border: 1px solid var(--bg-0);
		cursor: crosshair;
	}

	td.editing {
		padding: 0;
	}

	.cell {
		width: 100%;
		min-width: 6em;
		padding: 4px var(--sp-3);
		font: inherit;
		color: var(--text-1);
		background: var(--bg-2);
		border: 1px solid var(--line-2, var(--line-1));
		outline: none;
	}

	.note,
	.refusal {
		margin-top: var(--sp-2);
		font-size: var(--text-xs);
	}

	.note {
		color: var(--text-3);
	}

	.refusal {
		color: var(--status-warn);
	}
	.flag {
		margin: 0;
		accent-color: var(--accent, #0a84ff);
	}
</style>
