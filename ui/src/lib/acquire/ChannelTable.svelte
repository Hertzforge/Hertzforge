<script lang="ts">
	/**
	 * The channel table, one service-held table for every run, edited as a draft. It is sized from
	 * the selected station: a row for every channel, off until switched on, and rows set before kept
	 * through a rescan; a row on a channel the station no longer has stays, marked, until removed.
	 * Rows are selected by their number, and an edit to one selected row reaches every selected
	 * row. The service's dry run checks every edit; its findings sit on the row and the cell they
	 * name. IEPE is off on every row until the operator turns it on for that row. Apply sends the
	 * draft with the revision it started from; if the service's table moved meanwhile, the change
	 * is shown and nothing is sent until the operator chooses. The File menu saves the table, and
	 * opens one in place of the draft.
	 */
	import type { ChannelRole } from '$lib/protocol/types';
	import { confirmFirst } from '$lib/project/files';
	import { NO_SELECTION, isSelected, pick, pickHow, reach, step, within, type RowSelection } from '$lib/tables/selection';
	import TableFileMenu from '$lib/tables/TableFileMenu.svelte';
	import { pathName, type FileResult } from '$lib/tables/tableFiles';
	import Findings from './Findings.svelte';
	import NumberField from './NumberField.svelte';
	import type { AcquireState } from './store.svelte';
	import { STATUS_WORDS, freeChannels, physicalKey, rolesFor, type RowCell, type RowEdit, type RowFinding } from './table';

	interface Props {
		acq: AcquireState;
	}
	let { acq }: Props = $props();

	const ROLE_WORDS: Record<ChannelRole, string> = { control: 'Control', monitor: 'Monitor', reference: 'Reference', drive: 'Drive' };
	const UNITS = ['g', 'in/s', 'mil', 'lbf', 'V'];
	const POLARITIES = ['positive', 'negative'] as const;

	const locked = $derived(acq.locked);
	const onNi = $derived(acq.selected?.kind === 'ni');
	const sized = $derived(acq.sized);
	const findings = $derived(acq.findings);
	const applyWhy = $derived(
		!acq.connected ? 'The control service is not connected.' : !acq.isController ? 'Take control to apply the table.' : locked ? 'The table is locked while a run is live.' : acq.tableChange ? 'The service has a newer table; choose what to keep first.' : null
	);
	const unbound = $derived(onNi && sized.some((s) => s.status === 'unbound' && s.row.enabled));
	const onCount = $derived(sized.filter((s) => s.row.enabled).length);
	const missingCount = $derived(sized.filter((s) => s.status === 'missing').length);

	let open = $state<Record<number, boolean>>({});
	let sel = $state<RowSelection>(NO_SELECTION);
	let skippedNote = $state<string | null>(null);
	let fileResults = $state<FileResult[]>([]);
	let numberButtons = $state<HTMLButtonElement[]>([]);

	// rows taken away or a table opened in place: the selection keeps only rows still there
	$effect(() => {
		const count = sized.length;
		if (sel.rows.some((r) => r >= count) || (sel.focus ?? -1) >= count) sel = within(sel, count);
	});

	function cellFindings(index: number, cell: RowCell): RowFinding[] {
		return (findings.rows.get(index) ?? []).filter((f) => f.cell === cell);
	}
	const errs = (index: number, cell: RowCell) => cellFindings(index, cell).filter((f) => f.level === 'error').map((f) => f.message);
	/** A cell that holds no number says so before anything is sent. */
	const need = (v: number) => (Number.isFinite(v) ? [] : ['Enter a number.']);
	const blanks = $derived(acq.rows.filter((r) => ![r.index, r.sensitivity_mv_per_eu, r.range_max].every(Number.isFinite)).length);
	const warns = (index: number, cell: RowCell) => cellFindings(index, cell).filter((f) => f.level === 'warning').map((f) => f.message);
	const marked = (index: number, cell: RowCell) => cellFindings(index, cell).some((f) => f.level === 'error');

	/** An edit at row `at`: to every selected row when `at` is one of two or more selected, else to `at`. */
	function edit(at: number, e: RowEdit) {
		const rows = reach(sel, at);
		const { skipped } = acq.editRows(rows, e);
		skippedNote = skipped ? `${skipped} of ${rows.length} rows ${skipped === 1 ? 'does' : 'do'} not take that: an output has no IEPE or abort limits, and an input is never a drive.` : null;
	}

	function setAll(on: boolean) {
		if (sel.rows.length) acq.editRows(sel.rows, { field: 'enabled', value: on });
	}

	function clearSelection() {
		sel = NO_SELECTION;
		skippedNote = null;
	}

	function onNumberClick(e: MouseEvent, at: number) {
		sel = pick(sel, at, pickHow(e));
		skippedNote = null;
	}

	function onNumberKey(e: KeyboardEvent, at: number) {
		if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
		e.preventDefault();
		sel = step({ ...sel, focus: sel.focus ?? at }, e.key === 'ArrowDown' ? 1 : -1, e.shiftKey, sized.length);
		if (sel.focus !== null) numberButtons[sel.focus]?.focus();
	}

	function onTableKey(e: KeyboardEvent) {
		if (e.key === 'Escape' && sel.rows.length) {
			e.preventDefault();
			clearSelection();
		}
	}

	function remove(at: number) {
		acq.removeRow(at);
		sel = NO_SELECTION;
	}

	async function confirmOpen(path: string): Promise<boolean> {
		if (!acq.tableDirty) return true;
		return confirmFirst(`The channel table has edits that are not applied. Opening ${pathName(path)} replaces them. Open it anyway?`, 'Open');
	}

	const rowsWord = (n: number) => `${n} ${n === 1 ? 'row' : 'rows'}`;
</script>

<section class="aq-panel channels" aria-labelledby="channels-title" data-testid="channel-panel">
	<div class="aq-head">
		<h2 id="channels-title">Channels</h2>
		<span class="aq-sub num">
			Revision {acq.base} on the service{#if acq.tableDirty}<span class="edited" data-testid="table-edited">Edited here</span>{/if}
		</span>
		<span class="aq-spacer"></span>
		<div class="aq-actions">
			<TableFileMenu
				client={acq.tableFiles}
				kind="channels"
				name="Channel Table"
				table={() => acq.tableData()}
				bind:path={acq.tableFile}
				openBlocked={locked ? 'The table is locked while a run is live.' : null}
				{confirmOpen}
				onopen={(loaded, path) => {
					acq.openTable(loaded, path);
					sel = NO_SELECTION;
				}}
				onresult={(items) => (fileResults = items)}
				testid="channel-file"
			/>
			{#if unbound}
				<button class="btn sm" data-testid="bind-rows" disabled={locked} onclick={() => acq.bindRows()} title="Bind each enabled, unbound row to the next free channel of its direction, in table order.">Bind Unbound Rows</button>
			{/if}
			{#if acq.tableDirty}
				<button class="btn ghost sm" data-testid="table-discard" onclick={() => acq.discardTable()} disabled={locked}>Discard Edits</button>
				<button class="btn primary sm" data-testid="table-apply" disabled={!!applyWhy || acq.busy !== null || acq.tableErrorCount > 0 || blanks > 0} onclick={() => acq.applyTable()} title={applyWhy ?? (acq.tableErrorCount ? 'Fix the errors in the table first.' : 'Write the table to the service.')}>Apply Table</button>
			{/if}
		</div>
	</div>

	{#if fileResults.length}
		<div class="fileresults" data-testid="channel-file-results">
			<Findings items={fileResults} />
			<button class="aq-icon-btn" aria-label="Dismiss" onclick={() => (fileResults = [])}>
				<svg viewBox="0 0 12 12" width="10" height="10" aria-hidden="true"><path d="M3 3l6 6M9 3l-6 6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
			</button>
		</div>
	{/if}

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

	<Findings items={findings.table} testid="table-findings" />

	{#if sel.rows.length >= 2}
		<div class="strip" role="status" data-testid="channel-selection">
			<span><strong class="num">{sel.rows.length}</strong> rows selected. An edit to one of them reaches all {sel.rows.length}.</span>
			{#if skippedNote}<span class="skipped">{skippedNote}</span>{/if}
			<span class="aq-spacer"></span>
			<button class="btn sm" disabled={locked} data-testid="channel-selection-on" onclick={() => setAll(true)}>On</button>
			<button class="btn sm" disabled={locked} data-testid="channel-selection-off" onclick={() => setAll(false)}>Off</button>
			<button class="btn ghost sm" data-testid="channel-selection-clear" onclick={clearSelection}>Clear Selection</button>
		</div>
	{/if}

	<datalist id="aq-units">
		{#each UNITS as u (u)}<option value={u}></option>{/each}
	</datalist>

	<!-- Escape anywhere in the table clears the selection -->
	<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
	<div class="aq-scroll" role="group" aria-label="Channel table" onkeydown={onTableKey}>
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
				{#each sized as s, i (i)}
					{@const row = s.row}
					{@const rf = s.virtual ? [] : (findings.rows.get(row.index) ?? [])}
					{@const nErr = rf.filter((f) => f.level === 'error').length}
					{@const drive = row.role === 'drive'}
					{@const picked = isSelected(sel, i)}
					{@const roles = rolesFor(s.direction)}
					<tr class:disabled={!row.enabled} class:picked data-testid="channel-row-{i}" data-status={s.status}>
						<td class="c-idx">
							<button
								bind:this={numberButtons[i]}
								class="pick num"
								class:on={picked}
								aria-pressed={picked}
								aria-label="Select channel {row.index}"
								title="Select. Shift selects a run; Cmd or Ctrl adds or takes away one."
								data-testid="channel-pick-{i}"
								onclick={(e) => onNumberClick(e, i)}
								onkeydown={(e) => onNumberKey(e, i)}
							>
								{#if picked}<svg viewBox="0 0 12 12" width="9" height="9" aria-hidden="true"><path d="M2.5 6.2l2.4 2.4 4.6-5" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" /></svg>{/if}
								{Number.isFinite(row.index) ? row.index : '?'}
							</button>
						</td>
						<td class="c-on">
							<label class="aq-switch">
								<input type="checkbox" role="switch" aria-checked={row.enabled} data-testid="channel-on-{i}" aria-label="Channel {row.index} on" checked={row.enabled} disabled={locked} onchange={(e) => edit(i, { field: 'enabled', value: e.currentTarget.checked })} />
								<span class="track" aria-hidden="true"><span class="thumb"></span></span>
								<span class="word">{row.enabled ? 'On' : 'Off'}</span>
							</label>
						</td>
						<td>
							<input class="field desc" aria-label="Channel {row.index} description" value={row.description} disabled={locked} oninput={(e) => edit(i, { field: 'description', value: e.currentTarget.value })} />
						</td>
						<td>
							<select class="field role" aria-label="Channel {row.index} role" data-testid="channel-role-{i}" value={row.role} disabled={locked || roles.length < 2} aria-invalid={marked(row.index, 'role')} onchange={(e) => edit(i, { field: 'role', value: e.currentTarget.value as ChannelRole })}>
								{#each roles as r (r)}<option value={r}>{ROLE_WORDS[r]}</option>{/each}
								{#if !roles.includes(row.role)}<option value={row.role}>{ROLE_WORDS[row.role]}</option>{/if}
							</select>
						</td>
						<td>
							<div class="phys-cell">
								{#if !onNi}
									<span class="aq-muted aq-small mono">{physicalKey(row) ?? 'Simulator'}</span>
								{:else if s.status === 'present'}
									<span class="mono phys-key" data-testid="channel-physical-{i}">{physicalKey(row)}</span>
								{:else}
									{@const free = freeChannels(sized, acq.hardware, i)}
									<select class="field phys mono" aria-label="Channel {row.index} physical channel" data-testid="channel-physical-{i}" value={physicalKey(row) ?? ''} disabled={locked} aria-invalid={marked(row.index, 'physical')} onchange={(e) => e.currentTarget.value && e.currentTarget.value !== physicalKey(row) && acq.moveRow(i, e.currentTarget.value)}>
										{#if row.physical}<option value={physicalKey(row)}>{physicalKey(row)}</option>{:else}<option value="">No channel</option>{/if}
										{#each free as c (c.key)}<option value={c.key}>Move to {c.key}</option>{/each}
									</select>
									<span class="status" data-testid="channel-status-{i}">
										{#if s.status === 'unbound'}
											<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><circle cx="7" cy="7" r="5.6" fill="none" stroke="currentColor" stroke-width="1.4" stroke-dasharray="2.4 1.8" /></svg>
										{:else if s.status === 'missing'}
											<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><circle cx="7" cy="7" r="5.6" fill="none" stroke="currentColor" stroke-width="1.4" stroke-dasharray="2.4 1.8" /><path d="M4.6 4.6l4.8 4.8" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" /></svg>
										{:else}
											<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><rect x="1.8" y="3.4" width="7" height="7" rx="1.2" fill="none" stroke="currentColor" stroke-width="1.3" /><rect x="5.2" y="1.8" width="7" height="7" rx="1.2" fill="none" stroke="currentColor" stroke-width="1.3" /></svg>
										{/if}
										{STATUS_WORDS[s.status]}
									</span>
								{/if}
							</div>
						</td>
						<td>
							<label class="aq-switch" title={drive ? 'An output has no IEPE.' : 'IEPE drives a constant current into the input. Leave it off for an input wired to a voltage source.'}>
								<input type="checkbox" role="switch" aria-checked={row.interface === 'iepe'} data-testid="channel-iepe-{i}" aria-label="Channel {row.index} IEPE" checked={row.interface === 'iepe'} disabled={locked || drive} onchange={(e) => edit(i, { field: 'iepe', value: e.currentTarget.checked })} />
								<span class="track" aria-hidden="true"><span class="thumb"></span></span>
								<span class="word">{row.interface === 'iepe' ? 'On' : 'Off'}</span>
							</label>
						</td>
						<td>
							<input class="field units" list="aq-units" aria-label="Channel {row.index} units" value={row.units} disabled={locked} oninput={(e) => edit(i, { field: 'units', value: e.currentTarget.value })} />
						</td>
						<td class="num">
							<NumberField id="ch-{i}-sens" label="Channel {row.index} sensitivity" compact unit="mV/{row.units}" value={row.sensitivity_mv_per_eu} onchange={(v) => edit(i, { field: 'sensitivity_mv_per_eu', value: v ?? NaN })} errors={[...need(row.sensitivity_mv_per_eu), ...errs(row.index, 'sensitivity_mv_per_eu')]} disabled={locked} />
						</td>
						<td class="num">
							<NumberField id="ch-{i}-range" label="Channel {row.index} range" compact unit={row.units} value={row.range_max} onchange={(v) => edit(i, { field: 'range_max', value: v ?? NaN })} errors={[...need(row.range_max), ...errs(row.index, 'range_max')]} disabled={locked} />
						</td>
						<td class="c-end">
							<div class="endcell">
								{#if nErr}
									<span class="count err" title="{nErr} {nErr === 1 ? 'error' : 'errors'}">
										<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><path d="M4.5 1h5L13 4.5v5L9.5 13h-5L1 9.5v-5z" fill="currentColor" /><path d="M5 5l4 4M9 5l-4 4" stroke="var(--status-fail-text)" stroke-width="1.7" stroke-linecap="round" /></svg>
										<span class="num">{nErr}</span><span class="visually-hidden">errors</span>
									</span>
								{/if}
								<button class="aq-icon-btn" aria-expanded={!!open[i]} aria-label="More for channel {row.index}: number, polarity, control group, IEPE bias, abort limits" data-testid="channel-more-{i}" onclick={() => (open[i] = !open[i])}>
									<svg viewBox="0 0 12 12" width="12" height="12" aria-hidden="true" style:transform={open[i] ? 'rotate(90deg)' : ''}><path d="M4.5 2.5L8 6l-3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
								</button>
								{#if !s.virtual && (s.status !== 'present' || !onNi)}
									<button class="aq-icon-btn" aria-label="Remove channel {row.index}" data-testid="channel-remove-{i}" disabled={locked} onclick={() => remove(i)}>
										<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M3 3l6 6M9 3l-6 6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
									</button>
								{/if}
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
									<NumberField id="ch-{i}-index" label="Channel number" integer value={row.index} onchange={(v) => edit(i, { field: 'index', value: v ?? NaN })} errors={[...need(row.index), ...errs(row.index, 'index')]} disabled={locked} />
									<div class="pol">
										<span class="lbl">Polarity</span>
										<div class="aq-seg sm" role="radiogroup" aria-label="Channel {row.index} polarity">
											{#each POLARITIES as p (p)}
												<button type="button" role="radio" aria-checked={row.polarity === p} disabled={locked} onclick={() => edit(i, { field: 'polarity', value: p })}>{p === 'positive' ? 'Positive' : 'Negative'}</button>
											{/each}
										</div>
									</div>
									<NumberField id="ch-{i}-group" label="Control group" integer nullable="None" fallback={0} value={row.control_group} onchange={(v) => edit(i, { field: 'control_group', value: v })} errors={errs(row.index, 'control_group')} disabled={locked} />
									<NumberField id="ch-{i}-bias" label="IEPE bias" unit="V" nullable="None" fallback={24} value={row.iepe_bias_v} onchange={(v) => edit(i, { field: 'iepe_bias_v', value: v })} errors={errs(row.index, 'iepe_bias_v')} warnings={[...warns(row.index, 'iepe_bias_v'), ...warns(row.index, 'interface')]} disabled={locked || drive} />
									<NumberField id="ch-{i}-ab-g" label="Abort, acceleration" unit="g" nullable="None" fallback={50} value={row.abort.accel_g} onchange={(v) => edit(i, { field: 'abort_accel_g', value: v })} disabled={locked || drive} help="Peak; the run aborts at any level." />
									<NumberField id="ch-{i}-ab-v" label="Abort, velocity" unit="in/s" nullable="None" fallback={50} value={row.abort.velocity_ips} onchange={(v) => edit(i, { field: 'abort_velocity_ips', value: v })} disabled={locked || drive} />
									<NumberField id="ch-{i}-ab-d" label="Abort, displacement" unit="mil" nullable="None" fallback={500} value={row.abort.displacement_mil} onchange={(v) => edit(i, { field: 'abort_displacement_mil', value: v })} disabled={locked || drive} />
								</div>
							</td>
						</tr>
					{/if}
				{:else}
					<tr><td colspan="10" class="aq-muted">No channels yet. The table fills from the station once the service lists it.</td></tr>
				{/each}
			</tbody>
		</table>
	</div>
	{#if sized.length}
		<p class="aq-muted aq-small foot num" data-testid="channel-count">
			{rowsWord(onCount)} on of {sized.length}{#if missingCount}; {rowsWord(missingCount)} on hardware the station does not have{/if}
		</p>
	{/if}
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

	.fileresults {
		display: flex;
		align-items: flex-start;
		gap: var(--sp-2);
		margin-bottom: var(--sp-2);
	}

	.fileresults :global(.aq-findings) {
		flex: 1;
	}

	.strip {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2);
		margin-bottom: var(--sp-2);
		padding: 4px 8px;
		font-size: var(--text-xs);
		background: var(--accent-weak);
		border: 1px solid var(--line-2);
		border-radius: var(--radius-md);
	}

	.strip .skipped {
		color: var(--text-2);
	}

	.c-idx {
		width: 52px;
	}

	.pick {
		display: inline-flex;
		align-items: center;
		justify-content: flex-end;
		gap: 3px;
		min-width: 40px;
		padding: 2px 6px;
		color: var(--text-2);
		font: inherit;
		font-size: var(--text-xs);
		background: none;
		border: 1px solid transparent;
		border-radius: 4px;
		cursor: pointer;
	}

	.pick:hover {
		border-color: var(--line-3);
	}

	.pick:focus-visible {
		outline: 2px solid var(--focus);
		outline-offset: 1px;
	}

	.pick.on {
		color: var(--text-1);
		font-weight: var(--weight-strong);
		background: var(--bg-4);
		border-color: var(--accent);
	}

	tr.picked td {
		background: var(--accent-weak);
	}

	.c-on {
		width: 70px;
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

	.phys-cell {
		display: flex;
		align-items: center;
		gap: 6px;
	}

	.phys-key {
		font-size: var(--text-xs);
	}

	.status {
		display: inline-flex;
		align-items: center;
		gap: 3px;
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		white-space: nowrap;
		color: var(--amber-1);
	}

	.units {
		width: 68px;
	}

	td.num :global(.nf) {
		min-width: 118px;
	}

	tr.disabled td:not(.c-on):not(.c-end):not(.c-idx) {
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

	.mono {
		font-family: var(--font-mono);
	}

	.foot {
		margin: var(--sp-2) 0 0;
	}
</style>
