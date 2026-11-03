<script lang="ts">
	/**
	 * Rows of text under column headings. The headings stay at the top as the rows scroll; a cell
	 * that reads as a number sits at the right of its column. Copy puts the table on the clipboard
	 * as tab-separated text, headings first, so it pastes into a spreadsheet.
	 */
	import type { TableView } from '$lib/protocol/types';
	import { tableText, numeric } from './table';

	interface Props {
		view: TableView;
		showTitle?: boolean;
	}

	let { view, showTitle = true }: Props = $props();
	let copied = $state(false);

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
						{#each row as cell, i (i)}<td class:num={numeric(cell)}>{cell}</td>{/each}
					</tr>
				{/each}
			</tbody>
		</table>
		{#if view.rows.length === 0}<p class="empty">No rows.</p>{/if}
	</div>
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
</style>
