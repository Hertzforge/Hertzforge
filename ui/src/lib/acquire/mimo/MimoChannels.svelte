<script lang="ts">
	/**
	 * The test's channels: every row of the channel table with its role in the test (drive,
	 * control, limit or monitor), its unit and sensitivity, and what that role needs. A drive has
	 * its voltage ceiling (rms and peak), an optional abort level and the shaker behind it; a
	 * control has its alarm and abort bands; a limit has the PSD ceiling the core holds it under.
	 * The rows are the channel table's: a role or a unit changed here is changed in the table,
	 * which the setup applies with it. Wiring to the hardware is under Backend.
	 */
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import type { ChannelRow } from '$lib/protocol/types';
	import { assignRole, MIMO_ROLES, roleOf, rowName, type MimoRole } from './mimoSetup';
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
	const UNITS = ['g', 'm/s²', 'in/s', 'mil', 'lbf', 'N', 'V'];

	let open = $state<Record<number, boolean>>({});
	let fileInput = $state<HTMLInputElement | null>(null);
	let loadingFor = $state<number | null>(null);

	const at = (k: string) => E[k] ?? [];
	const under = (prefix: string, map: Record<string, string[]>) =>
		Object.entries(map)
			.filter(([k]) => k === prefix || k.startsWith(`${prefix}.`))
			.flatMap(([, v]) => v);

	/** Findings on a row that no field of it shows: role conflicts, the service's channel items. */
	function rowFindings(r: ChannelRow) {
		const items: { level: 'error' | 'warning'; message: string }[] = [];
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

	/** A new row, named after its index so every control has its own name. */
	function add(role: 'control' | 'drive') {
		acq.addRow(role);
		const r = acq.rows[acq.rows.length - 1];
		if (r) r.description = `${role === 'drive' ? 'Drive' : 'Control'} ${r.index}`;
	}

	function setRole(r: ChannelRow, role: MimoRole) {
		if (locked) return;
		assignRole(r, role, d);
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
			<button class="btn sm" data-testid="mimo-add-input" disabled={locked} onclick={() => add('control')}>Add Input</button>
			<button class="btn sm" data-testid="mimo-add-drive" disabled={locked} onclick={() => add('drive')}>Add Drive</button>
		</div>
	</div>
	<p class="aq-muted aq-small lead">
		A drive is an output to one shaker amplifier. A control is a response the reference is written on. A limit is a response the controller holds under its own ceiling. A monitor is recorded and shown only.
	</p>

	<Findings items={at('channels').map((message) => ({ level: 'error' as const, message }))} testid="mimo-channels-findings" />

	<input type="file" accept=".json,application/json" class="visually-hidden" bind:this={fileInput} onchange={onShakerFile} tabindex="-1" aria-hidden="true" />

	<div class="aq-scroll">
		<table class="aq-table roles" data-testid="mimo-channel-table">
			<thead>
				<tr>
					<th class="num">Channel</th>
					<th>Name</th>
					<th>Role</th>
					<th>Unit</th>
					<th class="num">Sensitivity</th>
					<th>For This Role</th>
				</tr>
			</thead>
			<tbody>
				{#each acq.rows as r (r)}
					{#if r.enabled}
						{@const role = roleOf(r, d)}
						{@const rf = rowFindings(r)}
						{@const key = String(r.index)}
						<tr data-testid="mimo-row-{r.index}" data-role={role}>
							<td class="num aq-muted">{r.index}</td>
							<td><input class="field name" aria-label="Channel {r.index} name" bind:value={r.description} disabled={locked} /></td>
							<td>
								<select class="field role" aria-label="Channel {r.index} role" data-testid="mimo-role-{r.index}" value={role === 'reference' ? '' : role} disabled={locked} aria-invalid={role === 'reference'} onchange={(e) => setRole(r, e.currentTarget.value as MimoRole)}>
									{#if role === 'reference'}<option value="" disabled>Choose a role</option>{/if}
									{#each MIMO_ROLES as o (o.id)}<option value={o.id}>{o.label}</option>{/each}
								</select>
							</td>
							<td>
								{#if role === 'drive'}
									<span class="aq-muted">V</span>
								{:else}
									<input class="field units" list="mimo-units" aria-label="Channel {r.index} unit" bind:value={r.units} disabled={locked} />
								{/if}
							</td>
							<td class="num">
								{#if role === 'drive'}
									<span class="aq-muted aq-small">Output</span>
								{:else}
									<NumberField id="mc-{r.index}-sens" label="Channel {r.index} sensitivity" compact unit="mV/{r.units}" value={r.sensitivity_mv_per_eu} onchange={(v) => (r.sensitivity_mv_per_eu = v ?? NaN)} errors={Number.isFinite(r.sensitivity_mv_per_eu) ? [] : ['Enter a number.']} disabled={locked} />
								{/if}
							</td>
							<td class="own">
								{#if role === 'drive' && d.drives[key]}
									{@const s = d.drives[key]}
									<div class="inline">
										<NumberField id="mc-{r.index}-ceil" label="Ceiling" compact unit="V rms" bind:value={s.ceiling_vrms} errors={at(`drive.${r.index}.ceiling_vrms`)} disabled={locked} testid="mimo-drive-{r.index}-ceiling" />
										<NumberField id="mc-{r.index}-peak" label="Peak" compact unit="V pk" bind:value={s.peak_v} errors={at(`drive.${r.index}.peak_v`)} disabled={locked} testid="mimo-drive-{r.index}-peak" />
										<button class="btn sm ghost more" aria-expanded={!!open[r.index]} onclick={() => (open[r.index] = !open[r.index])} data-testid="mimo-drive-{r.index}-more">
											{s.shaker_text.trim() ? 'Shaker record loaded' : 'Abort and shaker'}
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
						{#if role === 'drive' && open[r.index] && d.drives[key]}
							{@const s = d.drives[key]}
							<tr class="aq-detail">
								<td colspan="6">
									<div class="detail" data-testid="mimo-drive-{r.index}-detail">
										<NumberField id="mc-{r.index}-abort" label="Drive abort" unit="V rms" nullable="Off" fallback={Number.isFinite(s.ceiling_vrms) ? +(s.ceiling_vrms * 1.12).toFixed(2) : 2.5} bind:value={s.abort_vrms} errors={at(`drive.${r.index}.abort_vrms`)} disabled={locked} help="A realized drive at this rms aborts the run. Off: the ceiling's cap is the protection." />
										<NumberField id="mc-{r.index}-mass" label="Moving mass" unit="lb" bind:value={s.moving_mass_lb} errors={at(`drive.${r.index}.moving_mass_lb`)} disabled={locked} help="This shaker's armature, adapter and fixturing." />
										<div class="shaker">
											<span class="lbl">Shaker record</span>
											<div class="aq-actions">
												<button class="btn sm" disabled={locked} onclick={() => loadShaker(r.index)}>Load File</button>
												{#if s.shaker_text.trim()}<button class="btn sm ghost" disabled={locked} onclick={() => (s.shaker_text = '')}>Clear</button>{/if}
											</div>
											<textarea class="field mono" rows="3" aria-label="Channel {r.index} shaker record" bind:value={s.shaker_text} disabled={locked} placeholder="Paste the shaker record, or load its file. Without one, feasibility for this drive is not checked."></textarea>
											<Findings items={at(`drive.${r.index}.shaker`).map((message) => ({ level: 'error' as const, message }))} />
										</div>
									</div>
								</td>
							</tr>
						{/if}
						{#if rf.length}
							<tr class="aq-findrow"><td colspan="6"><Findings items={rf} /></td></tr>
						{/if}
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
