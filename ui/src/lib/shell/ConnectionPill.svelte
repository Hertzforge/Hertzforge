<script lang="ts">
	/** One service's connection: a word and a symbol, with color as the third cue. */
	import type { ServiceClient } from '$lib/protocol/client.svelte';

	interface Props {
		client: ServiceClient;
		label: string;
		testid?: string;
	}

	let { client, label, testid }: Props = $props();

	const word = $derived.by(() => {
		switch (client.status) {
			case 'open':
				return 'Connected';
			case 'connecting':
				return 'Connecting';
			case 'retrying':
				return client.retryInMs > 0 ? `Retry ${Math.ceil(client.retryInMs / 1000)} s` : 'Connecting';
			case 'incompatible':
				return 'Incompatible';
			default:
				return 'Offline';
		}
	});

	const title = $derived(
		[
			`${label} service, ${client.url}`,
			client.hello ? `${client.hello.protocol}, version ${client.hello.version}` : null,
			client.detail,
			client.status === 'open' ? null : 'Click to retry now.'
		]
			.filter(Boolean)
			.join('\n')
	);
</script>

<button class="pill {client.status}" data-testid={testid} data-status={client.status} {title} onclick={() => client.retryNow()} aria-label="{label}: {word}. {client.detail ?? ''}">
	<span class="sym" aria-hidden="true">
		{#if client.status === 'open'}
			<svg viewBox="0 0 12 12" width="11" height="11"><circle cx="6" cy="6" r="4.2" fill="currentColor" /></svg>
		{:else if client.status === 'connecting' || (client.status === 'retrying' && client.retryInMs === 0)}
			<svg class="spin" viewBox="0 0 12 12" width="11" height="11"><path d="M6 1.5a4.5 4.5 0 104.5 4.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" /></svg>
		{:else if client.status === 'retrying'}
			<svg viewBox="0 0 12 12" width="11" height="11"><circle cx="6" cy="6" r="4" fill="none" stroke="currentColor" stroke-width="1.5" stroke-dasharray="2 1.6" /></svg>
		{:else if client.status === 'incompatible'}
			<svg viewBox="0 0 12 12" width="11" height="11"><path d="M6 1.2l5 9.3H1z" fill="currentColor" /><path d="M6 4.6v2.6" stroke="var(--bg-1)" stroke-width="1.4" stroke-linecap="round" /></svg>
		{:else}
			<svg viewBox="0 0 12 12" width="11" height="11"><circle cx="6" cy="6" r="4" fill="none" stroke="currentColor" stroke-width="1.5" /></svg>
		{/if}
	</span>
	<span class="name">{label}</span>
	<span class="word">{word}</span>
</button>

<style>
	.pill {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		height: 26px;
		padding: 0 10px 0 8px;
		font-size: var(--text-xs);
		color: var(--text-2);
		background: var(--bg-2);
		border: 1px solid var(--line-2);
		border-radius: 13px;
		white-space: nowrap;
	}

	.pill:hover {
		border-color: var(--line-3);
	}

	.sym {
		display: inline-flex;
	}

	.name {
		font-weight: var(--weight-medium);
		color: var(--text-1);
	}

	.word {
		min-width: 9ch;
		font-variant-numeric: tabular-nums;
	}

	.open .sym {
		color: var(--blue-2);
	}

	.connecting .sym,
	.retrying .sym,
	.offline .sym {
		color: var(--text-3);
	}

	.retrying {
		border-color: var(--amber-line);
	}

	.retrying .word {
		color: var(--amber-2);
	}

	.incompatible {
		border-color: var(--amber-2);
		background: var(--amber-weak);
	}

	.incompatible .sym,
	.incompatible .word {
		color: var(--amber-2);
	}

	.spin {
		animation: spin 0.9s linear infinite;
	}

	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}
</style>
