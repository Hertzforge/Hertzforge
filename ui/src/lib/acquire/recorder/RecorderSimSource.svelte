<script lang="ts">
	/**
	 * The simulator's stated source, named whenever a Recorder run is on the simulator: its plant
	 * moves only when driven and a Recorder drives nothing, so the service lays a fixed source on
	 * the recorded rows (PROTOCOL.md, "The Recorder on the Simulator").
	 */
	interface Props {
		/** The recorded rows' indexes, in table order. */
		rows: number[];
	}
	let { rows }: Props = $props();

	const SIGNALS = ['markers: a half-sine of 0.5 peak lasting 10 ms, 0.5 s into every 2.000 s', '250 Hz at 0.2 peak', '433 Hz at 0.1 peak', '1000 Hz at 0.15 peak'];
	const lines = $derived(rows.map((index, k) => ({ index, text: SIGNALS[k % 4] })));
</script>

<section class="src" aria-labelledby="rec-sim-src" data-testid="recorder-sim-source">
	<h3 id="rec-sim-src">The Simulator's Source</h3>
	<p class="aq-small aq-muted">The simulator moves only when it is driven, and a Recorder run drives nothing, so its inputs carry this source, repeating every 2.000 s from the run's first frame, on its noise. Levels are in each row's unit.</p>
	{#if lines.length}
		<ul>
			{#each lines as l (l.index)}<li><span class="num">Row {l.index}</span>: {l.text}</li>{/each}
		</ul>
	{:else}
		<p class="aq-small">The first recorded row carries the markers; the next three 250, 433 and 1000 Hz; a fifth row and after repeat the four.</p>
	{/if}
</section>

<style>
	.src h3 {
		margin: 0 0 4px;
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
	}

	.src p {
		margin: 0 0 4px;
	}

	ul {
		margin: 0;
		padding-left: 1.1em;
		font-size: var(--text-sm);
	}
</style>
