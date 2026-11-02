<script lang="ts">
	/**
	 * The channel table, one service-held table for every run, edited as a draft. The service's
	 * dry run checks every edit; its findings sit on the row and the cell they name. IEPE is off
	 * on every row until the operator turns it on for that row. Apply sends the draft with the
	 * revision it started from; if the service's table moved meanwhile, the change is shown and
	 * nothing is sent until the operator chooses.
	 */
	import type { ChannelRole, ChannelRow } from '$lib/protocol/types';
	import Findings from './Findings.svelte';
	import NumberField from './NumberField.svelte';
	import type { AcquireState } from './store.svelte';
	import { channelGroups, physicalKey, type RowCell, type RowFinding } from './table';

	interface Props {
		acq: AcquireState;
	}
	let { acq }: Props = $props();

	const ROLES: { value: ChannelRole; label: string }[] = [
		{ value: 'control', label: 'Control' },
		{ value: 'monitor', label: 'Monitor' },
		{ value: 'reference', label: 'Reference' },
		{ value: 'drive', label: 'Drive' }
	];
	const UNITS = ['g', 'in/s', 'mil', 'lbf', 'V'];

	const locked = $derived(acq.locked);
	const onNi = $derived(acq.selected?.kind === 'ni');
	const inputs = $derived(onNi && acq.selected ? channelGroups(acq.niDevices, acq.selected.devices, 'input') : []);
	const outputs = $derived(onNi && acq.selected ? channelGroups(acq.niDevices, acq.selected.devices, 'output') : []);
	const findings = $derived(acq.findings);
	const applyWhy = $derived(
		!acq.connected ? 'The control service is not connected.' : !acq.isController ? 'Take control to apply the table.' : locked ? 'The table is locked while a run is live.' : acq.tableChange ? 'The service has a newer table; choose what to keep first.' : null
	);

	let open = $state<Record<number, boolean>>({});

	function cellFindings(index: number, cell: RowCell): RowFinding[] {
		return (findings.rows.get(index) ?? []).filter((f) => f.cell === cell);
	}
	const errs = (index: number, cell: RowCell) => cellFindings(index, cell).filter((f) => f.level === 'error').map((f) => f.message);
	/** A cell that holds no number says so before anything is sent. */
	const need = (v: number) => (Number.isFinite(v) ? [] : ['Enter a number.']);
	const blanks = $derived(acq.rows.filter((r) => ![r.index, r.sensitivity_mv_per_eu, r.range_max].every(Number.isFinite)).length);
	const warns = (index: number, cell: RowCell) => cellFindings(index, cell).filter((f) => f.level === 'warning').map((f) => f.message);
	const marked = (index: number, cell: RowCell) => cellFindings(index, cell).some((f) => f.level === 'error');

	function setPhysical(row: ChannelRow, key: string) {
		if (!key) row.physical = null;
		else {
			const [device, channel] = key.split('/');
			row.physical = { device, channel };
		}
	}

	function setRole(row: ChannelRow, role: ChannelRole) {
		const wasDrive = row.role === 'drive';
		row.role = role;
		// An output carries no IEPE; a row that changes direction loses its binding.
		if (role === 'drive') {
			row.interface = 'single_ended';
			row.iepe_bias_v = null;
		}
		if (wasDrive !== (role === 'drive')) row.physical = null;
	}

	function setIepe(row: ChannelRow, on: boolean) {
		row.interface = on ? 'iepe' : 'single_ended';
		if (!on) row.iepe_bias_v = null;
	}
</script>

<section class="aq-panel channels" aria-labelledby="channels-title" data-testid="channel-panel">
	<div class="aq-head">
		<h2 id="channels-title">Channels</h2>
		<span class="aq-sub num">
			Revision {acq.base} on the service{#if acq.tableDirty}<span class="edited" data-testid="table-edited">Edited here</span>{/if}
		</span>
		<span class="aq-spacer"></span>
		<div class="aq-actions">
			<button class="btn sm" data-testid="add-input" disabled={locked} onclick={() => acq.addRow('monitor')}>Add Input</button>
			<button class="btn sm" data-testid="add-drive" disabled={locked} onclick={() => acq.addRow('drive')}>Add Drive</button>
			{#if onNi}
				<button class="btn sm" data-testid="bind-rows" disabled={locked} onclick={() => acq.bindRows()} title="Bind each enabled, unbound row to the next free channel of its direction, in table order.">Bind Unbound Rows</button>
			{/if}
			{#if acq.tableDirty}
				<button class="btn ghost sm" data-testid="table-discard" onclick={() => acq.discardTable()} disabled={locked}>Discard Edits</button>
				<button class="btn primary sm" data-testid="table-apply" disabled={!!applyWhy || acq.busy !== null || acq.tableErrorCount > 0 || blanks > 0} onclick={() => acq.applyTable()} title={applyWhy ?? (acq.tableErrorCount ? 'Fix the errors in the table first.' : 'Write the table to the service.')}>Apply Table</button>
			{/if}
		</div>
	</div>

	{#if acq.tableChange}
		{@const ch = acq.tableChange}
		<div class="aq-notice warn change" role="alert" data-testid="table-change">
			<div class="grow">
				<strong>The service's table changed while this screen had edits (now revision {ch.revision}).</strong>
				<ul>
					{#each ch.changes as line (line)}<li>{line}</li>{/each}
					{#if ch.changes.length === 0}<li>No row differs from the revision these edits started from.</li>{/if}
				</ul>
				<p class="aq-small">The table carries the abort limits, so nothing is merged or sent on its own.</p>
			</div>
			<div class="aq-actions">
				<button class="btn sm" data-testid="table-take" onclick={() => acq.takeServerTable()}>Load the Service's Table</button>
				<button class="btn sm" data-testid="table-keep" onclick={() => acq.keepEditsOnServer()}>Keep My Edits on Revision {ch.revision}</button>
			</div>
		</div>
	{/if}

	{#if !onNi}
		<p class="aq-muted aq-small simnote">On the simulator, physical bindings are kept but not used. On an NI station, every enabled row is bound to a channel of the selected devices.</p>
	{/if}

	<Findings items={findings.table} testid="table-findings" />

	<datalist id="aq-units">
		{#each UNITS as u (u)}<option value={u}></option>{/each}
	</datalist>

	<div class="aq-scroll">
		<table class="aq-table" data-testid="channel-table">
			<thead>
				<tr>
					<th class="c-idx">#</th>
					<th class="c-on">On</th>
					<th>Description</th>
					<th>Role</th>
					<th>Physical Channel</th>
					<th>IEPE</th>
					<th>Units</th>
					<th class="num">Sensitivity</th>
					<th class="num">Range</th>
					<th><span class="visually-hidden">More and status</span></th>
				</tr>
			</thead>
			<tbody>
				{#each acq.rows as row, i (row)}
					{@const rf = findings.rows.get(row.index) ?? []}
					{@const nErr = rf.filter((f) => f.level === 'error').length}
					{@const drive = row.role === 'drive'}
					<tr class:disabled={!row.enabled} data-testid="channel-row-{i}">
						<td class="c-idx">
							<NumberField id="ch-{i}-index" label="Channel number" compact integer value={row.index} onchange={(v) => (row.index = v ?? NaN)} errors={[...need(row.index), ...errs(row.index, 'index')]} disabled={locked} />
						</td>
						<td class="c-on">
							<input type="checkbox" aria-label="Channel {row.index} enabled" bind:checked={row.enabled} disabled={locked} />
						</td>
						<td>
							<input class="field desc" aria-label="Channel {row.index} description" bind:value={row.description} disabled={locked} />
						</td>
						<td>
							<select class="field role" aria-label="Channel {row.index} role" value={row.role} disabled={locked} aria-invalid={marked(row.index, 'role')} onchange={(e) => setRole(row, e.currentTarget.value as ChannelRole)}>
								{#each ROLES as r (r.value)}<option value={r.value}>{r.label}</option>{/each}
							</select>
						</td>
						<td>
							{#if onNi}
								{@const groups = drive ? outputs : inputs}
								<select
									class="field phys mono"
									aria-label="Channel {row.index} physical channel"
									data-testid="channel-physical-{i}"
									value={physicalKey(row) ?? ''}
									disabled={locked}
									aria-invalid={marked(row.index, 'physical')}
									onchange={(e) => setPhysical(row, e.currentTarget.value)}
								>
									<option value="">Not bound</option>
									{#if row.physical && !groups.some((g) => g.channels.some((c) => c.key === physicalKey(row)))}
										<option value={physicalKey(row)}>{physicalKey(row)} (not in the selected devices)</option>
									{/if}
									{#each groups as g (g.device.id)}
										<optgroup label={g.label}>
											{#each g.channels as c (c.key)}<option value={c.key}>{c.key}</option>{/each}
										</optgroup>
									{/each}
								</select>
							{:else}
								<span class="aq-muted aq-small mono">{physicalKey(row) ?? 'Simulator'}</span>
							{/if}
						</td>
						<td>
							<label class="aq-switch" title={drive ? 'An output has no IEPE.' : 'IEPE drives a constant current into the input. Leave it off for an input wired to a voltage source.'}>
								<input type="checkbox" role="switch" data-testid="channel-iepe-{i}" aria-label="Channel {row.index} IEPE" checked={row.interface === 'iepe'} disabled={locked || drive} onchange={(e) => setIepe(row, e.currentTarget.checked)} />
								<span class="track" aria-hidden="true"><span class="thumb"></span></span>
								<span class="word">{row.interface === 'iepe' ? 'On' : 'Off'}</span>
							</label>
						</td>
						<td>
							<input class="field units" list="aq-units" aria-label="Channel {row.index} units" bind:value={row.units} disabled={locked} />
						</td>
						<td class="num">
							<NumberField id="ch-{i}-sens" label="Channel {row.index} sensitivity" compact unit="mV/{row.units}" value={row.sensitivity_mv_per_eu} onchange={(v) => (row.sensitivity_mv_per_eu = v ?? NaN)} errors={[...need(row.sensitivity_mv_per_eu), ...errs(row.index, 'sensitivity_mv_per_eu')]} disabled={locked} />
						</td>
						<td class="num">
							<NumberField id="ch-{i}-range" label="Channel {row.index} range" compact unit={row.units} value={row.range_max} onchange={(v) => (row.range_max = v ?? NaN)} errors={[...need(row.range_max), ...errs(row.index, 'range_max')]} disabled={locked} />
						</td>
						<td class="c-end">
							<div class="endcell">
								{#if nErr}
									<span class="count err" title="{nErr} {nErr === 1 ? 'error' : 'errors'}">
										<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><path d="M4.5 1h5L13 4.5v5L9.5 13h-5L1 9.5v-5z" fill="currentColor" /><path d="M5 5l4 4M9 5l-4 4" stroke="var(--status-fail-text)" stroke-width="1.7" stroke-linecap="round" /></svg>
										<span class="num">{nErr}</span><span class="visually-hidden">errors</span>
									</span>
								{/if}
								<button class="aq-icon-btn" aria-expanded={!!open[i]} aria-label="More for channel {row.index}: polarity, control group, IEPE bias, abort limits" data-testid="channel-more-{i}" onclick={() => (open[i] = !open[i])}>
									<svg viewBox="0 0 12 12" width="12" height="12" aria-hidden="true" style:transform={open[i] ? 'rotate(90deg)' : ''}><path d="M4.5 2.5L8 6l-3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
								</button>
								<button class="aq-icon-btn" aria-label="Remove channel {row.index}" disabled={locked} onclick={() => acq.removeRow(i)}>
									<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M3 3l6 6M9 3l-6 6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
								</button>
							</div>
						</td>
					</tr>
					{#if rf.length}
						<tr class="aq-findrow"><td colspan="10"><Findings items={rf} testid="channel-findings-{i}" /></td></tr>
					{/if}
					{#if open[i]}
						<tr class="aq-detail">
							<td colspan="10">
								<div class="detail">
									<div class="pol">
										<span class="lbl">Polarity</span>
										<div class="aq-seg sm" role="radiogroup" aria-label="Channel {row.index} polarity">
											{#each ['positive', 'negative'] as p (p)}
												<button type="button" role="radio" aria-checked={row.polarity === p} disabled={locked} onclick={() => (row.polarity = p as ChannelRow['polarity'])}>{p === 'positive' ? 'Positive' : 'Negative'}</button>
											{/each}
										</div>
									</div>
									<NumberField id="ch-{i}-group" label="Control group" integer nullable="None" fallback={0} bind:value={row.control_group} errors={errs(row.index, 'control_group')} disabled={locked} />
									<NumberField id="ch-{i}-bias" label="IEPE bias" unit="V" nullable="None" fallback={24} bind:value={row.iepe_bias_v} errors={errs(row.index, 'iepe_bias_v')} warnings={[...warns(row.index, 'iepe_bias_v'), ...warns(row.index, 'interface')]} disabled={locked || drive} />
									<NumberField id="ch-{i}-ab-g" label="Abort, acceleration" unit="g" nullable="None" fallback={50} bind:value={row.abort.accel_g} disabled={locked || drive} help="Peak; the run aborts at any level." />
									<NumberField id="ch-{i}-ab-v" label="Abort, velocity" unit="in/s" nullable="None" fallback={50} bind:value={row.abort.velocity_ips} disabled={locked || drive} />
									<NumberField id="ch-{i}-ab-d" label="Abort, displacement" unit="mil" nullable="None" fallback={500} bind:value={row.abort.displacement_mil} disabled={locked || drive} />
								</div>
							</td>
						</tr>
					{/if}
				{:else}
					<tr><td colspan="10" class="aq-muted">The table is empty. Add a control input and a drive.</td></tr>
				{/each}
			</tbody>
		</table>
	</div>
</section>

<style>
	.edited {
		display: inline-block;
		margin-left: 8px;
		padding: 0 6px;
		font-family: var(--font-ui);
		font-size: var(--text-2xs);
		font-weight: var(--weight-strong);
		letter-spacing: 0.05em;
		text-transform: uppercase;
		color: var(--amber-1);
		border: 1px dashed var(--amber-2);
		border-radius: 3px;
	}

	.change {
		flex-wrap: wrap;
		margin-bottom: var(--sp-3);
	}

	.change ul {
		margin: 4px 0;
		padding-left: 18px;
		font-size: var(--text-xs);
	}

	.simnote {
		margin: -4px 0 var(--sp-2);
	}

	.c-idx {
		width: 64px;
	}

	.c-on {
		width: 38px;
		text-align: center;
	}

	.c-on input {
		margin: 0;
		accent-color: var(--blue-2);
	}

	.desc {
		min-width: 120px;
	}

	.role {
		min-width: 102px;
	}

	.phys {
		min-width: 150px;
	}

	.units {
		width: 68px;
	}

	td.num :global(.nf) {
		min-width: 118px;
	}

	tr.disabled td:not(.c-on):not(.c-end) {
		opacity: 0.55;
	}

	.c-end {
		width: 1%;
		white-space: nowrap;
	}

	.endcell {
		display: flex;
		align-items: center;
		justify-content: flex-end;
		gap: 2px;
	}

	.count {
		display: inline-flex;
		align-items: center;
		gap: 3px;
		margin-right: 4px;
		font-size: var(--text-xs);
	}

	.count.err {
		color: var(--amber-1);
	}

	.detail {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(170px, 1fr));
		gap: var(--sp-3) var(--sp-4);
		align-items: start;
	}

	.pol {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.lbl {
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}
</style>
