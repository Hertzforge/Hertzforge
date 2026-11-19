<script lang="ts">
	/**
	 * A breakpoint table's actions line: the table's own buttons (Add Row), then its File menu, and
	 * under them what the last Open, Save or Export did. `onopen` puts a file's rows in the table's
	 * place, or throws to refuse it in words; either way the table says what happened.
	 */
	import type { Snippet } from 'svelte';
	import TableFileMenu from '$lib/tables/TableFileMenu.svelte';
	import type { FileResult, LoadedTable, TableData, TableFileClient } from '$lib/tables/tableFiles';
	import Findings from './Findings.svelte';

	interface Props {
		client: TableFileClient | null;
		/** Which breakpoint table the file holds (`random_profile` and so on). */
		tableKind: string;
		/** What the table is called, in the dialogs' titles and as a new file's name. */
		name: string;
		table: () => TableData;
		onopen: (loaded: LoadedTable) => FileResult[];
		/** The table cannot change: a file is not opened into it, but it can be saved. */
		locked: boolean;
		testid: string;
		children?: Snippet;
	}
	let { client, tableKind, name, table, onopen, locked, testid, children }: Props = $props();

	let path = $state<string | null>(null);
	let results = $state<FileResult[]>([]);
</script>

<div class="tfb">
	<div class="line">
		{@render children?.()}
		<TableFileMenu
			{client}
			kind="breakpoints"
			{tableKind}
			{name}
			{table}
			onopen={(loaded) => onopen(loaded)}
			bind:path
			openBlocked={locked ? 'The table is locked while a run is live.' : null}
			onresult={(items) => (results = items)}
			align="start"
			testid="{testid}-file"
		/>
	</div>
	{#if results.length}
		<div class="results" data-testid="{testid}-file-results">
			<Findings items={results} />
			<button class="aq-icon-btn" aria-label="Dismiss" onclick={() => (results = [])}>
				<svg viewBox="0 0 12 12" width="10" height="10" aria-hidden="true"><path d="M3 3l6 6M9 3l-6 6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
			</button>
		</div>
	{/if}
</div>

<style>
	.tfb {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		margin-top: var(--sp-2);
	}

	.line {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2);
	}

	.results {
		display: flex;
		align-items: flex-start;
		gap: var(--sp-2);
	}

	.results :global(.aq-findings) {
		flex: 1;
		margin: 0;
	}
</style>
