<script lang="ts">
	/** Findings as a list: each with its symbol and its word (Error or Warning), never a color alone. */
	interface Props {
		items: { level: 'error' | 'warning' | 'note'; message: string }[];
		id?: string;
		testid?: string;
	}
	let { items, id, testid }: Props = $props();
</script>

{#if items.length}
	<ul class="aq-findings" {id} data-testid={testid} aria-live="polite">
		{#each items as f, i (i)}
			<li class={f.level}>
				{#if f.level === 'error'}
					<svg viewBox="0 0 14 14" width="11" height="11" aria-hidden="true"><path d="M4.5 1h5L13 4.5v5L9.5 13h-5L1 9.5v-5z" fill="currentColor" /><path d="M5 5l4 4M9 5l-4 4" stroke="var(--status-fail-text)" stroke-width="1.7" stroke-linecap="round" /></svg>
					<span class="tag">Error</span>
				{:else if f.level === 'warning'}
					<svg viewBox="0 0 14 14" width="11" height="11" aria-hidden="true"><path d="M7 1.6l5.6 10H1.4z" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round" /></svg>
					<span class="tag">Warning</span>
				{:else}
					<svg viewBox="0 0 14 14" width="11" height="11" aria-hidden="true"><circle cx="7" cy="7" r="6" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M7 6.2v4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /><circle cx="7" cy="4.1" r="0.9" fill="currentColor" /></svg>
					<span class="tag">Note</span>
				{/if}
				<span>{f.message}</span>
			</li>
		{/each}
	</ul>
{/if}
