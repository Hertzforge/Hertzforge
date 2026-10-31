<script lang="ts">
	/**
	 * Named values with a status each. `list` stacks rows (side panels); `strip` lays them out as
	 * tiles under a chart. Status shows as symbol and word as well as color.
	 */
	import type { ReadingView } from '$lib/protocol/types';
	import { formatNumber } from './format';
	import StatusMark from './StatusMark.svelte';

	interface Props {
		view: ReadingView;
		layout?: 'list' | 'strip';
		showTitle?: boolean;
	}

	let { view, layout = 'list', showTitle = true }: Props = $props();

	function valueText(v: number | string): string {
		return typeof v === 'number' ? formatNumber(v, 4) : v;
	}
</script>

<section class="reading {layout}" data-testid="view-reading" aria-label={view.title}>
	{#if showTitle}<h3 class="title">{view.title}</h3>{/if}
	<dl>
		{#each view.rows as row, i (row.label + i)}
			<div class="row {row.status}" class:long={typeof row.value === 'string' && row.value.length > 24}>
				<dt title={row.label}>{row.label}</dt>
				<dd>
					<span class="value num">{valueText(row.value)}</span>{#if row.unit}<span class="unit">{row.unit}</span>{/if}
				</dd>
				<dd class="status"><StatusMark status={row.status} /></dd>
			</div>
		{/each}
	</dl>
</section>

<style>
	.reading {
		min-width: 0;
	}

	.title {
		margin-bottom: var(--sp-2);
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
	}

	dl {
		margin: 0;
	}

	dd {
		margin: 0;
	}

	.value {
		color: var(--text-1);
	}

	.unit {
		margin-left: 4px;
		font-size: var(--text-xs);
		color: var(--text-3);
	}

	/* List: label left, value right, status at the end */
	.list .row {
		display: grid;
		grid-template-columns: minmax(0, 1fr) auto 4.6em;
		align-items: baseline;
		gap: var(--sp-3);
		padding: 7px 0;
		border-bottom: 1px solid var(--line-1);
		font-size: var(--text-sm);
	}

	/* A sentence-length value takes the full width under its label. */
	.list .row.long {
		grid-template-columns: minmax(0, 1fr) 4.6em;
		row-gap: 2px;
	}

	.list .row.long dd:first-of-type {
		grid-column: 1 / -1;
		grid-row: 2;
		text-align: left;
		white-space: normal;
	}

	.list .row.long .status {
		grid-column: 2;
		grid-row: 1;
	}

	.list .row:last-child {
		border-bottom: 0;
	}

	.list dt {
		color: var(--text-2);
		min-width: 0;
		overflow-wrap: anywhere;
	}

	.list dd:first-of-type {
		text-align: right;
		white-space: nowrap;
	}

	.list .status {
		display: flex;
		justify-content: flex-start;
	}

	.list .row.fail {
		margin: 0 calc(-1 * var(--sp-2));
		padding-left: var(--sp-2);
		padding-right: var(--sp-2);
		background: var(--amber-weak);
		border-radius: var(--radius-sm);
	}

	/* Strip: tiles */
	.strip dl {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
		gap: 6px;
	}

	.strip .row {
		display: grid;
		grid-template-rows: auto auto;
		grid-template-columns: minmax(0, 1fr) auto;
		align-content: start;
		gap: 2px var(--sp-2);
		padding: 7px 10px 8px;
		background: var(--bg-2);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-md);
	}

	.strip dt {
		grid-column: 1;
		font-size: var(--text-2xs);
		color: var(--text-3);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.strip .status {
		grid-column: 2;
		grid-row: 1;
	}

	.strip dd:first-of-type {
		grid-column: 1 / -1;
		grid-row: 2;
		font-size: var(--text-md);
		line-height: 1.3;
		overflow-wrap: anywhere;
	}

	.strip .row.fail {
		border-color: var(--amber-2);
		background: var(--amber-weak);
	}

	.strip .row.warn {
		border-color: var(--amber-line);
	}
</style>
