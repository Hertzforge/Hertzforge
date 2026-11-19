<script lang="ts">
	/**
	 * The test's channels: every row of the channel table, sized from the station, with its role in
	 * the test (drive, control, limit or monitor), its unit and sensitivity, and what that role
	 * needs. A drive has its voltage ceiling (rms and peak), an optional abort level and the shaker
	 * behind it; a control has its alarm and abort bands; a limit has the PSD ceiling the core holds
	 * it under. A row is off until switched on. Rows are selected by their number, and an edit to
	 * one selected row reaches every selected row. The rows are the channel table's: a role or a
	 * unit changed here is changed in the table, which the setup applies with it, and the File menu
	 * saves and opens it. Wiring to the hardware is under Backend.
	 */
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import type { ChannelRow } from '$lib/protocol/types';
	import { confirmFirst } from '$lib/project/files';
	import { NO_SELECTION, isSelected, pick, pickHow, reach, within, type RowSelection } from '$lib/tables/selection';
	import TableFileMenu from '$lib/tables/TableFileMenu.svelte';
	import { pathName, type FileResult } from '$lib/tables/tableFiles';
	import { rolesFor, type RowEdit } from '../table';
	import { assignRole, MIMO_ROLES, roleOf, type MimoRole } from './mimoSetup';
	import type { MimoState } from './mimoStore.svelte';

	interface Props {
		mimo: MimoState;
	}
	let { mimo }: Props = $props();

	const acq = $derived(mimo.acq);
	const d = $derived(mimo.setup);
	const locked = $derived(mimo.locked);
	const E = $derived(mimo.fieldErrors);
	const W = $derived(mimo.fieldWarnings);
	const t = $derived(mimo.channels);
	const sized = $derived(acq.sized);
	const UNITS = ['g', 'm/s²', 'in/s', 'mil', 'lbf', 'N', 'V'];

	let open = $state<Record<number, boolean>>({});
	let fileInput = $state<HTMLInputElement | null>(null);
	let loadingFor = $state<number | null>(null);
	let sel = $state<RowSelection>(NO_SELECTION);
	let skippedNote = $state<string | null>(null);
	let fileResults = $state<FileResult[]>([]);

	$effect(() => {
		const count = sized.length;
		if (sel.rows.some((r) => r >= count)) sel = within(sel, count);
	});

	const at = (k: string) => E[k] ?? [];
	const under = (prefix: string, map: Record<string, string[]>) =>
		Object.entries(map)
			.filter(([k]) => k === prefix || k.startsWith(`${prefix}.`))
			.flatMap(([, v]) => v);

	/** Findings on a row that no field of it shows: role conflicts, the service's channel items. */
	function rowFindings(r: ChannelRow) {
		const items: { level: 'error' | 'warning'; message: string }[] = [];
		if (!r.enabled) return items;
		for (const m of at(`channel.${r.index}`)) items.push({ level: 'error', message: m });
		for (const m of W[`channel.${r.index}`] ?? []) items.push({ level: 'warning', message: m });
		const role = roleOf(r, d);
		const own = role === 'drive' ? `drive.${r.index}` : role === 'control' ? `control.${r.index}` : role === 'limit' ? `limit.${r.index}` : null;
		if (own) {
			const shown = new Set(['ceiling_vrms', 'peak_v', 'abort_vrms', 'alarm_db', 'abort_db', 'ceiling', 'abort_margin_db'].flatMap((f) => at(`${own}.${f}`)));
			for (const m of under(own, E)) if (!shown.has(m) && !(open[r.index] && role === 'drive')) items.push({ level: 'error', message: m });
			for (const m of under(own, W)) items.push({ level: 'warning', message: m });
		}
		return items;
	}

	/** The test roles a row may take: an output is a drive, an input any of the others. */
	const rolesOf = (direction: 'input' | 'output' | null) => {
		const allowed = rolesFor(direction);
		return MIMO_ROLES.filter((o) => (o.id === 'drive' ? allowed.includes('drive') : allowed.includes('control')));
	};

	function edit(i: number, e: RowEdit) {
		const rows = reach(sel, i);
		const { skipped } = acq.editRows(rows, e);
		skippedNote = skipped ? `${skipped} of ${rows.length} rows ${skipped === 1 ? 'does' : 'do'} not take that.` : null;
	}

	/** A test role for the row at `i`, or for every selected row when `i` is one of them. */
	function setRole(i: number, role: MimoRole) {
		if (locked) return;
		const rows = reach(sel, i);
		acq.materializeTable();
		let skipped = 0;
		for (const p of rows) {
			const s = acq.sized[p];
			const r = acq.rows[p];
			if (!s || !r || !rolesOf(s.direction).some((o) => o.id === role)) {
				skipped++;
				continue;
			}
			assignRole(r, role, d);
		}
		skippedNote = skipped ? `${skipped} of ${rows.length} rows ${skipped === 1 ? 'does' : 'do'} not take that: an input is never a drive, and an output is only a drive.` : null;
	}

	function loadShaker(ch: number) {
		loadingFor = ch;
		fileInput?.click();
	}

	async function onShakerFile(e: Event) {
		const input = e.currentTarget as HTMLInputElement;
		const file = input.files?.[0];
		input.value = '';
		if (!file || loadingFor === null) return;
		const s = d.drives[String(loadingFor)];
		if (s) s.shaker_text = await file.text();
		open[loadingFor] = true;
	}

	async function confirmOpen(path: string): Promise<boolean> {
		if (!acq.tableDirty) return true;
		return confirmFirst(`The channel table has edits that are not applied. Opening ${pathName(path)} replaces them. Open it anyway?`, 'Open');
	}

	const count = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;
	const summary = $derived(
		[count(t.controls.length, 'control', 'controls'), count(t.drives.length, 'drive', 'drives'), count(t.limits.length, 'limit', 'limits'), count(t.monitors.length, 'monitor', 'monitors')].join(', ')
	);
</script>

<section class="aq-panel" aria-labelledby="mc-title" data-testid="mimo-channels">
	<div class="aq-head">
		<h2 id="mc-title">Test Channels</h2>
		<span class="aq-sub num" data-testid="mimo-channel-count">{summary}</span>
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
				testid="mimo-channel-file"
			/>
		</div>
	</div>
	<p class="aq-muted aq-small lead">
		A drive is an output to one shaker amplifier. A control is a response the reference is written on. A limit is a response the controller holds under its own ceiling. A monitor is recorded and shown only.
	</p>

	{#if fileResults.length}
		<div class="fileresults">
			<Findings items={fileResults} />
			<button class="aq-icon-btn" aria-label="Dismiss" onclick={() => (fileResults = [])}>
				<svg viewBox="0 0 12 12" width="10" height="10" aria-hidden="true"><path d="M3 3l6 6M9 3l-6 6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
			</button>
		</div>
	{/if}

	<Findings items={at('channels').map((message) => ({ level: 'error' as const, message }))} testid="mimo-channels-findings" />

	{#if sel.rows.length >= 2}
		<div class="strip" role="status" data-testid="mimo-selection">
			<span><strong class="num">{sel.rows.length}</strong> rows selected. An edit to one of them reaches all {sel.rows.length}.</span>
			{#if skippedNote}<span class="skipped">{skippedNote}</span>{/if}
			<span class="aq-spacer"></span>
			<button class="btn sm" disabled={locked} onclick={() => acq.editRows(sel.rows, { field: 'enabled', value: true })}>On</button>
			<button class="btn sm" disabled={locked} onclick={() => acq.editRows(sel.rows, { field: 'enabled', value: false })}>Off</button>
			<button class="btn ghost sm" onclick={() => ((sel = NO_SELECTION), (skippedNote = null))}>Clear Selection</button>
		</div>
	{/if}

	<input type="file" accept=".json,application/json" class="visually-hidden" bind:this={fileInput} onchange={onShakerFile} tabindex="-1" aria-hidden="true" />

	<!-- Escape anywhere in the table clears the selection -->
	<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
	<div class="aq-scroll" role="group" aria-label="Test channels" onkeydown={(e) => e.key === 'Escape' && sel.rows.length && ((sel = NO_SELECTION), (skippedNote = null))}>
		<table class="aq-table roles" data-testid="mimo-channel-table">
			<thead>
				<tr>
					<th class="num">Channel</th>
					<th class="c-on">On</th>
					<th>Name</th>
					<th>Role</th>
					<th>Unit</th>
					<th class="num">Sensitivity</th>
					<th>For This Role</th>
				</tr>
			</thead>
			<tbody>
				{#each sized as s, i (i)}
					{@const r = s.row}
					{@const role = roleOf(r, d)}
					{@const rf = rowFindings(r)}
					{@const key = String(r.index)}
					{@const picked = isSelected(sel, i)}
					{@const choices = rolesOf(s.direction)}
					<tr class:off={!r.enabled} class:picked data-testid="mimo-row-{r.index}" data-role={role} data-on={r.enabled}>
						<td class="num">
							<button class="pick num" class:on={picked} aria-pressed={picked} aria-label="Select channel {r.index}" title="Select. Shift selects a run; Cmd or Ctrl adds or takes away one." data-testid="mimo-pick-{r.index}" onclick={(e) => ((sel = pick(sel, i, pickHow(e))), (skippedNote = null))}>
								{#if picked}<svg viewBox="0 0 12 12" width="9" height="9" aria-hidden="true"><path d="M2.5 6.2l2.4 2.4 4.6-5" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" /></svg>{/if}
								{r.index}
							</button>
						</td>
						<td class="c-on">
							<label class="aq-switch">
								<input type="checkbox" role="switch" aria-checked={r.enabled} aria-label="Channel {r.index} on" data-testid="mimo-on-{r.index}" checked={r.enabled} disabled={locked} onchange={(e) => edit(i, { field: 'enabled', value: e.currentTarget.checked })} />
								<span class="track" aria-hidden="true"><span class="thumb"></span></span>
								<span class="word">{r.enabled ? 'On' : 'Off'}</span>
							</label>
						</td>
						<td><input class="field name" aria-label="Channel {r.index} name" value={r.description} disabled={locked} oninput={(e) => edit(i, { field: 'description', value: e.currentTarget.value })} /></td>
						<td>
							<select class="field role" aria-label="Channel {r.index} role" data-testid="mimo-role-{r.index}" value={role === 'reference' ? '' : role} disabled={locked || choices.length < 2} aria-invalid={r.enabled && role === 'reference'} onchange={(e) => setRole(i, e.currentTarget.value as MimoRole)}>
								{#if role === 'reference'}<option value="" disabled>Choose a role</option>{/if}
								{#each choices as o (o.id)}<option value={o.id}>{o.label}</option>{/each}
							</select>
						</td>
						<td>
							{#if role === 'drive'}
								<span class="aq-muted">V</span>
							{:else}
								<input class="field units" list="mimo-units" aria-label="Channel {r.index} unit" value={r.units} disabled={locked} oninput={(e) => edit(i, { field: 'units', value: e.currentTarget.value })} />
							{/if}
						</td>
						<td class="num">
							{#if role === 'drive'}
								<span class="aq-muted aq-small">Output</span>
							{:else}
								<NumberField id="mc-{i}-sens" label="Channel {r.index} sensitivity" compact unit="mV/{r.units}" value={r.sensitivity_mv_per_eu} onchange={(v) => edit(i, { field: 'sensitivity_mv_per_eu', value: v ?? NaN })} errors={Number.isFinite(r.sensitivity_mv_per_eu) ? [] : ['Enter a number.']} disabled={locked} />
							{/if}
						</td>
						<td class="own">
							{#if !r.enabled}
								<span class="aq-muted aq-small">Off</span>
							{:else if role === 'drive' && d.drives[key]}
								{@const sd = d.drives[key]}
								<div class="inline">
									<NumberField id="mc-{r.index}-ceil" label="Ceiling" compact unit="V rms" bind:value={sd.ceiling_vrms} errors={at(`drive.${r.index}.ceiling_vrms`)} disabled={locked} testid="mimo-drive-{r.index}-ceiling" />
									<NumberField id="mc-{r.index}-peak" label="Peak" compact unit="V pk" bind:value={sd.peak_v} errors={at(`drive.${r.index}.peak_v`)} disabled={locked} testid="mimo-drive-{r.index}-peak" />
									<button class="btn sm ghost more" aria-expanded={!!open[r.index]} onclick={() => (open[r.index] = !open[r.index])} data-testid="mimo-drive-{r.index}-more">
										{sd.shaker_text.trim() ? 'Shaker record loaded' : 'Abort and shaker'}
									</button>
								</div>
							{:else if role === 'control' && d.controls[key]}
								{@const c = d.controls[key]}
								<div class="inline">
									<span class="band" title="Alarm band, below and above the reference">
										<span class="lbl">Alarm</span>
										<NumberField id="mc-{r.index}-al" label="Channel {r.index} alarm below" compact unit="dB" bind:value={c.alarm_db[0]} errors={at(`control.${r.index}.alarm_db`)} disabled={locked} />
										<NumberField id="mc-{r.index}-ah" label="Channel {r.index} alarm above" compact unit="dB" bind:value={c.alarm_db[1]} disabled={locked} />
									</span>
									<span class="band" title="Abort band, below and above the reference">
										<span class="lbl">Abort</span>
										<NumberField id="mc-{r.index}-bl" label="Channel {r.index} abort below" compact unit="dB" bind:value={c.abort_db[0]} errors={at(`control.${r.index}.abort_db`)} disabled={locked} />
										<NumberField id="mc-{r.index}-bh" label="Channel {r.index} abort above" compact unit="dB" bind:value={c.abort_db[1]} disabled={locked} />
									</span>
								</div>
							{:else if role === 'limit' && d.limits[key]}
								{@const l = d.limits[key]}
								<div class="inline">
									<NumberField id="mc-{r.index}-lim" label="Ceiling" compact unit="{r.units}²/Hz" bind:value={l.ceiling} errors={at(`limit.${r.index}.ceiling`)} disabled={locked} testid="mimo-limit-{r.index}-ceiling" />
									<NumberField id="mc-{r.index}-mar" label="Abort above the ceiling" compact unit="dB" bind:value={l.abort_margin_db} errors={at(`limit.${r.index}.abort_margin_db`)} disabled={locked} />
								</div>
							{:else if role === 'monitor'}
								<span class="aq-muted aq-small">Recorded and shown</span>
							{/if}
						</td>
					</tr>
					{#if r.enabled && role === 'drive' && open[r.index] && d.drives[key]}
						{@const sd = d.drives[key]}
						<tr class="aq-detail">
							<td colspan="7">
								<div class="detail" data-testid="mimo-drive-{r.index}-detail">
									<NumberField id="mc-{r.index}-abort" label="Drive abort" unit="V rms" nullable="Off" fallback={Number.isFinite(sd.ceiling_vrms) ? +(sd.ceiling_vrms * 1.12).toFixed(2) : 2.5} bind:value={sd.abort_vrms} errors={at(`drive.${r.index}.abort_vrms`)} disabled={locked} help="A realized drive at this rms aborts the run. Off: the ceiling's cap is the protection." />
									<NumberField id="mc-{r.index}-mass" label="Moving mass" unit="lb" bind:value={sd.moving_mass_lb} errors={at(`drive.${r.index}.moving_mass_lb`)} disabled={locked} help="This shaker's armature, adapter and fixturing." />
									<div class="shaker">
										<span class="lbl">Shaker record</span>
										<div class="aq-actions">
											<button class="btn sm" disabled={locked} onclick={() => loadShaker(r.index)}>Load File</button>
											{#if sd.shaker_text.trim()}<button class="btn sm ghost" disabled={locked} onclick={() => (sd.shaker_text = '')}>Clear</button>{/if}
										</div>
										<textarea class="field mono" rows="3" aria-label="Channel {r.index} shaker record" bind:value={sd.shaker_text} disabled={locked} placeholder="Paste the shaker record, or load its file. Without one, feasibility for this drive is not checked."></textarea>
										<Findings items={at(`drive.${r.index}.shaker`).map((message) => ({ level: 'error' as const, message }))} />
									</div>
								</div>
							</td>
						</tr>
					{/if}
					{#if rf.length}
						<tr class="aq-findrow"><td colspan="7"><Findings items={rf} /></td></tr>
					{/if}
				{/each}
			</tbody>
		</table>
	</div>
	<datalist id="mimo-units">{#each UNITS as u (u)}<option value={u}></option>{/each}</datalist>

	<div class="aq-group mass">
		<NumberField id="mc-article" label="Test article mass" unit="lb" bind:value={d.article_mass_lb} errors={at('channels.article_mass_lb')} disabled={locked} help="Split equally across the shakers when feasibility is checked." />
	</div>
</section>

<style>
	.lead {
		margin: -6px 0 var(--sp-3);
		max-width: 90ch;
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

	tr.off td:not(.c-on):not(:first-child) {
		opacity: 0.55;
	}

	.c-on {
		width: 70px;
	}

	.name {
		min-width: 140px;
	}

	.role {
		min-width: 110px;
	}

	.units {
		width: 72px;
	}

	.roles td.num :global(.nf) {
		min-width: 120px;
	}

	.inline {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 6px var(--sp-3);
	}

	.inline :global(.nf) {
		min-width: 112px;
	}

	.band {
		display: inline-flex;
		align-items: center;
		gap: 4px;
	}

	.band :global(.nf) {
		min-width: 88px;
	}

	.lbl {
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}

	.more {
		color: var(--text-2);
	}

	.detail {
		display: grid;
		grid-template-columns: repeat(2, minmax(180px, 220px)) minmax(0, 1fr);
		gap: var(--sp-3);
		align-items: start;
		padding: var(--sp-2) 0;
	}

	.shaker {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.mono {
		font-family: var(--font-mono);
		font-size: var(--text-xs);
	}

	.mass {
		max-width: 260px;
	}

	@media (max-width: 1100px) {
		.detail {
			grid-template-columns: minmax(0, 1fr);
		}
	}
</style>
