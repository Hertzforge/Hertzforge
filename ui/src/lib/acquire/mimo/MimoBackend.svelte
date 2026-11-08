<script lang="ts">
	/**
	 * Where the test runs: the station (simulator or NI hardware) and how the channels are wired
	 * to it, exactly as for every test type, and each drive's feasibility. Before identification
	 * the drive a test needs is not known, so feasibility is shown as the service reports it: not
	 * evaluated until identification, then predicted on the Predict page.
	 */
	import { formatFixed } from '$lib/viz/format';
	import ChannelTable from '../ChannelTable.svelte';
	import StationPanel from '../StationPanel.svelte';
	import type { MimoState } from './mimoStore.svelte';

	interface Props {
		mimo: MimoState;
	}
	let { mimo }: Props = $props();

	const acq = $derived(mimo.acq);
	const feas = $derived(mimo.check && mimo.checkedKey === mimo.key ? mimo.check.feasibility : null);
	const pred = $derived(mimo.status?.prediction ?? null);
	const names = $derived(new Map(mimo.channels.drives.map((r) => [r.index, r.description.trim() || `Channel ${r.index}`])));

	const VERDICT: Record<string, string> = { pass: 'Within the ratings', advisory: 'Advisory', exceeded: 'Exceeds the ratings', not_evaluated: 'After identification' };
</script>

<div class="stack">
	<StationPanel {acq} />

	<section class="aq-panel" aria-labelledby="mf-title" data-testid="mimo-feasibility">
		<div class="aq-head">
			<h2 id="mf-title">Feasibility per Drive</h2>
			<span class="aq-sub">Each drive against its ceiling and the shaker behind it.</span>
		</div>
		{#if feas && feas.length}
			<table class="aq-table">
				<thead>
					<tr><th>Drive</th><th class="num">Ceiling</th><th class="num">Predicted</th><th>Verdict</th><th>Says</th></tr>
				</thead>
				<tbody>
					{#each feas as f, i (f.channel)}
						{@const p = pred?.drive_rms_v[i] ?? f.predicted_vrms}
						<tr data-testid="feas-{f.channel}" data-verdict={f.verdict}>
							<td>{names.get(f.channel) ?? `Channel ${f.channel}`}</td>
							<td class="num">{formatFixed(f.ceiling_vrms, 2)} V rms</td>
							<td class="num">{p === null || p === undefined ? '-' : `${formatFixed(p, 3)} V rms`}</td>
							<td class="verdict {f.verdict}">
								{#if f.verdict === 'exceeded'}
									<svg viewBox="0 0 14 14" width="11" height="11" aria-hidden="true"><path d="M4.5 1h5L13 4.5v5L9.5 13h-5L1 9.5v-5z" fill="currentColor" /></svg>
								{:else if f.verdict === 'pass'}
									<svg viewBox="0 0 14 14" width="11" height="11" aria-hidden="true"><path d="M2.5 7.3l2.8 2.8 6-6.3" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" /></svg>
								{:else if f.verdict === 'advisory'}
									<svg viewBox="0 0 14 14" width="11" height="11" aria-hidden="true"><path d="M7 1.6l5.6 10H1.4z" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round" /></svg>
								{:else}
									<svg viewBox="0 0 14 14" width="11" height="11" aria-hidden="true"><circle cx="7" cy="7" r="5.5" fill="none" stroke="currentColor" stroke-width="1.3" stroke-dasharray="2 2" /></svg>
								{/if}
								{VERDICT[f.verdict]}
							</td>
							<td class="aq-small says">{f.message}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		{:else}
			<p class="aq-muted aq-small">Shown once the setup is checked by the control service: it needs the channels and the reference.</p>
		{/if}
	</section>

	<ChannelTable {acq} />
</div>

<style>
	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
	}

	.verdict {
		white-space: nowrap;
	}

	.verdict svg {
		vertical-align: -1px;
		margin-right: 4px;
	}

	.verdict.exceeded {
		font-weight: var(--weight-strong);
		color: var(--amber-1);
	}

	.says {
		color: var(--text-2);
	}
</style>
