<script lang="ts">
	/**
	 * Rows of text under column headings. The headings stay at the top as the rows scroll; a cell
	 * that reads as a number sits at the right of its column. Copy puts the table on the clipboard
	 * as tab-separated text, headings first, so it pastes into a spreadsheet. A table with `edit` is
	 * typed into in place, as Visual Dynamics' tables are: click a cell, type, and Enter moves down,
	 * Tab across; tab-separated text pasted into a cell sets the block it covers. A value refused
	 * leaves the cell as it was, with the reason under the table.
	 */
	import { tick } from 'svelte';
	import type { TableView } from '$lib/protocol/types';
	import { numeric, parseTsv, pasteTargets, tableText } from './table';

	interface Props {
		view: TableView;
		showTitle?: boolean;
		/** A cell typed into; resolves to the refusal's words, or null. */
		onedit?: (row: number, column: string, value: string) => Promise<string | null>;
	}

	let { view, showTitle = true, onedit }: Props = $props();
	let copied = $state(false);
	let editing = $state<{ row: number; col: number } | null>(null);
	let draft = $state('');
	let refusal = $state<string | null>(null);
	let saving = $state(false);
	let field = $state<HTMLInputElement | null>(null);

	const keys = $derived(onedit && view.edit ? view.edit.columns : []);
	const editable = (col: number) => !!keys[col];

	async function open(row: number, col: number) {
		if (!editable(col) || saving) return;
		editing = { row, col };
		draft = view.rows[row]?.[col] ?? '';
		await tick();
		field?.focus();
		field?.select();
	}

	/** Commit the cell, then move by (dr, dc) to the next editable cell, if any. */
	async function commit(dr = 0, dc = 0) {
		const at = editing;
		if (!at || !onedit) return;
		const was = view.rows[at.row]?.[at.col] ?? '';
		editing = null;
		if (draft !== was) {
			saving = true;
			refusal = await onedit(at.row, keys[at.col] as string, draft);
			saving = false;
			if (refusal) return;
		} else refusal = null;
		if (!dr && !dc) return;
		let row = at.row + dr;
		let col = at.col + dc;
		while (dc && col < keys.length && !editable(col)) col += dc;
		if (row >= 0 && row < view.rows.length && col >= 0 && col < keys.length) await open(row, col);
	}

	function key(e: KeyboardEvent) {
		if (e.key === 'Enter') {
			e.preventDefault();
			void commit(e.shiftKey ? -1 : 1, 0);
		} else if (e.key === 'Tab') {
			e.preventDefault();
			void commit(0, e.shiftKey ? -1 : 1);
		} else if (e.key === 'Escape') {
			editing = null;
		}
	}

	/** A block pasted into a cell sets the cells it covers, one by one, stopping at a refusal. */
	async function paste(e: ClipboardEvent) {
		const text = e.clipboardData?.getData('text/plain') ?? '';
		const at = editing;
		if (!at || !onedit || !/[\t\n]/.test(text.replace(/\n$/, ''))) return;
		e.preventDefault();
		editing = null;
		saving = true;
		refusal = null;
		for (const target of pasteTargets(parseTsv(text), at.row, at.col, keys, view.rows.length)) {
			refusal = await onedit(target.row, target.key, target.value);
			if (refusal) break;
		}
		saving = false;
	}

	async function copy() {
		await navigator.clipboard.writeText(tableText(view));
		copied = true;
		setTimeout(() => (copied = false), 1500);
	}
</script>

<section class="table" data-testid="view-table" aria-label={view.title}>
	<header>
		{#if showTitle}<h3 class="title">{view.title}</h3>{/if}
		<button class="btn ghost sm" data-testid="table-copy" onclick={copy} title="Copy the table as tab-separated text">
			{copied ? 'Copied' : 'Copy'}
		</button>
	</header>
	<div class="scroll">
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
									<input bind:this={field} bind:value={draft} class="cell mono" aria-label="{view.columns[i]}, row {r + 1}"
										onkeydown={key} onpaste={paste} onblur={() => commit()} data-testid="table-cell-input" />
								</td>
							{:else}
								<td class:num={numeric(cell)} class:editable={editable(i)} onclick={() => open(r, i)}>{cell}</td>
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
</style>
