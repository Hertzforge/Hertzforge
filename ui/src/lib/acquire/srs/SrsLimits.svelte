<script lang="ts">
	/**
	 * What protects the shaker and the article: the drive ceiling and the drive abort in volts peak
	 * (no default: they are the amplifier's input, so the setup asks for both), a predicted peak
	 * acceleration ceiling and how far past a ceiling the run aborts instead of scaling, ceilings on
	 * the pulse's kinematics (checked on the prediction before each pulse and on the measured pulse
	 * after), monitored channels, and the shaker and moving mass the synthesis is checked against at
	 * arm.
	 */
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import { rowName } from '../mimo/mimoSetup';
	import { newMonitor } from './srsSetup';
	import type { SrsStore } from './srsStore.svelte';

	interface Props {
		srs: SrsStore;
	}
	let { srs }: Props = $props();

	const d = $derived(srs.setup);
	const lim = $derived(d.limits);
	const k = $derived(lim.kinematic);
	const f = $derived(d.feasibility);
	const locked = $derived(srs.locked);
	const E = $derived(srs.fieldErrors);
	const errs = (key: string) => E[key] ?? [];
	const rows = $derived(srs.rows.filter((r) => r.enabled && r.role !== 'drive'));
	const verdict = $derived(srs.check && srs.checkedKey === srs.key ? (srs.check.derived?.feasibility ?? null) : null);
	const peakG = $derived(srs.synthesis?.derived.reference.peak_g ?? null);
	const MASS: { key: keyof typeof f.moving_mass_lb; label: string }[] = [
		{ key: 'armature', label: 'Armature' },
		{ key: 'slip_table', label: 'Slip table' },
		{ key: 'drive_adapter', label: 'Drive adapter' },
		{ key: 'fixturing', label: 'Fixturing' },
		{ key: 'test_article', label: 'Test article' }
	];

	function setChannel(i: number, index: number) {
		const r = srs.rows.find((x) => x.index === index);
		lim.monitors[i].channel = index;
		if (r) lim.monitors[i].unit = r.units;
	}

	async function loadShaker(e: Event & { currentTarget: HTMLInputElement }) {
		const file = e.currentTarget.files?.[0];
		if (file) f.shaker_text = await file.text();
		e.currentTarget.value = '';
	}
</script>

<div class="cols">
	<div class="stack">
		<section class="aq-panel" aria-labelledby="rl-drive" data-testid="srs-drive-limits">
			<div class="aq-head"><h2 id="rl-drive">Drive</h2><span class="aq-sub">In volts peak at the amplifier's input.</span></div>
			<p class="aq-lead aq-small">There is no default for either: give the values for your amplifier. They hold for identification and for every pulse.</p>
			<div class="aq-fields">
				<NumberField id="rl-ceil" label="Drive ceiling" unit="V pk" step={0.1} min={0} bind:value={lim.drive_vpk_ceiling} errors={errs('limits.drive_vpk_ceiling')} disabled={locked} testid="srs-drive-ceiling" help="A drive over it is scaled down to it, and the run says so." />
				<NumberField id="rl-abort" label="Drive abort" unit="V pk" step={0.1} min={0} bind:value={lim.drive_vpk_abort} errors={errs('limits.drive_vpk_abort')} disabled={locked} testid="srs-drive-abort" help="Above the ceiling. A drive predicted over it is never played: the run aborts first." />
				<NumberField id="rl-acc" label="Peak acceleration ceiling" unit="g" nullable="None" fallback={peakG !== null ? Math.ceil(2 * peakG) : 200} step={1} min={0} bind:value={lim.peak_accel_g} errors={errs('limits.peak_accel_g')} disabled={locked} testid="srs-peak-accel" help="A predicted response peak over it scales the pulse down before it fires." />
				<NumberField id="rl-fac" label="Abort at, times a ceiling" step={0.5} min={1} bind:value={lim.abort_factor} errors={errs('limits.abort_factor')} disabled={locked} testid="srs-abort-factor" help="A predicted peak or a monitored peak at this times its ceiling aborts instead of scaling. Usual: 2." />
			</div>
		</section>

		<section class="aq-panel" aria-labelledby="rl-kin" data-testid="srs-kinematic">
			<div class="aq-head"><h2 id="rl-kin">Kinematic Ceilings</h2><span class="aq-sub">On the prediction before each pulse, and on the measured pulse after.</span></div>
			<div class="aq-fields">
				<NumberField id="rl-ka" label="Peak acceleration" unit="g" nullable="None" fallback={peakG !== null ? Math.ceil(1.5 * peakG) : 150} step={1} min={0} bind:value={k.accel_g} errors={errs('limits.kinematic.accel_g')} disabled={locked} testid="srs-kin-accel" />
				<NumberField id="rl-kv" label="Peak velocity" unit="in/s" nullable="None" fallback={50} step={1} min={0} bind:value={k.velocity_ips} errors={errs('limits.kinematic.velocity_ips')} disabled={locked} testid="srs-kin-velocity" />
				<NumberField id="rl-kd" label="Displacement, peak to peak" unit="in" nullable="None" fallback={1} step={0.05} min={0} bind:value={k.disp_pp_in} errors={errs('limits.kinematic.disp_pp_in')} disabled={locked} testid="srs-kin-disp" />
			</div>
			<p class="aq-small aq-muted">A pulse predicted past a ceiling never fires; a measured pulse past one aborts the run. The shaker's ratings below are checked as well.</p>
		</section>

		<section class="aq-panel" aria-labelledby="rl-mon" data-testid="srs-monitors">
			<div class="aq-head">
				<h2 id="rl-mon">Monitored Channels</h2>
				<span class="aq-sub">Amplifier current, a second accelerometer, anything with a ceiling.</span>
				<span class="aq-spacer"></span>
				<button class="btn sm" disabled={locked || !rows.length} onclick={() => lim.monitors.push(newMonitor(rows.find((r) => r.role === 'monitor') ?? rows[0]))} data-testid="srs-monitor-add">Add Channel</button>
			</div>
			{#if lim.monitors.length}
				<table class="aq-table">
					<thead><tr><th>Channel</th><th>Unit</th><th class="r">Peak ceiling</th><th>Checked</th><th aria-label="Remove"></th></tr></thead>
					<tbody>
						{#each lim.monitors as m, i (i)}
							<tr data-testid="srs-monitor-{i}">
								<td>
									<select class="field" aria-label="Monitor {i + 1} channel" value={m.channel} disabled={locked} onchange={(e) => setChannel(i, Number(e.currentTarget.value))}>
										{#if !rows.some((r) => r.index === m.channel)}<option value={m.channel}>Channel {m.channel}</option>{/if}
										{#each rows as r (r.index)}<option value={r.index}>{r.index}: {rowName(r)}</option>{/each}
									</select>
								</td>
								<td>{m.unit}</td>
								<td class="num"><NumberField id="rl-m-{i}" label="Monitor {i + 1} peak ceiling" compact unit={m.unit} step={1} min={0} bind:value={m.peak_ceiling} errors={errs(`limits.monitors.${i}.peak_ceiling`)} disabled={locked} /></td>
								<td>
									<select class="field" aria-label="Monitor {i + 1} check" bind:value={m.kind} disabled={locked}>
										<option value="between_pulse">After each pulse</option>
										<option value="predictive">Before each pulse, predicted</option>
									</select>
								</td>
								<td>
									<button class="aq-icon-btn" aria-label="Remove monitor {i + 1}" disabled={locked} onclick={() => lim.monitors.splice(i, 1)}>
										<svg viewBox="0 0 12 12" width="10" height="10" aria-hidden="true"><path d="M3 3l6 6M9 3l-6 6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
									</button>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
				<Findings items={Object.entries(E).filter(([key]) => key.startsWith('limits.monitors')).flatMap(([, v]) => v).map((message) => ({ level: 'error' as const, message }))} />
			{:else}
				<p class="aq-muted aq-small">None. A monitored channel past its ceiling scales the next pulse down; at the abort factor it aborts the run.</p>
			{/if}
		</section>
	</div>

	<section class="aq-panel" aria-labelledby="rl-shaker" data-testid="srs-feasibility">
		<div class="aq-head"><h2 id="rl-shaker">Shaker and Moving Mass</h2><span class="aq-sub">The synthesized pulse is checked against the shaker's ratings at arm.</span></div>
		<label class="aq-switch" for="rl-feas">
			<input id="rl-feas" type="checkbox" role="switch" checked={f.enabled} disabled={locked} onchange={(e) => (f.enabled = e.currentTarget.checked)} data-testid="srs-feasibility-on" />
			<span class="track" aria-hidden="true"><span class="thumb"></span></span>
			<span>Check the pulse against the shaker</span>
			<span class="word">{f.enabled ? 'On' : 'Off'}</span>
		</label>
		{#if f.enabled}
			<div class="aq-group">
				<h3>Shaker Record</h3>
				<textarea class="field mono rec" rows="6" bind:value={f.shaker_text} disabled={locked} aria-label="Shaker record" placeholder="Paste the shaker record, or load it from a file." data-testid="srs-shaker"></textarea>
				<label class="btn sm ghost file">Load From File<input type="file" accept=".json,application/json" disabled={locked} onchange={loadShaker} /></label>
				<Findings items={errs('feasibility.shaker').map((message) => ({ level: 'error' as const, message }))} />
			</div>
			<div class="aq-group">
				<h3>Moving Mass</h3>
				<div class="aq-fields">
					{#each MASS as m (m.key)}
						<NumberField id="rl-mass-{m.key}" label={m.label} unit="lb" step={1} min={0} bind:value={f.moving_mass_lb[m.key]} errors={errs(`feasibility.moving_mass_lb.${m.key}`)} disabled={locked} />
					{/each}
				</div>
			</div>
			<div class="aq-group">
				<h3>At Rest After the Pulse</h3>
				<div class="aq-fields">
					<NumberField id="rl-rv" label="Residual velocity" unit="in/s" step={0.05} min={0} bind:value={f.residual_velocity_ips} errors={errs('feasibility.residual_velocity_ips')} disabled={locked} help="The most velocity the synthesis may leave. Usual: 0.1 in/s." />
					<NumberField id="rl-rd" label="Residual displacement" unit="in" step={0.001} min={0} bind:value={f.residual_disp_in} errors={errs('feasibility.residual_disp_in')} disabled={locked} help="The most displacement the synthesis may leave. Usual: 0.001 in." />
				</div>
			</div>
		{:else}
			<p class="aq-muted aq-small">Off: the pulse is not checked against a shaker, and the run says so.</p>
		{/if}
		{#if verdict}
			<p class="aq-notice aq-small" class:warn={verdict.verdict === 'exceeded' || verdict.verdict === 'advisory'} data-testid="srs-feasibility-verdict">{verdict.message}</p>
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

	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
	}

	.r {
		text-align: right;
	}

	.mono {
		font-family: var(--font-mono);
	}

	.rec {
		width: 100%;
		resize: vertical;
	}

	.file {
		position: relative;
		margin-top: var(--sp-2);
		overflow: hidden;
	}

	.file input {
		position: absolute;
		inset: 0;
		opacity: 0;
		cursor: pointer;
	}

	@media (max-width: 1240px) {
		.cols {
			grid-template-columns: minmax(0, 1fr);
		}
	}
</style>
