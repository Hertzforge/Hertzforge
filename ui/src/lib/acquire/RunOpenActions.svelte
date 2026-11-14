<script lang="ts">
	/**
	 * A finished run's way to its data, on the end line of every test type that keeps a run folder:
	 * Open in Analysis (then Show in Analysis), which converts the folder and opens it in the
	 * analysis workspace that suits the run, and Copy Folder Path; each with the line that says how
	 * it went. The status is a word and a mark, never a colour alone.
	 */
	import type { RunRecordRef } from '$lib/protocol/types';
	import { runOpenWords, type RunOpen } from './runOpen.svelte';

	interface Props {
		record: RunRecordRef;
		opener: RunOpen;
	}
	let { record, opener }: Props = $props();

	const opening = $derived(opener.at(record.folder));
	const openWords = $derived(runOpenWords(opening, opener.refusal, opener.added));
</script>

<div class="endfoot">
	<div class="endacts" data-testid="run-end-actions">
		<button class="btn sm" data-testid="act-open-analysis" data-status={opening} aria-busy={opening === 'working'} aria-disabled={opening === 'working'} class:off={opening === 'working'} title="Convert the run folder and open its data in the analysis side." onclick={() => void opener.open(record.folder)}>
			{#if opening === 'working'}<svg class="spin" viewBox="0 0 12 12" width="12" height="12" aria-hidden="true"><path d="M6 1.5a4.5 4.5 0 104.5 4.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" /></svg>{/if}
			{openWords.label}
		</button>
		<button class="btn sm ghost" data-testid="act-copy-path" title={record.folder} onclick={() => void opener.copyPath(record.folder)}>Copy Folder Path</button>
	</div>
	{#if openWords.line}
		<p class="openline {opening}" role={opening === 'refused' ? 'alert' : 'status'} data-testid="open-analysis-status" data-status={opening}>
			{#if opening === 'done'}
				<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><path d="M2.5 7.3l2.8 2.8 6-6.3" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" /></svg>
			{:else if opening === 'refused'}
				<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><path d="M4.5 1h5L13 4.5v5L9.5 13h-5L1 9.5v-5z" fill="currentColor" /><path d="M4.8 7h4.4" stroke="var(--bg-1)" stroke-width="1.7" stroke-linecap="round" /></svg>
			{/if}
			<span>{openWords.line}</span>
		</p>
	{/if}
	{#if opener.copy !== 'idle'}
		<p class="openline" role="status" data-testid="copy-path-status">{opener.copy === 'copied' ? "Copied the folder's path." : 'The clipboard cannot be written here. The path is shown above, to select and copy.'}</p>
	{/if}
</div>

<style>
	/* The end line's actions take its whole width, under its words. */
	.endfoot {
		flex-basis: 100%;
		padding-left: 24px;
		color: var(--text-2);
	}

	.endacts {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
	}

	.btn.off {
		opacity: 0.45;
		cursor: default;
	}

	.openline {
		display: flex;
		align-items: flex-start;
		gap: 5px;
		margin-top: 4px;
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	.openline svg {
		flex: none;
		margin-top: 2px;
	}

	.openline.done {
		color: var(--blue-1);
	}

	.openline.refused {
		font-weight: var(--weight-strong);
		color: var(--amber-1);
	}

	.spin {
		animation: spin 0.8s linear infinite;
	}

	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}
</style>
