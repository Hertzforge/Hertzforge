<script lang="ts">
	/**
	 * How the loop identifies the system, controls, protects and ramps, plus response and force
	 * limits and the check against a shaker's ratings. Every field starts at the protocol's
	 * default and says in a line what it does to the run.
	 */
	import type { ChannelRow, LimitProfile } from '$lib/protocol/types';
	import Findings from './Findings.svelte';
	import NumberField from './NumberField.svelte';
	import { defaultControl, under, type ControlDraft } from './setup';
	import type { AcquireState } from './store.svelte';

	interface Props {
		acq: AcquireState;
	}
	let { acq }: Props = $props();

	const c = $derived(acq.setup.control);
	const locked = $derived(acq.locked);
	const E = $derived(acq.fieldErrors);
	const W = $derived(acq.fieldWarnings);
	const ctl = $derived(acq.server?.channels ?? []);
	const controlRows = $derived(ctl.filter((r) => r.enabled && r.role === 'control'));
	const inputRows = $derived(ctl.filter((r) => r.enabled && r.role !== 'drive'));

	type NumKey = { [K in keyof ControlDraft]: ControlDraft[K] extends number | null ? K : never }[keyof ControlDraft];

	interface Spec {
		key: NumKey;
		label: string;
		unit?: string;
		integer?: boolean;
		step?: number;
		min?: number;
		max?: number;
		nullable?: string;
		help: string;
	}

	const SYSID: Spec[] = [
		{ key: 'coherence_floor', label: 'Coherence floor', step: 0.05, min: 0, max: 1, help: 'The band coherence identification must reach. Below it, the run holds and never drives at test level.' }
	];
	const LOOP: Spec[] = [
		{ key: 'control_dof', label: 'Control degrees of freedom', step: 10, min: 2, help: 'Of the control estimate. Higher is steadier and slower to adapt.' },
		{ key: 'damping', label: 'Correction damping', step: 1, min: 0, max: 10, help: '0 corrects fully every loop; 10 spreads a correction over about ten loops.' },
		{ key: 'max_sigma', label: 'Drive clip', unit: 'sigma', step: 0.5, min: 1, nullable: 'No clip', help: 'Peaks of the drive are held to this many standard deviations.' }
	];
	const PROTECT: Spec[] = [
		{ key: 'max_lines_out', label: 'Lines out to abort', integer: true, step: 1, min: 0, help: 'Lines outside the abort band in one loop that trip the envelope abort.' },
		{ key: 'max_loops_out', label: 'Diverging loops to abort', integer: true, step: 1, min: 1, help: 'Diverging loops in a row that trip the divergence abort.' },
		{ key: 'drive_ceiling_vrms', label: 'Drive ceiling', unit: 'V rms', step: 0.5, min: 0, help: 'The drive is held at or below this.' },
		{ key: 'drive_abort_vrms', label: 'Drive abort', unit: 'V rms', step: 0.5, min: 0, nullable: 'None', help: 'A drive at or above this aborts the run.' }
	];
	const RAMPS: Spec[] = [
		{ key: 'soft_start_db', label: 'Soft start', unit: 'dB', step: 1, min: 0, help: 'The run engages this far below the first level and ramps up to it.' },
		{ key: 'soft_start_relax_db', label: 'Soft start band relief', unit: 'dB', step: 1, min: 0, help: 'Alarm and abort bands widen by this until the first level is reached.' },
		{ key: 'level_ramp_s', label: 'Level ramp', unit: 's', step: 0.5, min: 0, help: 'Ramp time of every level change.' },
		{ key: 'shutdown_ramp_s', label: 'Shutdown ramp', unit: 's', step: 0.5, min: 0, help: 'Ramp time of a controlled stop. An abort does not ramp.' }
	];

	const DEF = defaultControl();
	const errs = (k: string) => E[k] ?? [];
	const warns = (k: string) => W[k] ?? [];

	function weightOf(ch: number): number {
		return c.weights.find((w) => w.channel === ch)?.weight ?? 1;
	}

	function setWeight(ch: number, v: number) {
		const w = c.weights.find((x) => x.channel === ch);
		if (w) w.weight = v;
		else c.weights.push({ channel: ch, weight: v });
	}

	function addLimit() {
		const first = inputRows.find((r) => r.role !== 'control') ?? inputRows[0];
		acq.setup.limits.push({ channel: first?.index ?? 1, kind: 'response', rows: [{ freq_hz: 20, ceiling: 1 }, { freq_hz: 2000, ceiling: 1 }] });
	}

	const unitOf = (ch: number) => ctl.find((r: ChannelRow) => r.index === ch)?.units ?? 'g';

	async function loadShaker(e: Event) {
		const input = e.currentTarget as HTMLInputElement;
		const file = input.files?.[0];
		if (!file) return;
		acq.setup.feasibility.shaker_text = await file.text();
		acq.setup.feasibility.shaker_source = file.name;
		input.value = '';
	}

	function reset() {
		acq.setup.control = defaultControl();
	}
</script>

{#snippet fields(specs: Spec[])}
	<div class="aq-fields">
		{#each specs as s (s.key)}
			<NumberField
				id="ctl-{s.key}"
				label={s.label}
				unit={s.unit}
				integer={s.integer}
				step={s.step}
				min={s.min}
				max={s.max}
				nullable={s.nullable}
				fallback={(DEF[s.key] as number | null) ?? s.min ?? 1}
				bind:value={c[s.key]}
				errors={errs(`control.${s.key}`)}
				warnings={warns(`control.${s.key}`)}
				help={s.help}
				disabled={locked}
				testid="ctl-{s.key}"
			/>
		{/each}
	</div>
{/snippet}

{#snippet toggle(id: string, label: string, checked: boolean, set: (v: boolean) => void, help: string)}
	<div class="tog">
		<label class="aq-switch" for={id}>
			<input {id} type="checkbox" role="switch" {checked} disabled={locked} onchange={(e) => set(e.currentTarget.checked)} data-testid={id} />
			<span class="track" aria-hidden="true"><span class="thumb"></span></span>
			<span>{label}</span>
			<span class="word">{checked ? 'On' : 'Off'}</span>
		</label>
		<p class="help">{help}</p>
	</div>
{/snippet}

<section class="aq-panel control" aria-labelledby="control-title" data-testid="control-panel">
	<div class="aq-head">
		<h2 id="control-title">Control</h2>
		<span class="aq-sub">Every field starts at its usual value.</span>
		<span class="aq-spacer"></span>
		<button class="btn ghost sm" onclick={reset} disabled={locked}>Usual Values</button>
	</div>

	<div class="aq-group">
		<h3>System Identification</h3>
		{@render fields(SYSID)}
		<div class="aq-fields more">
			{@render toggle('ctl-review', 'Hold for review after identification', c.review_after_sysid, (v) => (c.review_after_sysid = v), 'Nothing drives at test level until you proceed.')}
			{@render toggle('ctl-shaped', 'Shaped identification drive', c.pretest.shaped, (v) => (c.pretest.shaped = v), 'Follow the reference shape instead of a flat band.')}
			<NumberField id="ctl-pre-level" label="Identification level" unit="dB" step={1} max={0} bind:value={c.pretest.level_db} errors={errs('control.pretest.level_db')} help="Of the control response, relative to the reference. The drive steps up from the starting drive until the response is there." disabled={locked} />
			<NumberField id="ctl-pre-avg" label="Identification averages" integer step={1} min={1} bind:value={c.pretest.averages} errors={errs('control.pretest.averages')} help="More is steadier and takes longer." disabled={locked} />
			<NumberField id="ctl-pre-max" label="Largest identification drive" unit="V rms" step={0.1} nullable="No limit" fallback={1} bind:value={c.pretest.max_vrms} errors={errs('control.pretest.max_vrms')} help="If the level needs more, the run stops before driving at test level." disabled={locked} />
			<NumberField id="ctl-pre-start" label="Starting identification drive" unit="V rms" step={0.01} min={0} bind:value={c.pretest.start_vrms} errors={errs('control.pretest.start_vrms')} help="Identification starts here and steps up." disabled={locked} />
		</div>
	</div>

	<div class="aq-group">
		<h3>Control Loop</h3>
		{@render fields(LOOP)}
		{#if controlRows.length > 1}
			<div class="multi">
				<div class="sel">
					<label for="ctl-composite" class="lbl">Control channels combine by</label>
					<select id="ctl-composite" class="field" bind:value={c.composite} disabled={locked}>
						<option value="weighted_avg">Weighted average</option>
						<option value="min">Lowest channel</option>
						<option value="max">Highest channel</option>
					</select>
				</div>
				{#each controlRows as r (r.index)}
					<NumberField id="ctl-w-{r.index}" label="Weight, channel {r.index} {r.description}" step={0.1} min={0} value={weightOf(r.index)} onchange={(v) => setWeight(r.index, v ?? NaN)} disabled={locked} />
				{/each}
			</div>
			<Findings items={under(E, 'control.weights').map((message) => ({ level: 'error' as const, message }))} />
		{/if}
		<Findings items={(E['control'] ?? []).map((message) => ({ level: 'error' as const, message }))} />
	</div>

	<div class="aq-group">
		<h3>Protection</h3>
		<p class="aq-lead">The controller's own aborts. Each channel's absolute limits are in the channel table.</p>
		{@render fields(PROTECT)}
	</div>

	<div class="aq-group">
		<h3>Ramps</h3>
		{@render fields(RAMPS)}
	</div>

	<div class="aq-group" data-testid="limits">
		<h3>Response and Force Limits</h3>
		<p class="aq-lead">Where a channel would pass its ceiling, the controller lowers the drive at those lines. A limit never aborts.</p>
		<Findings items={under(E, 'limits').filter((m) => !/rows/.test(m)).map((message) => ({ level: 'error' as const, message }))} />
		{#each acq.setup.limits as lim, i (lim)}
			{@const unit = `${unitOf(lim.channel)}²/Hz`}
			<div class="limit">
				<div class="limhead">
					<div class="sel">
						<label for="lim-{i}-ch" class="lbl">Channel</label>
						<select id="lim-{i}-ch" class="field" value={lim.channel} disabled={locked} onchange={(e) => (lim.channel = Number(e.currentTarget.value))}>
							{#each inputRows as r (r.index)}<option value={r.index}>{r.index} {r.description}</option>{/each}
							{#if !inputRows.some((r) => r.index === lim.channel)}<option value={lim.channel}>{lim.channel} (not an enabled input)</option>{/if}
						</select>
					</div>
					<div class="aq-seg sm" role="radiogroup" aria-label="Limit {i + 1} kind">
						{#each [['response', 'Response'], ['force', 'Force']] as [k, label] (k)}
							<button type="button" role="radio" aria-checked={lim.kind === k} disabled={locked} onclick={() => (lim.kind = k as LimitProfile['kind'])}>{label}</button>
						{/each}
					</div>
					<span class="aq-spacer"></span>
					<button class="btn ghost sm" disabled={locked} onclick={() => acq.setup.limits.splice(i, 1)}>Remove Limit</button>
				</div>
				<table class="aq-table limrows">
					<thead><tr><th class="num">Frequency</th><th class="num">Ceiling</th><th></th></tr></thead>
					<tbody>
						{#each lim.rows as row, k (row)}
							<tr>
								<td class="num"><NumberField id="lim-{i}-{k}-f" label="Limit {i + 1} row {k + 1} frequency" compact unit="Hz" bind:value={row.freq_hz} disabled={locked} /></td>
								<td class="num"><NumberField id="lim-{i}-{k}-c" label="Limit {i + 1} row {k + 1} ceiling" compact {unit} bind:value={row.ceiling} disabled={locked} /></td>
								<td class="acts">
									<button class="aq-icon-btn" aria-label="Remove row {k + 1}" disabled={locked || lim.rows.length <= 1} onclick={() => lim.rows.splice(k, 1)}>
										<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M3 3l6 6M9 3l-6 6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
									</button>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
				<Findings items={under(E, `limits.${i}`).map((message) => ({ level: 'error' as const, message }))} />
				<button class="btn sm" disabled={locked} onclick={() => lim.rows.push({ freq_hz: (lim.rows.at(-1)?.freq_hz ?? 10) * 2, ceiling: lim.rows.at(-1)?.ceiling ?? 1 })}>Add Row</button>
			</div>
		{/each}
		<button class="btn sm" disabled={locked || inputRows.length === 0} onclick={addLimit} data-testid="limit-add">Add Limit</button>
	</div>

	<div class="aq-group" data-testid="feasibility-editor">
		<h3>Shaker Ratings</h3>
		{@render toggle('feas-on', 'Check the profile against a shaker', acq.setup.feasibility.enabled, (v) => (acq.setup.feasibility.enabled = v), 'Checked when the run arms. A profile beyond the ratings blocks arming until you acknowledge it in your own words.')}
		{#if acq.setup.feasibility.enabled}
			{@const f = acq.setup.feasibility}
			<div class="shaker">
				<div class="aq-actions">
					<label class="btn sm file">
						Load Shaker Record
						<input type="file" accept=".json,application/json" onchange={loadShaker} disabled={locked} />
					</label>
					<span class="aq-small aq-muted">{f.shaker_source ? `Loaded from ${f.shaker_source}` : f.shaker_text.trim() ? 'Pasted' : 'No record yet'}</span>
				</div>
				<label for="feas-text" class="lbl">Shaker record</label>
				<textarea id="feas-text" class="field mono" rows="4" bind:value={f.shaker_text} disabled={locked} spellcheck="false" aria-invalid={(E['feasibility.shaker'] ?? []).length > 0} data-testid="feas-text"></textarea>
				<Findings items={(E['feasibility.shaker'] ?? []).map((message) => ({ level: 'error' as const, message }))} />
				<h4>Moving Mass</h4>
				<div class="aq-fields">
					<NumberField id="mm-arm" label="Armature" unit="lb" bind:value={f.moving_mass_lb.armature} errors={E['feasibility.moving_mass_lb.armature'] ?? []} disabled={locked} />
					<NumberField id="mm-slip" label="Slip table" unit="lb" bind:value={f.moving_mass_lb.slip_table} disabled={locked} />
					<NumberField id="mm-adapt" label="Drive adapter" unit="lb" bind:value={f.moving_mass_lb.drive_adapter} disabled={locked} />
					<NumberField id="mm-fix" label="Fixturing" unit="lb" bind:value={f.moving_mass_lb.fixturing} disabled={locked} />
					<NumberField id="mm-art" label="Test article" unit="lb" bind:value={f.moving_mass_lb.test_article} disabled={locked} />
					<NumberField id="mm-cf" label="Crest factor" step={0.1} min={1} bind:value={f.crest_factor} errors={E['feasibility.crest_factor'] ?? []} disabled={locked} />
				</div>
			</div>
		{/if}
	</div>
</section>

<style>
	.more {
		margin-top: var(--sp-3);
	}

	.tog {
		display: flex;
		flex-direction: column;
		gap: 4px;
		padding-top: 18px;
	}

	.aq-group > .tog {
		padding-top: 0;
	}

	.help {
		font-size: var(--text-2xs);
		color: var(--text-3);
		line-height: 1.4;
	}

	.multi {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(190px, 1fr));
		gap: var(--sp-3) var(--sp-4);
		margin-top: var(--sp-3);
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

	.limit {
		margin-bottom: var(--sp-3);
		padding: var(--sp-3);
		background: var(--bg-2);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-md);
	}

	.limhead {
		display: flex;
		flex-wrap: wrap;
		align-items: flex-end;
		gap: var(--sp-3);
		margin-bottom: var(--sp-2);
	}

	.limhead .aq-spacer {
		flex: 1;
	}

	.limrows {
		max-width: 420px;
		margin-bottom: var(--sp-2);
	}

	.limrows th,
	.limrows td {
		background: transparent;
	}

	.acts {
		width: 1%;
	}

	.shaker {
		display: flex;
		flex-direction: column;
		gap: 6px;
		margin-top: var(--sp-3);
	}

	.shaker h4 {
		margin-top: var(--sp-3);
		font-size: var(--text-xs);
		font-weight: var(--weight-strong);
		color: var(--text-2);
	}

	textarea {
		height: auto;
		padding: 8px;
		font-size: var(--text-xs);
		resize: vertical;
	}

	.file {
		position: relative;
		overflow: hidden;
	}

	.file input {
		position: absolute;
		inset: 0;
		opacity: 0;
		cursor: pointer;
	}

	.file:focus-within {
		outline: 2px solid var(--focus);
		outline-offset: 2px;
	}
</style>
