<script lang="ts">
	/**
	 * The limit channels during a run, one row each: the channel's name, how far its worst line is
	 * over its ceiling (negative: under) and that line's frequency. Over the ceiling is a triangle
	 * and the word, never color alone. The rows come once the run has started.
	 */
	import { hz } from './mmixWords';
	import type { MimoMixedStore } from './mmixStore.svelte';

	interface Props {
		mm: MimoMixedStore;
	}
	let { mm }: Props = $props();

	const limits = $derived(mm.status?.limits ?? []);
	const stated = $derived(mm.runConfig?.limits?.length ?? mm.setup.limits.length);
	const dB = (v: number | null) => (v === null || !Number.isFinite(v) ? '-' : `${v >= 0 ? '+' : '-'}${Math.abs(v).toFixed(2)} dB`);
</script>

<section aria-label="Limits" data-testid="mmix-limits-card">
	{#if !limits.length}
		<p class="aq-small aq-muted">{stated ? 'The limit channels are read once the run starts.' : 'The setup has no limit channels.'}</p>
	{:else}
		<div class="aq-scroll">
			<table class="aq-table" data-testid="mmix-limits-table">
				<thead><tr><th>Channel</th><th class="num" title="The worst line against the ceiling: negative is under it.">Against ceiling</th><th class="num">At</th><th>State</th></tr></thead>
				<tbody>
					{#each limits as l (l.channel)}
						{@const over = l.over_db !== null && l.over_db > 0}
						<tr data-testid="mmix-limit-{l.channel}" data-over={over}>
							<td>{mm.channelLabel(l.channel) || `Channel ${l.channel}`}</td>
							<td class="num" class:warn={over}>{dB(l.over_db)}</td>
							<td class="num">{hz(l.at_hz)}</td>
							<td class="state">
								{#if over}<span class="mark warn" aria-hidden="true"><svg viewBox="0 0 12 12" width="12" height="12"><path d="M6 1l5 9.5H1z" fill="currentColor" /></svg></span>Over the ceiling{:else if l.over_db === null}<span class="aq-muted">Not read yet</span>{:else}Under the ceiling{/if}
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
</section>

<style>
	td.num,
	.state {
		white-space: nowrap;
	}

	.mark {
		display: inline-flex;
		margin-right: 4px;
		vertical-align: -1px;
	}

	.mark.warn,
	td.warn {
		color: var(--amber-1);
	}
</style>
