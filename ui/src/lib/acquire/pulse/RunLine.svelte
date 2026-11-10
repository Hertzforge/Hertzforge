<script lang="ts">
	/** A line of a test type's own in the Run card: a hold or a stall, its mark, its words. */
	interface Props {
		/** A warning carries a triangle and a heavier border; a note, a plain one. */
		tone: 'warn' | 'note';
		head: string;
		msg?: string | null;
		testid: string;
	}
	let { tone, head, msg = null, testid }: Props = $props();
</script>

<div class="line {tone}" role={tone === 'warn' ? 'alert' : 'status'} data-testid={testid}>
	<span class="mk" aria-hidden="true">
		{#if tone === 'warn'}
			<svg viewBox="0 0 14 14" width="16" height="16"><path d="M7 1.6l5.6 10H1.4z" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round" /><path d="M7 5.4v3" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
		{:else}
			<svg viewBox="0 0 14 14" width="16" height="16"><circle cx="7" cy="7" r="6" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M7 6.2v4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /><circle cx="7" cy="4.1" r="0.9" fill="currentColor" /></svg>
		{/if}
	</span>
	<div class="what">
		<p class="head"><strong>{head}</strong></p>
		{#if msg}<p class="msg">{msg}</p>{/if}
	</div>
</div>

<style>
	.line {
		display: flex;
		align-items: flex-start;
		gap: var(--sp-2);
		padding: var(--sp-2);
		background: var(--bg-2);
		border: 1px solid var(--line-2);
		border-radius: var(--radius-md);
	}

	.line.warn {
		color: var(--amber-1);
		background: var(--amber-weak);
		border: 2px solid var(--amber-2);
	}

	.mk {
		display: inline-flex;
		margin-top: 1px;
	}

	.what {
		flex: 1;
		min-width: 0;
	}

	.head {
		font-size: var(--text-sm);
		color: var(--text-1);
	}

	.msg {
		margin-top: 2px;
		font-size: var(--text-xs);
		color: var(--text-2);
	}
</style>
