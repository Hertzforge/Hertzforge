<script lang="ts">
	/**
	 * The run's law in the dock: its name and version, the values its manifest declares on its last
	 * call (by their labels and units), its unusable calls in a row, the ticks a guard bound, the
	 * ticks a drive was over its ceiling, the largest turn of a drive's phase it has asked for, and
	 * its requests made and granted.
	 */
	import type { LawEntry } from '$lib/protocol/mimo';
	import type { MimoSineLawLive, MimoSineLawRef } from '$lib/protocol/mimoSine';
	import { formatNumber } from '$lib/viz/format';

	interface Props {
		law: MimoSineLawRef | null;
		live: MimoSineLawLive | null;
		entry: LawEntry | null;
	}
	let { law, live, entry }: Props = $props();

	const values = $derived(
		Object.entries(live?.values ?? {}).map(([k, v]) => {
			const d = entry?.diagnostics.find((x) => x.key === k);
			return { key: k, label: d?.label ?? k, unit: d?.unit ?? '', help: d?.help ?? '', value: v };
		})
	);
</script>

<div class="lawcard" data-testid="msine-law-card">
	{#if law}
		<p class="name"><strong>{law.name}</strong> <span class="num aq-muted">{law.version}</span></p>
		{#if values.length}
			<dl data-testid="msine-law-values">
				{#each values as v (v.key)}<div title={v.help}><dt>{v.label}</dt><dd class="num">{formatNumber(v.value, 4)}{v.unit ? ` ${v.unit}` : ''}</dd></div>{/each}
			</dl>
		{:else}
			<p class="aq-small aq-muted">The law's values are shown after its first call.</p>
		{/if}
		<dl>
			<div><dt>Unusable calls in a row</dt><dd class="num">{live?.misses ?? law.misses}</dd></div>
			<div><dt>Ticks a guard bound</dt><dd class="num">{live?.capped_ticks ?? law.capped_ticks}</dd></div>
			<div><dt>Ticks over a ceiling</dt><dd class="num">{law.over_ceiling}</dd></div>
			<div><dt>Largest turn asked for</dt><dd class="num">{law.rotation_max_deg === null ? '-' : `${formatNumber(law.rotation_max_deg, 3)} deg`}</dd></div>
			<div><dt>Requests</dt><dd class="num">{law.requests.asked} made, {law.requests.granted} granted</dd></div>
		</dl>
		{#if live?.faulted}<p class="aq-small warn">The law judges itself faulted. The controller's guards decide; this is shown only.</p>{/if}
	{:else}
		<p class="aq-small aq-muted">The run's law is shown once the setup is applied.</p>
	{/if}
</div>

<style>
	.lawcard {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		font-size: var(--text-xs);
	}

	.name {
		margin: 0;
	}

	dl {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: 4px var(--sp-3);
		margin: 0;
	}

	dt {
		font-size: var(--text-2xs);
		color: var(--text-3);
	}

	dd {
		margin: 0;
	}

	.warn {
		color: var(--amber-1);
	}
</style>
