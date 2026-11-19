<script lang="ts">
	/**
	 * What protects the shaker and the test article: the summed drive's ceiling and abort in volts
	 * peak (the random part and every tone together), the random part's own drive ceiling, abort and
	 * clip, the floor's envelope and divergence aborts, the kinematic monitor, the limit channels
	 * (Random's ceilings, which lower the random part only), and the check against a shaker's
	 * ratings. A limit channel's rows are saved to and opened from a breakpoint table file (`limit`).
	 */
	import type { ChannelRow, LimitProfile } from '$lib/protocol/types';
	import type { LoadedTable } from '$lib/tables/tableFiles';
	import { pointsRows, pointsTable, type PointsFile } from '../breakpointFile';
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import TableFileBar from '../TableFileBar.svelte';
	import { under } from '../setup';
	import type { MixedStore } from './mixedStore.svelte';

	interface Props {
		mixed: MixedStore;
	}
	let { mixed }: Props = $props();

	const d = $derived(mixed.setup);
	const lim = $derived(d.limits);
	const c = $derived(d.control);
	const locked = $derived(mixed.locked);
	const E = $derived(mixed.fieldErrors);
	const W = $derived(mixed.fieldWarnings);
	const errs = (k: string) => E[k] ?? [];
	const inputRows = $derived(mixed.rows.filter((r) => r.enabled && r.role !== 'drive'));
	const unitOf = (ch: number) => mixed.rows.find((r: ChannelRow) => r.index === ch)?.units ?? 'g';
	const dry = $derived(mixed.derived);

	type LimitRow = LimitProfile['rows'][number];
	const limitFile = (unit: string): PointsFile<LimitRow> => ({ keys: ['freq_hz', 'ceiling'], column: 'ceiling', unit });

	function openLimit(l: LimitProfile, unit: string, loaded: LoadedTable) {
		const { rows, notes } = pointsRows(loaded, limitFile(unit));
		l.rows.splice(0, l.rows.length, ...rows);
		return notes;
	}

	let shakerSource = $state<string | null>(null);
	async function loadShaker(e: Event) {
		const input = e.currentTarget as HTMLInputElement;
		const file = input.files?.[0];
		if (!file) return;
		d.feasibility.shaker_text = await file.text();
		shakerSource = file.name;
		input.value = '';
	}
</script>

<div class="cols">
	<section class="aq-panel" aria-labelledby="ml-drive" data-testid="mixed-drive-limits">
		<div class="aq-head"><h2 id="ml-drive">The Drive</h2><span class="aq-sub">There is no default: these are your amplifier's input.</span></div>
		<div class="aq-fields">
			<NumberField id="ml-ceil" label="Summed drive ceiling" unit="V pk" step={0.1} min={0} max={10} value={lim.drive_vpk_ceiling} onchange={(v) => (lim.drive_vpk_ceiling = v ?? NaN)} errors={errs('limits.drive_vpk_ceiling')} disabled={locked} testid="mixed-vpk-ceiling" help="The random part and every tone together are held at or under this, judged on every frame." />
			<NumberField id="ml-abort" label="Summed drive abort" unit="V pk" step={0.1} min={0} max={10} value={lim.drive_vpk_abort} onchange={(v) => (lim.drive_vpk_abort = v ?? NaN)} errors={errs('limits.drive_vpk_abort')} disabled={locked} testid="mixed-vpk-abort" help="A summed drive sample at or over this aborts the run. At least the ceiling." />
		</div>
		<h3>The Random Part</h3>
		<div class="aq-fields">
			<NumberField id="ml-rc" label="Random drive ceiling" unit="V rms" step={0.5} min={0} value={c.drive_ceiling_vrms} onchange={(v) => (c.drive_ceiling_vrms = v ?? NaN)} errors={errs('control.drive_ceiling_vrms')} disabled={locked} testid="mixed-vrms-ceiling" help="The floor and narrowbands' synthesis is held at or under this. Usual: 10 V rms." />
			<NumberField id="ml-ra" label="Random drive abort" unit="V rms" nullable="None" fallback={10} step={0.5} min={0} value={c.drive_abort_vrms} onchange={(v) => (c.drive_abort_vrms = v)} errors={errs('control.drive_abort_vrms')} disabled={locked} testid="mixed-vrms-abort" />
			<NumberField id="ml-sig" label="Random drive clip" unit="sigma" nullable="No clip" fallback={5} step={0.5} min={1} value={c.max_sigma} onchange={(v) => (c.max_sigma = v)} errors={errs('control.max_sigma')} disabled={locked} testid="mixed-sigma" help="Peaks of the random part, before the tones are added. Usual: 5." />
		</div>
		<h3>The Floor's Aborts</h3>
		<div class="aq-fields">
			<NumberField id="ml-lines" label="Lines out to abort" integer step={1} min={0} value={c.max_lines_out} onchange={(v) => (c.max_lines_out = v ?? NaN)} errors={errs('control.max_lines_out')} disabled={locked} testid="mixed-lines-out" help="Clean lines outside the floor's abort band in one loop. Usual: 10." />
			<NumberField id="ml-loops" label="Diverging loops to abort" integer step={1} min={1} value={c.max_loops_out} onchange={(v) => (c.max_loops_out = v ?? NaN)} errors={errs('control.max_loops_out')} disabled={locked} testid="mixed-loops-out" help="Usual: 5." />
		</div>
		<label class="check"><input type="checkbox" bind:checked={lim.kinematic_monitor} disabled={locked} data-testid="mixed-kinematic" /> Abort when the response's velocity or displacement passes the shaker's rating (needs the shaker record below)</label>
	</section>

	<section class="aq-panel" aria-labelledby="ml-lim" data-testid="mixed-limit-channels">
		<div class="aq-head"><h2 id="ml-lim">Limit Channels</h2></div>
		<p class="aq-lead aq-small" data-testid="mixed-limit-note">A limit lowers the random part only, never a tone.</p>
		<Findings items={(E['limits.channels'] ?? []).map((message) => ({ level: 'error' as const, message }))} />
		{#each lim.channels as l, i (l)}
			{@const unit = `${unitOf(l.channel)}²/Hz`}
			<div class="limit" data-testid="mixed-limit-{i}">
				<div class="limhead">
					<div class="sel">
						<label for="ml-{i}-ch" class="lbl">Channel</label>
						<select id="ml-{i}-ch" class="field" value={l.channel} disabled={locked} onchange={(e) => (l.channel = Number(e.currentTarget.value))}>
							{#each inputRows as r (r.index)}<option value={r.index}>{r.index} {r.description}</option>{/each}
							{#if !inputRows.some((r) => r.index === l.channel)}<option value={l.channel}>{l.channel} (not an enabled input)</option>{/if}
						</select>
					</div>
					<div class="aq-seg sm" role="radiogroup" aria-label="Limit {i + 1} kind">
						{#each [['response', 'Response'], ['force', 'Force']] as [k, label] (k)}
							<button type="button" role="radio" aria-checked={l.kind === k} disabled={locked} onclick={() => (l.kind = k as LimitProfile['kind'])}>{label}</button>
						{/each}
					</div>
					<span class="aq-spacer"></span>
					<button class="btn ghost sm" disabled={locked} onclick={() => lim.channels.splice(i, 1)}>Remove Limit</button>
				</div>
				<table class="aq-table limrows">
					<thead><tr><th class="num">Frequency</th><th class="num">Ceiling</th><th></th></tr></thead>
					<tbody>
						{#each l.rows as row, k (row)}
							<tr>
								<td class="num"><NumberField id="ml-{i}-{k}-f" label="Limit {i + 1} row {k + 1} frequency" compact unit="Hz" value={row.freq_hz} onchange={(v) => (row.freq_hz = v ?? NaN)} disabled={locked} /></td>
								<td class="num"><NumberField id="ml-{i}-{k}-c" label="Limit {i + 1} row {k + 1} ceiling" compact {unit} value={row.ceiling} onchange={(v) => (row.ceiling = v ?? NaN)} disabled={locked} /></td>
								<td><button class="btn ghost sm" aria-label="Remove row {k + 1}" disabled={locked || l.rows.length <= 1} onclick={() => l.rows.splice(k, 1)}>Remove</button></td>
							</tr>
						{/each}
					</tbody>
				</table>
				<Findings items={under(E, `limits.channels.${i}`).map((message) => ({ level: 'error' as const, message }))} />
				<TableFileBar client={mixed.tableFiles} tableKind="limit" name="Limit" table={() => pointsTable(l.rows, limitFile(unit))} onopen={(loaded) => openLimit(l, unit, loaded)} {locked} testid="mixed-limit-{i}">
					<button class="btn sm" disabled={locked} onclick={() => l.rows.push({ freq_hz: (l.rows.at(-1)?.freq_hz ?? 10) * 2, ceiling: l.rows.at(-1)?.ceiling ?? 1 })}>Add Row</button>
				</TableFileBar>
			</div>
		{/each}
		<button class="btn sm" disabled={locked || inputRows.length === 0} onclick={() => mixed.addLimitChannel()} data-testid="mixed-limit-add">Add Limit</button>

		<h3>Shaker Ratings</h3>
		<label class="check"><input type="checkbox" bind:checked={d.feasibility.enabled} disabled={locked} data-testid="mixed-feas-on" /> Check the predicted peak of the sum against a shaker, at configure and at arm</label>
		{#if d.feasibility.enabled}
			{@const f = d.feasibility}
			<div class="shaker">
				<div class="aq-actions">
					<label class="btn sm file">
						Load Shaker Record
						<input type="file" accept=".json,application/json" onchange={loadShaker} disabled={locked} />
					</label>
					<span class="aq-small aq-muted">{shakerSource ? `Loaded from ${shakerSource}` : f.shaker_text.trim() ? 'Pasted' : 'No record yet'}</span>
				</div>
				<label for="ml-feas" class="lbl">Shaker record</label>
				<textarea id="ml-feas" class="field mono" rows="4" bind:value={f.shaker_text} disabled={locked} spellcheck="false" aria-invalid={errs('feasibility.shaker').length > 0} data-testid="mixed-feas-text"></textarea>
				<Findings items={errs('feasibility.shaker').map((message) => ({ level: 'error' as const, message }))} />
				<div class="aq-fields">
					<NumberField id="ml-arm" label="Armature" unit="lb" bind:value={f.moving_mass_lb.armature} errors={errs('feasibility.moving_mass_lb.armature')} disabled={locked} />
					<NumberField id="ml-slip" label="Slip table" unit="lb" bind:value={f.moving_mass_lb.slip_table} disabled={locked} />
					<NumberField id="ml-adapt" label="Drive adapter" unit="lb" bind:value={f.moving_mass_lb.drive_adapter} disabled={locked} />
					<NumberField id="ml-fix" label="Fixturing" unit="lb" bind:value={f.moving_mass_lb.fixturing} disabled={locked} />
					<NumberField id="ml-art" label="Test article" unit="lb" bind:value={f.moving_mass_lb.test_article} disabled={locked} />
					<NumberField id="ml-cf" label="Crest factor" step={0.1} min={1} bind:value={f.crest_factor} errors={errs('feasibility.crest_factor')} warnings={W['feasibility.crest_factor'] ?? []} disabled={locked} help="Times the random part's rms, in the predicted peak. Usual: 3." />
				</div>
				{#if dry}<p class="aq-small" data-testid="mixed-feas-verdict">{dry.feasibility.message}</p>{/if}
			</div>
		{/if}
	</section>
</div>

<style>
	.cols {
		display: grid;
		grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
		gap: var(--sp-3);
		align-items: start;
	}

	h3 {
		margin: var(--sp-4) 0 var(--sp-2);
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
	}

	.check {
		display: flex;
		align-items: center;
		gap: 6px;
		margin-top: var(--sp-2);
		font-size: var(--text-sm);
	}

	.limit {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		margin-bottom: var(--sp-3);
		padding-bottom: var(--sp-3);
		border-bottom: 1px solid var(--line-1);
	}

	.limhead {
		display: flex;
		align-items: flex-end;
		gap: var(--sp-2);
	}

	.sel {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.lbl {
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}

	.shaker {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		margin-top: var(--sp-2);
	}

	.file input {
		display: none;
	}

	@media (max-width: 1240px) {
		.cols {
			grid-template-columns: minmax(0, 1fr);
		}
	}
</style>
