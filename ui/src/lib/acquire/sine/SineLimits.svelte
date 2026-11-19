<script lang="ts">
	/**
	 * What protects the shaker and the article: the drive ceiling and the drive abort in volts peak
	 * (no default: they are the amplifier's input, so the setup asks for both), the controlled stop's
	 * ramp as the service gives it (read only), the kinematic monitor, limit channels each with a
	 * bound in the profile's form and the notch that holds the drive down where one would pass its
	 * bound, a reference output locked to the tone, and the shaker and moving mass the profile is
	 * checked against.
	 */
	import { formatNumber } from '$lib/viz/format';
	import Findings from '../Findings.svelte';
	import { rowName } from '../mimo/mimoSetup';
	import NumberField from '../NumberField.svelte';
	import SineProfileTable from './SineProfileTable.svelte';
	import type { SineStore } from './sineStore.svelte';

	interface Props {
		sine: SineStore;
	}
	let { sine }: Props = $props();

	const d = $derived(sine.setup);
	const lim = $derived(d.limits);
	const cola = $derived(d.cola);
	const f = $derived(d.feasibility);
	const locked = $derived(sine.locked);
	const E = $derived(sine.fieldErrors);
	const W = $derived(sine.fieldWarnings);
	const errs = (key: string) => E[key] ?? [];
	const dry = $derived(sine.check && sine.checkedKey === sine.key ? sine.check.derived : null);
	const inputs = $derived(sine.rows.filter((r) => r.enabled && r.role !== 'drive' && r.role !== 'control'));
	// The simulator has one output: a reference output needs a station with two.
	const oneOutput = $derived((sine.acq.selected?.outputs ?? 1) < 2);
	let notchOpen = $state(false);
	const MASS: { key: keyof typeof f.moving_mass_lb; label: string }[] = [
		{ key: 'armature', label: 'Armature' },
		{ key: 'slip_table', label: 'Slip table' },
		{ key: 'drive_adapter', label: 'Drive adapter' },
		{ key: 'fixturing', label: 'Fixturing' },
		{ key: 'test_article', label: 'Test article' }
	];

	async function loadShaker(e: Event & { currentTarget: HTMLInputElement }) {
		const file = e.currentTarget.files?.[0];
		if (file) f.shaker_text = await file.text();
		e.currentTarget.value = '';
	}
</script>

<div class="cols">
	<div class="stack">
		<section class="aq-panel" aria-labelledby="nl-drive" data-testid="sine-drive-limits">
			<div class="aq-head"><h2 id="nl-drive">Drive</h2><span class="aq-sub">In volts peak at the amplifier's input.</span></div>
			<p class="aq-lead aq-small lead">There is no default for either: give the values for your amplifier. They hold for identification and for every tick of the run.</p>
			<div class="aq-fields">
				<NumberField id="nl-ceil" label="Drive ceiling" unit="V pk" step={0.1} min={0} max={10} bind:value={lim.drive_vpk_ceiling} errors={errs('limits.drive_vpk_ceiling')} disabled={locked} testid="sine-drive-ceiling" help="The drive is held at or under it, whatever the law asks." />
				<NumberField id="nl-abort" label="Drive abort" unit="V pk" step={0.1} min={0} max={10} bind:value={lim.drive_vpk_abort} errors={errs('limits.drive_vpk_abort')} disabled={locked} testid="sine-drive-abort" help="At least the ceiling. A drive at or over it aborts the run." />
			</div>
			<div class="aq-group sep">
				<h3>Controlled Stop</h3>
				{#if dry}
					<p class="aq-small" data-testid="sine-stop-ramp">The drive ramps down at <span class="num">{formatNumber(dry.stop_ramp_db_per_s, 4)} dB a second</span>; from the drive ceiling that takes <span class="num">{formatNumber(dry.safe_stop_s, 3)} s</span> of station time. The ramp is the service's own and is not set here.</p>
				{:else}
					<p class="aq-small aq-muted">The ramp of a controlled stop is shown once the setup checks.</p>
				{/if}
			</div>
			<label class="aq-switch tog" for="nl-kin">
				<input id="nl-kin" type="checkbox" role="switch" checked={lim.kinematic_monitor} disabled={locked} onchange={(e) => (lim.kinematic_monitor = e.currentTarget.checked)} data-testid="sine-kinematic-monitor" />
				<span class="track" aria-hidden="true"><span class="thumb"></span></span>
				<span>Stop when the response passes the shaker's velocity or displacement rating</span>
				<span class="word">{lim.kinematic_monitor ? 'On' : 'Off'}</span>
			</label>
			<p class="help">It needs the shaker below; without one the check is off and the run says so.</p>
		</section>

		<section class="aq-panel" aria-labelledby="nl-lim" data-testid="sine-limit-channels">
			<div class="aq-head">
				<h2 id="nl-lim">Limit Channels</h2>
				<span class="aq-sub">Inputs the drive is held down for.</span>
				<span class="aq-spacer"></span>
				<button class="btn sm" disabled={locked || !inputs.length} title={inputs.length ? 'Add a limit channel' : 'The channel table has no enabled input other than the control rows.'} onclick={() => sine.addLimitChannel()} data-testid="sine-limit-add">Add Channel</button>
			</div>
			<p class="aq-lead aq-small lead">Where a limit channel would pass its bound, the drive is held down there (a notch), and the control point may read under its profile. A limit channel outside its own abort band stops the run.</p>
			{#each lim.channels as ch, i (i)}
				<div class="aq-group lc" data-testid="sine-limit-{i}">
					<div class="lchead">
						<label class="lbl" for="nl-ch-{i}">Channel</label>
						<select id="nl-ch-{i}" class="field" value={ch.channel} disabled={locked} onchange={(e) => (ch.channel = Number(e.currentTarget.value))} data-testid="sine-limit-{i}-channel">
							{#if !inputs.some((r) => r.index === ch.channel)}<option value={ch.channel}>Channel {ch.channel}</option>{/if}
							{#each inputs as r (r.index)}<option value={r.index}>{r.index}: {rowName(r)}</option>{/each}
						</select>
						<label class="lbl" for="nl-sh-{i}">Shared bound</label>
						<input id="nl-sh-{i}" class="field" bind:value={ch.shared_name} disabled={locked} placeholder="Its own" data-testid="sine-limit-{i}-shared" />
						<button class="aq-icon-btn" aria-label="Remove limit channel {i + 1}" disabled={locked} onclick={() => lim.channels.splice(i, 1)}>
							<svg viewBox="0 0 12 12" width="10" height="10" aria-hidden="true"><path d="M3 3l6 6M9 3l-6 6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
						</button>
					</div>
					<Findings items={errs(`limits.channels.${i}.channel`).map((message) => ({ level: 'error' as const, message }))} />
					<SineProfileTable rows={ch.profile.rows} prefix="limits.channels.{i}.profile.rows" errors={{ ...E, [`limits.channels.${i}.profile.rows`]: [...errs(`limits.channels.${i}.profile.rows`), ...errs(`limits.channels.${i}.profile`)] }} warnings={W} {locked} testid="sine-limit-{i}-rows" label="Limit channel {i + 1} bound" valueWord="Bound" bands={false} file={{ client: sine.acq.tableFiles, tableKind: 'sine_limit', name: 'Sine Limit' }} />
				</div>
			{:else}
				<p class="aq-muted aq-small">None.</p>
			{/each}
			<Findings items={errs('limits.channels').map((message) => ({ level: 'error' as const, message }))} />
			{#if lim.channels.length}
				<div class="aq-group sep">
					<h3>Notch <button class="btn ghost sm" aria-expanded={notchOpen} onclick={() => (notchOpen = !notchOpen)} data-testid="sine-notch-open">{notchOpen ? 'Hide' : 'Show'}</button></h3>
					{#if notchOpen}
						<div class="aq-fields">
							<NumberField id="nl-nf" label="Drive under which a gain is not estimated, share of full" step={0.005} min={0} max={1} bind:value={lim.notch.drive_floor} errors={errs('limits.notch.drive_floor')} disabled={locked} help="Usual: 0.01." />
							<NumberField id="nl-ns" label="Ticks the estimate is smoothed over" integer step={1} min={1} bind:value={lim.notch.smoothing_ticks} errors={errs('limits.notch.smoothing_ticks')} disabled={locked} help="Usual: 3." />
							<NumberField id="nl-nr" label="Comes back at most" unit="dB/s" step={5} min={0} bind:value={lim.notch.release_ramp_db_per_s} errors={errs('limits.notch.release_ramp_db_per_s')} disabled={locked} help="How fast the drive may return once the notch releases. Usual: 20 dB/s." />
						</div>
					{/if}
				</div>
			{/if}
		</section>
	</div>

	<div class="stack">
		<section class="aq-panel" aria-labelledby="nl-cola" data-testid="sine-cola">
			<div class="aq-head"><h2 id="nl-cola">Reference Output</h2><span class="aq-sub">A constant-amplitude copy of the tone on a second output.</span></div>
			<label class="aq-switch" for="nl-cola-on" class:off={oneOutput}>
				<input id="nl-cola-on" type="checkbox" role="switch" checked={cola.enabled} disabled={locked || (oneOutput && !cola.enabled)} onchange={(e) => (cola.enabled = e.currentTarget.checked)} data-testid="sine-cola-on" />
				<span class="track" aria-hidden="true"><span class="thumb"></span></span>
				<span>Play a reference output locked to the tone</span>
				<span class="word">{cola.enabled ? 'On' : 'Off'}</span>
			</label>
			{#if oneOutput}<p class="help" data-testid="sine-cola-sim">{sine.acq.selected?.kind === 'sim' ? 'The simulator has one output, so it plays no reference output.' : 'This station has one output.'} A reference output needs a station with two outputs and a second drive row.</p>{/if}
			{#if cola.enabled}
				<div class="aq-fields spaced">
					<NumberField id="nl-cv" label="Amplitude" unit="V pk" step={0.05} min={0} max={10} bind:value={cola.volts_pk} errors={errs('cola.volts_pk')} disabled={locked} testid="sine-cola-volts" />
					<div class="sel">
						<label for="nl-cw" class="lbl">Waveform</label>
						<select id="nl-cw" class="field" bind:value={cola.waveform} disabled={locked}>
							<option value="sine">Sine</option>
							<option value="square">Square</option>
							<option value="dc">Steady level</option>
						</select>
					</div>
				</div>
				<p class="help">It ramps down with the drive at a stop.</p>
			{/if}
			<Findings items={[...errs('cola.enabled'), ...errs('cola')].map((message) => ({ level: 'error' as const, message }))} testid="sine-cola-findings" />
		</section>

		<section class="aq-panel" aria-labelledby="nl-shaker" data-testid="sine-feasibility">
			<div class="aq-head"><h2 id="nl-shaker">Shaker and Moving Mass</h2><span class="aq-sub">The profile is checked against the shaker's ratings.</span></div>
			<label class="aq-switch" for="nl-feas">
				<input id="nl-feas" type="checkbox" role="switch" checked={f.enabled} disabled={locked} onchange={(e) => (f.enabled = e.currentTarget.checked)} data-testid="sine-feasibility-on" />
				<span class="track" aria-hidden="true"><span class="thumb"></span></span>
				<span>Check the profile against the shaker</span>
				<span class="word">{f.enabled ? 'On' : 'Off'}</span>
			</label>
			{#if f.enabled}
				<div class="aq-group">
					<h3>Shaker Record</h3>
					<textarea class="field mono rec" rows="6" bind:value={f.shaker_text} disabled={locked} aria-label="Shaker record" placeholder="Paste the shaker record, or load it from a file." data-testid="sine-shaker"></textarea>
					<label class="btn sm ghost file">Load From File<input type="file" accept=".json,application/json" disabled={locked} onchange={loadShaker} /></label>
					<Findings items={errs('feasibility.shaker').map((message) => ({ level: 'error' as const, message }))} />
				</div>
				<div class="aq-group">
					<h3>Moving Mass</h3>
					<div class="aq-fields">
						{#each MASS as m (m.key)}
							<NumberField id="nl-mass-{m.key}" label={m.label} unit="lb" step={1} min={0} bind:value={f.moving_mass_lb[m.key]} errors={errs(`feasibility.moving_mass_lb.${m.key}`)} disabled={locked} />
						{/each}
					</div>
				</div>
			{:else}
				<p class="aq-muted aq-small">Off: the profile is not checked against a shaker, the kinematic monitor is off, and the run says so.</p>
			{/if}
			{#if dry}
				<p class="aq-notice aq-small" class:warn={dry.feasibility.verdict === 'exceeded' || dry.feasibility.verdict === 'advisory'} data-testid="sine-feasibility-verdict">{dry.feasibility.message}</p>
			{/if}
		</section>
	</div>
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

	.help {
		margin: 2px 0 0;
		font-size: var(--text-xs);
		color: var(--text-3);
	}

	.tog,
	.spaced {
		margin-top: var(--sp-3);
	}

	.off {
		opacity: 0.6;
	}

	.lc {
		padding-top: var(--sp-2);
		border-top: 1px solid var(--line-1);
	}

	.lchead {
		display: grid;
		grid-template-columns: auto minmax(0, 1fr) auto minmax(0, 1fr) auto;
		align-items: center;
		gap: var(--sp-2);
		margin-bottom: var(--sp-2);
	}

	h3 {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
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
	/* A group after fields or a table is set off as one after another group is. */
	.sep {
		margin-top: var(--sp-5);
		padding-top: var(--sp-4);
		border-top: 1px solid var(--line-1);
	}

	.lead {
		margin: 0 0 var(--sp-3);
	}
</style>
