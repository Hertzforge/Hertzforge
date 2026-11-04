<script lang="ts">
	/**
	 * The reference profile: breakpoint rows, each a PSD anchor or a slope from the row before,
	 * with the profile's alarm and abort bands and, on any row, its own. Findings from the editor
	 * and from the service's dry run sit on the row and the cell they name.
	 */
	import type { BandDb } from '$lib/protocol/types';
	import Findings from './Findings.svelte';
	import NumberField from './NumberField.svelte';
	import { newBreakpoint, under, type BreakpointDraft, type ProfileHost } from './setup';

	interface Props {
		acq: ProfileHost;
		/** The panel's heading and the line under it (Random: "Profile"). */
		heading?: string;
		sub?: string;
		/** The test's title field (Random's profile carries it). */
		showTitle?: boolean;
		/** Each row's own bands (Random's envelope); off where the bands are the channel's. */
		rowBands?: boolean;
		/** The bands group under the table. */
		showBands?: boolean;
		/** The PSD's unit. */
		unit?: string;
	}
	let { acq, heading = 'Profile', sub = 'The reference PSD the run controls to, at full level.', showTitle = true, rowBands = true, showBands = true, unit = 'g²/Hz' }: Props = $props();

	const d = $derived(acq.setup);
	const locked = $derived(acq.locked);
	const E = $derived(acq.fieldErrors);
	const W = $derived(acq.fieldWarnings);

	let open = $state<Record<number, boolean>>({});

	const key = (i: number, col: string) => `profile.breakpoints.${i}.${col}`;
	const at = (map: Record<string, string[]>, k: string) => map[k] ?? [];

	function rowFindings(i: number) {
		const items: { level: 'error' | 'warning'; message: string }[] = [];
		for (const m of under(E, `profile.breakpoints.${i}`)) items.push({ level: 'error', message: m });
		for (const m of under(W, `profile.breakpoints.${i}`)) items.push({ level: 'warning', message: m });
		// Each field shows its own; the row lists what no field carries.
		const shown = new Set([...at(E, key(i, 'freq_hz')), ...at(E, key(i, 'value'))]);
		return items.filter((f) => !shown.has(f.message));
	}

	function bandText(b: BandDb): string {
		return b[0] === b[1] ? `±${b[0]}` : `-${b[0]}/+${b[1]}`;
	}

	function bandsSummary(b: BreakpointDraft): string | null {
		if (!b.alarm_db && !b.abort_db) return null;
		const alarm = b.alarm_db ?? d.alarm_db;
		const abort = b.abort_db ?? d.abort_db;
		return `Alarm ${bandText(alarm)}, abort ${bandText(abort)} dB`;
	}

	function ownBands(i: number) {
		const b = d.breakpoints[i];
		b.alarm_db = [...(b.alarm_db ?? d.alarm_db)] as BandDb;
		b.abort_db = [...(b.abort_db ?? d.abort_db)] as BandDb;
		open[i] = true;
	}

	function profileBands(i: number) {
		const b = d.breakpoints[i];
		b.alarm_db = null;
		b.abort_db = null;
		open[i] = false;
	}

	function insertAfter(i: number) {
		const rows = d.breakpoints;
		const here = rows[i]?.freq_hz;
		const next = rows[i + 1]?.freq_hz;
		const f = Number.isFinite(here) && Number.isFinite(next) ? Math.round(Math.sqrt(here * next)) : Number.isFinite(here) ? here * 2 : 20;
		rows.splice(i + 1, 0, newBreakpoint(f, i < 0 ? 'psd' : 'slope', i < 0 ? 0.01 : 0));
		open = {};
	}

	function remove(i: number) {
		d.breakpoints.splice(i, 1);
		open = {};
	}
</script>

<section class="aq-panel profile" aria-labelledby="profile-title" data-testid="profile-panel">
	<div class="aq-head">
		<h2 id="profile-title">{heading}</h2>
		<span class="aq-sub">{sub}</span>
	</div>

	{#if showTitle}
		<div class="title-row">
			<label for="setup-title" class="lbl">Test title</label>
			<input id="setup-title" class="field" bind:value={d.title} disabled={locked} aria-invalid={at(E, 'title').length > 0} data-testid="setup-title" />
			<Findings items={at(E, 'title').map((message) => ({ level: 'error' as const, message }))} />
		</div>
	{/if}

	<Findings items={at(E, 'profile.breakpoints').map((message) => ({ level: 'error' as const, message }))} />

	<div class="aq-scroll">
		<table class="aq-table bp" data-testid="breakpoint-table">
			<thead>
				<tr>
					<th class="num">Row</th>
					<th class="num">Frequency</th>
					<th>Defined By</th>
					<th class="num">Value</th>
					{#if rowBands}<th>Bands</th>{/if}
					<th><span class="visually-hidden">Row actions</span></th>
				</tr>
			</thead>
			<tbody>
				{#each d.breakpoints as b, i (b)}
					{@const rf = rowFindings(i)}
					<tr data-testid="bp-row-{i}">
						<td class="num aq-muted">{i + 1}</td>
						<td class="num">
							<NumberField id="bp-{i}-f" label="Row {i + 1} frequency" compact unit="Hz" bind:value={b.freq_hz} errors={at(E, key(i, 'freq_hz'))} disabled={locked} testid="bp-{i}-freq" />
						</td>
						<td>
							<div class="aq-seg sm" role="radiogroup" aria-label="Row {i + 1} defined by">
								<button type="button" role="radio" aria-checked={b.kind === 'psd'} disabled={locked} onclick={() => (b.kind = 'psd')} data-testid="bp-{i}-psd">PSD</button>
								<button type="button" role="radio" aria-checked={b.kind === 'slope'} disabled={locked || i === 0} title={i === 0 ? 'The first row is a PSD anchor.' : undefined} onclick={() => (b.kind = 'slope')} data-testid="bp-{i}-slope">Slope</button>
							</div>
						</td>
						<td class="num">
							{#if b.kind === 'psd'}
								<NumberField id="bp-{i}-v" label="Row {i + 1} PSD" compact {unit} bind:value={b.psd_g2_hz} errors={at(E, key(i, 'value'))} disabled={locked} testid="bp-{i}-value" />
							{:else}
								<NumberField id="bp-{i}-v" label="Row {i + 1} slope" compact unit="dB/oct" bind:value={b.slope_db_oct} errors={at(E, key(i, 'value'))} disabled={locked} testid="bp-{i}-value" />
							{/if}
						</td>
						{#if rowBands}<td>
							{#if bandsSummary(b)}
								<button class="bands own" aria-expanded={!!open[i]} onclick={() => (open[i] = !open[i])} disabled={locked}>{bandsSummary(b)}</button>
							{:else}
								<button class="bands" onclick={() => ownBands(i)} disabled={locked} title="Give this row its own alarm and abort bands">Profile bands</button>
							{/if}
						</td>{/if}
						<td class="acts">
							<button class="aq-icon-btn" aria-label="Insert a row after row {i + 1}" disabled={locked} onclick={() => insertAfter(i)}>
								<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M6 2v8M2 6h8" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
							</button>
							<button class="aq-icon-btn" aria-label="Remove row {i + 1}" disabled={locked || d.breakpoints.length <= 1} onclick={() => remove(i)}>
								<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M3 3l6 6M9 3l-6 6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
							</button>
						</td>
					</tr>
					{#if rf.length}
						<tr class="aq-findrow"><td colspan="6"><Findings items={rf} /></td></tr>
					{/if}
					{#if rowBands && open[i] && b.alarm_db && b.abort_db}
						<tr class="aq-detail">
							<td colspan="6">
								<div class="own-bands">
									<span class="lbl">Row {i + 1} to the next row</span>
									<NumberField id="bp-{i}-al" label="Alarm below" unit="dB" bind:value={b.alarm_db[0]} disabled={locked} />
									<NumberField id="bp-{i}-ah" label="Alarm above" unit="dB" bind:value={b.alarm_db[1]} disabled={locked} />
									<NumberField id="bp-{i}-bl" label="Abort below" unit="dB" bind:value={b.abort_db[0]} disabled={locked} />
									<NumberField id="bp-{i}-bh" label="Abort above" unit="dB" bind:value={b.abort_db[1]} disabled={locked} />
									<button class="btn sm ghost" onclick={() => profileBands(i)} disabled={locked}>Use the Profile Bands</button>
								</div>
							</td>
						</tr>
					{/if}
				{/each}
			</tbody>
		</table>
	</div>
	<div class="aq-actions add">
		<button class="btn sm" data-testid="bp-add" disabled={locked} onclick={() => insertAfter(d.breakpoints.length - 1)}>Add Row</button>
	</div>

	{#if showBands}
	<div class="aq-group bands-group">
		<h3>Bands Around the Reference</h3>
		<p class="aq-lead">Positive dB below and above the reference. An alarm warns; the abort band trips the envelope abort. A row's own bands hold from that row to the next.</p>
		<div class="aq-fields">
			<NumberField id="pb-al" label="Alarm below" unit="dB" bind:value={d.alarm_db[0]} errors={at(E, 'profile.alarm_db')} disabled={locked} />
			<NumberField id="pb-ah" label="Alarm above" unit="dB" bind:value={d.alarm_db[1]} disabled={locked} />
			<NumberField id="pb-bl" label="Abort below" unit="dB" bind:value={d.abort_db[0]} errors={at(E, 'profile.abort_db')} disabled={locked} />
			<NumberField id="pb-bh" label="Abort above" unit="dB" bind:value={d.abort_db[1]} disabled={locked} />
		</div>
	</div>
	{/if}
</section>

<style>
	.title-row {
		display: grid;
		grid-template-columns: auto minmax(0, 420px);
		align-items: center;
		gap: 4px var(--sp-3);
		margin-bottom: var(--sp-3);
	}

	.title-row :global(.aq-findings) {
		grid-column: 2;
	}

	.lbl {
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}

	.bp td.num :global(.nf) {
		min-width: 130px;
	}

	.bands {
		height: 26px;
		padding: 0 8px;
		font-size: var(--text-xs);
		color: var(--text-3);
		background: transparent;
		border: 1px dashed var(--line-2);
		border-radius: var(--radius-sm);
		white-space: nowrap;
	}

	.bands.own {
		color: var(--text-1);
		border-style: solid;
		border-color: var(--line-3);
		background: var(--bg-2);
	}

	.bands:hover:not(:disabled) {
		border-color: var(--line-3);
		color: var(--text-1);
	}

	.acts {
		width: 1%;
		white-space: nowrap;
	}

	.own-bands {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
		gap: var(--sp-3);
		align-items: end;
	}

	.own-bands .lbl {
		grid-column: 1 / -1;
	}

	.add {
		margin-top: var(--sp-2);
	}

	.bands-group {
		margin-top: var(--sp-4);
	}

	.bands-group :global(.aq-fields) {
		grid-template-columns: repeat(auto-fill, minmax(130px, 1fr));
	}
</style>
