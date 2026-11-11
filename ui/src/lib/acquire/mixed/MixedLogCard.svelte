<script lang="ts">
	/**
	 * The run's log, newest first: each entry's time, its kind in a word and the service's words. A
	 * part switched on or off (by its window, its end, or a screen) is logged as a Part entry.
	 */
	import type { MixedLogEntry } from '$lib/protocol/mixed';
	import { LOG_WORDS } from '../words';

	interface Props {
		log: MixedLogEntry[];
	}
	let { log }: Props = $props();

	const WORDS: Record<string, string> = { ...LOG_WORDS, part: 'Part' };
	const recent = $derived([...log].reverse().slice(0, 200));
	const time = (s: number) => new Date(s * 1000).toLocaleTimeString([], { hour12: false });
</script>

{#if recent.length}
	<ol class="log" data-testid="mixed-log">
		{#each recent as e (e.seq)}
			<li data-kind={e.kind}><span class="t num">{time(e.at_unix)}</span><span class="k">{WORDS[e.kind] ?? e.kind}</span><span class="x">{e.text}</span></li>
		{/each}
	</ol>
{:else}
	<p class="aq-muted aq-small">Nothing logged yet.</p>
{/if}

<style>
	.log {
		display: flex;
		flex-direction: column;
		gap: 2px;
		margin: 0;
		padding: 0;
		list-style: none;
		font-size: var(--text-xs);
	}

	li {
		display: grid;
		grid-template-columns: auto 4.5em minmax(0, 1fr);
		gap: var(--sp-2);
	}

	.t,
	.k {
		color: var(--text-3);
	}
</style>
