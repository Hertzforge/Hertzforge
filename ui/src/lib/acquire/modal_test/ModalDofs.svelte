<script lang="ts">
	/**
	 * Each input row's DOF and its part in the test: the node it sits at, its direction, and whether
	 * it is a reference (the hammer, or a shaker's force), a response, or only monitored. These are
	 * the channel table's own columns, edited here through the table's edit and sent with the table:
	 * nothing of them is kept by this screen. Roving rows take each point's node in turn; they are
	 * chosen in Roving. A drive row has no DOF.
	 */
	import Findings from '../Findings.svelte';
	import type { ChannelRow } from '$lib/protocol/types';
	import type { ModalTestStore } from './modalTestStore.svelte';

	interface Props {
		modal: ModalTestStore;
	}
	let { modal }: Props = $props();

	const locked = $derived(modal.locked);
	const rows = $derived(modal.rows);
	const inputs = $derived(rows.map((r, position) => ({ r, position })).filter(({ r }) => r.role !== 'drive'));
	const findings = $derived([...(modal.fieldErrors['dofs'] ?? []).map((message) => ({ level: 'error' as const, message })), ...(modal.fieldWarnings['dofs'] ?? []).map((message) => ({ level: 'warning' as const, message }))]);
	const DIRECTIONS = ['X+', 'X-', 'Y+', 'Y-', 'Z+', 'Z-'];
	const ROLES: { id: NonNullable<ChannelRow['analysis_role']>; label: string }[] = [
		{ id: 'reference', label: 'Reference' },
		{ id: 'response', label: 'Response' },
		{ id: 'monitor', label: 'Monitor' },
		{ id: '', label: 'Not used' }
	];
	const roving = $derived(new Set(modal.setup.roving.on ? modal.setup.roving.channels : []));
</script>

<section class="aq-panel" aria-labelledby="mt-dofs" data-testid="mt-dofs">
	<div class="aq-head"><h2 id="mt-dofs">DOFs</h2><span class="aq-sub">The channel table's node, direction and role of each input row.</span></div>
	<p class="aq-small lead">
		{modal.setup.mode === 'impact' ? 'Make the hammer row the one reference, and each accelerometer a response.' : 'Make the force row of each shaker a reference, and each accelerometer a response.'}
		The FRFs are labeled by these DOFs, as the modal fit reads them. They are kept in the channel table, which is applied with the setup.
	</p>
	{#if inputs.length}
		<table class="aq-table">
			<thead><tr><th class="num">Row</th><th>Description</th><th>Unit</th><th>Role</th><th class="num">Node</th><th>Direction</th><th>Roves</th></tr></thead>
			<tbody>
				{#each inputs as { r, position } (r.index)}
					<tr data-testid="mt-dof-row" data-row={r.index}>
						<td class="num">{r.index}</td>
						<td>{r.description}</td>
						<td>{r.units}</td>
						<td>
							<select class="field" aria-label="Row {r.index} role" disabled={locked || !r.enabled} value={r.analysis_role ?? ''} onchange={(e) => modal.editDof(position, { field: 'analysis_role', value: e.currentTarget.value as NonNullable<ChannelRow['analysis_role']> })} data-testid="mt-dof-role">
								{#each ROLES as o (o.id)}<option value={o.id}>{o.label}</option>{/each}
							</select>
						</td>
						<td class="num">
							<input class="field num" type="number" min="0" step="1" aria-label="Row {r.index} node" disabled={locked || !r.enabled} value={r.node ?? ''} onchange={(e) => modal.editDof(position, { field: 'node', value: e.currentTarget.value === '' ? null : Math.round(Number(e.currentTarget.value)) })} data-testid="mt-dof-node" />
						</td>
						<td>
							<select class="field" aria-label="Row {r.index} direction" disabled={locked || !r.enabled} value={r.direction ?? ''} onchange={(e) => modal.editDof(position, { field: 'direction', value: e.currentTarget.value })} data-testid="mt-dof-direction">
								<option value="">None</option>
								{#each DIRECTIONS as d (d)}<option value={d}>{d}</option>{/each}
							</select>
						</td>
						<td>{roving.has(r.index) ? 'Yes' : ''}</td>
					</tr>
				{/each}
			</tbody>
		</table>
	{:else}
		<p class="aq-small aq-muted">No input row yet: add the hammer and the accelerometers in Station &amp; Channels.</p>
	{/if}
	{#if modal.acq.tableDirty}<p class="aq-small warn" data-testid="mt-dofs-dirty">The table has edits the service has not checked yet: Apply Setup sends the table first, then the setup.</p>{/if}
	<Findings items={findings} testid="mt-dofs-findings" />
</section>

<style>
	.lead {
		margin: 0 0 var(--sp-2);
	}

	input.field {
		width: 5em;
	}

	.warn {
		color: var(--status-warn);
		margin: var(--sp-2) 0 0;
	}
</style>
