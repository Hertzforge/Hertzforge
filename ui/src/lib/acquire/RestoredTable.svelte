<script lang="ts">
	/**
	 * A restored channel table: the one the control service kept across its restart, or one taken
	 * from the project or a file. It is said in words, with where it came from and when, and with
	 * a marker that is a shape and a label, never a color alone. No run arms on it until the
	 * operator reviews every row (channel, role, unit, sensitivity, range, polarity, abort limits)
	 * and applies it. Shown in the channel table and beside every test's Arm.
	 */
	import type { AcquireState } from './store.svelte';
	import { applyRows, restoredWords } from './tableHome';

	interface Props {
		acq: AcquireState;
		/** Beside Arm: one line and the Review and Apply button. */
		compact?: boolean;
	}
	let { acq, compact = false }: Props = $props();

	const restored = $derived(acq.restored);
	const rows = $derived(acq.server ? applyRows(acq.server.channels) : []);
	const applyWhy = $derived(
		!acq.connected ? 'The control service is not connected.' : !acq.isController ? 'Take control to apply the table.' : acq.locked ? 'The table is locked while a run is live.' : null
	);
	let reviewing = $state(false);

	async function apply() {
		if (await acq.applyRestored()) reviewing = false;
	}
</script>

{#if restored}
	<div class="aq-notice warn restored" class:compact role="status" data-testid={compact ? 'arm-table-restored' : 'table-restored'} data-source={restored.source}>
		<div class="grow">
			<p class="line">
				<span class="mark" aria-hidden="true">
					<svg viewBox="0 0 12 12" width="12" height="12"><path d="M6 1l5 9.5H1z" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round" /><path d="M6 4.6v2.8M6 8.9v.1" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" /></svg>
				</span>
				<strong data-testid="restored-label">Channel Table Not Applied</strong>
				<span data-testid="restored-words">{restoredWords(restored)}</span>
			</p>
			{#if reviewing}
				<div class="aq-scroll review">
					<table class="aq-table" data-testid="restored-review">
						<caption class="aq-small">Revision {acq.server?.revision}: every row a run would arm with.</caption>
						<thead>
							<tr><th>Channel</th><th>On</th><th>Description</th><th>Role</th><th>Unit</th><th>Sensitivity</th><th>Range</th><th>Polarity</th><th>Abort Limits</th></tr>
						</thead>
						<tbody>
							{#each rows as r (r.channel)}
								<tr data-testid="restored-row">
									<td class="num">{r.channel}</td>
									<td>{r.enabled ? 'On' : 'Off'}</td>
									<td>{r.description}</td>
									<td>{r.role}</td>
									<td>{r.unit}</td>
									<td class="num">{r.sensitivity}</td>
									<td class="num">{r.range}</td>
									<td>{r.polarity}</td>
									<td>{r.abort}</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
				{#if applyWhy}<p class="aq-small" data-testid="restored-apply-why">{applyWhy}</p>{/if}
			{/if}
		</div>
		<div class="aq-actions">
			{#if reviewing}
				<button class="btn primary sm" data-testid="restored-apply" disabled={!!applyWhy || acq.busy !== null} title={applyWhy ?? 'Apply this table: runs may then arm on it.'} onclick={apply}>Apply This Table</button>
				<button class="btn ghost sm" data-testid="restored-cancel" onclick={() => (reviewing = false)}>Cancel</button>
			{:else}
				<button class="btn sm" data-testid="restored-review-open" onclick={() => (reviewing = true)}>Review and Apply</button>
			{/if}
		</div>
	</div>
{/if}
{#if !compact && acq.keptProblem}
	<div class="aq-notice warn" role="status" data-testid="table-kept-problem">
		<p class="line"><strong>Kept Table:</strong> {acq.keptProblem}</p>
	</div>
{/if}

<style>
	.restored {
		align-items: flex-start;
	}
	.line {
		margin: 0;
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
		align-items: baseline;
	}
	.mark {
		display: inline-flex;
		align-self: center;
	}
	.review {
		margin-top: 8px;
		max-height: 260px;
	}
	.compact {
		margin: 4px 0 0;
	}
</style>
