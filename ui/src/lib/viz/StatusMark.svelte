<script lang="ts">
	/** A status as symbol, word and color together. Never color alone. */
	import type { Status } from '$lib/protocol/types';

	interface Props {
		status: Status;
		/** Show the word beside the symbol. Info has no word. */
		word?: boolean;
	}

	let { status, word = true }: Props = $props();

	const WORDS: Record<Status, string> = { pass: 'Pass', warn: 'Warn', fail: 'Fail', info: 'Info' };
</script>

<span class="mark {status}" aria-label={WORDS[status]} role="img">
	<svg viewBox="0 0 14 14" width="13" height="13" aria-hidden="true">
		{#if status === 'pass'}
			<circle cx="7" cy="7" r="6" fill="none" stroke="currentColor" stroke-width="1.3" />
			<path d="M4.2 7.2l1.9 1.9 3.8-4" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" />
		{:else if status === 'warn'}
			<path d="M7 1.6l5.6 10H1.4z" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round" />
			<path d="M7 5.4v3" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" />
			<circle cx="7" cy="10.1" r="0.85" fill="currentColor" />
		{:else if status === 'fail'}
			<path d="M4.5 1h5L13 4.5v5L9.5 13h-5L1 9.5v-5z" fill="currentColor" />
			<path d="M5 5l4 4M9 5l-4 4" stroke="var(--status-fail-text)" stroke-width="1.6" stroke-linecap="round" />
		{:else}
			<circle cx="7" cy="7" r="2" fill="currentColor" opacity="0.7" />
		{/if}
	</svg>
	{#if word && status !== 'info'}<span class="word">{WORDS[status]}</span>{/if}
</span>

<style>
	.mark {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		font-size: var(--text-2xs);
		font-weight: var(--weight-strong);
		letter-spacing: 0.04em;
		text-transform: uppercase;
		white-space: nowrap;
	}

	svg {
		flex: none;
	}

	.pass {
		color: var(--status-pass);
	}

	.info {
		color: var(--status-info);
	}

	.warn {
		color: var(--status-warn);
	}

	.fail {
		color: var(--status-fail-fill);
	}

	.fail .word {
		padding: 1px 5px;
		color: var(--status-fail-text);
		background: var(--status-fail-fill);
		border-radius: 3px;
	}
</style>
