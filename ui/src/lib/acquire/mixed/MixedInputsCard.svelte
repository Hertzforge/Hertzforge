<script lang="ts">
	/**
	 * Every enabled input during the run: its rms over the band with the tones in, and whether its
	 * range has overloaded this run (latched), with a mark and a word, never color alone. Also the
	 * loop and tick counts the frame is from.
	 */
	import type { MixedLiveHead } from '$lib/protocol/mixed';
	import { formatNumber } from '$lib/viz/format';

	interface Props {
		live: MixedLiveHead | null;
	}
	let { live }: Props = $props();
</script>

{#if live?.channels.length}
	<table class="aq-table" data-testid="mixed-inputs">
		<thead><tr><th class="num">Ch</th><th>Input</th><th class="num">Rms</th><th>Range</th></tr></thead>
		<tbody>
			{#each live.channels as c (c.index)}
				<tr data-overload={c.overload}>
					<td class="num">{c.index}</td>
					<td>{c.description || c.role}</td>
					<td class="num">{c.rms === null ? '-' : `${formatNumber(c.rms, 3)} ${c.unit}`}</td>
					<td class:warn={c.overload}>{#if c.overload}<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M6 1l5 9.5H1z" fill="currentColor" /></svg> Overload{:else}OK{/if}</td>
				</tr>
			{/each}
		</tbody>
	</table>
	<p class="aq-small aq-muted" data-testid="mixed-counts">Loop {live.loop}, tick {live.tick}.</p>
{:else}
	<p class="aq-muted aq-small">Appears when a run starts.</p>
{/if}

<style>
	.warn {
		color: var(--amber-1);
	}

	p {
		margin: var(--sp-1) 0 0;
	}
</style>
