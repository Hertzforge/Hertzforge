<script lang="ts">
	/**
	 * What the setup asks of the shaker before anything is armed: rms acceleration, velocity and
	 * displacement of the reference at full level, the band, and the analysis grid. Every number
	 * is the service's: the ones it holds with its setup while the editor matches it (so a
	 * reloaded or second screen shows what the run uses), else its dry run of the draft. The shell
	 * only shows them. Once a run arms, the grid the station actually runs at replaces the
	 * requested one.
	 */
	import { formatFixed, formatNumber } from '$lib/viz/format';
	import NumberField from './NumberField.svelte';
	import { N_PTS_OPTIONS } from './setup';
	import type { AcquireState } from './store.svelte';
	import { STATE_WORDS } from './words';

	interface Props {
		acq: AcquireState;
	}
	let { acq }: Props = $props();

	const d = $derived(acq.derived);
	const run = $derived(acq.run);
	const actual = $derived(run?.grid?.actual && acq.setupStatus !== 'changed' ? run.grid : null);
	const grid = $derived(actual ?? d?.grid ?? null);
	const a = $derived(acq.setup.acquisition);
	const E = $derived(acq.fieldErrors);
	const errors = $derived(acq.setupErrorCount);
	const status = $derived(acq.setupStatus);
	const feas = $derived(run?.feasibility ?? null);

	const VERDICT: Record<string, string> = { pass: 'Within the shaker ratings', advisory: 'Advisory', exceeded: 'Exceeds the shaker ratings', not_evaluated: 'Not checked' };
</script>

<section class="aq-panel summary" aria-labelledby="summary-title" data-testid="setup-summary">
	<div class="aq-head">
		<h2 id="summary-title">At Full Level</h2>
		<span class="aq-spacer"></span>
		<span class="check" data-testid="setup-check" aria-live="polite">
			{#if acq.checking}
				Checking
			{:else if errors > 0}
				<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><path d="M4.5 1h5L13 4.5v5L9.5 13h-5L1 9.5v-5z" fill="var(--amber-2)" /></svg>
				{errors} {errors === 1 ? 'error' : 'errors'} to fix
			{:else if d}
				<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><circle cx="7" cy="7" r="6" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M4.2 7.2l1.9 1.9 3.8-4" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" /></svg>
				Checked by the control service
			{/if}
		</span>
	</div>

	<div class="big">
		<div>
			<span class="v num" data-testid="derived-grms">{d ? formatFixed(d.grms_g, 3) : '-'}</span>
			<span class="u">g rms</span>
			<span class="l">Acceleration</span>
		</div>
		<div>
			<span class="v num" data-testid="derived-velocity">{d ? formatFixed(d.velocity_rms_ips, 3) : '-'}</span>
			<span class="u">in/s rms</span>
			<span class="l">Velocity</span>
		</div>
		<div>
			<span class="v num" data-testid="derived-displacement">{d ? formatFixed(d.displacement_rms_mil, 3) : '-'}</span>
			<span class="u">mil rms</span>
			<span class="l">Displacement</span>
		</div>
	</div>
	{#if !d && errors > 0}
		<p class="aq-muted aq-small">The numbers come back once the setup and the channel table have no errors.</p>
	{/if}

	<dl class="facts">
		<div><dt>Band</dt><dd class="num">{d ? `${formatNumber(d.band_hz[0])} to ${formatNumber(d.band_hz[1])} Hz` : '-'}</dd></div>
		<div><dt>Control channels</dt><dd class="num">{d ? d.control_channels.join(', ') || '-' : '-'}</dd></div>
		<div>
			<dt>{actual ? 'Station rate' : 'Rate asked'}</dt>
			<dd class="num">{grid ? `${formatNumber(grid.fs_hz, 6)} Hz` : '-'}</dd>
		</div>
		<div><dt>Line spacing</dt><dd class="num">{grid ? `${formatNumber(grid.df_hz, 4)} Hz` : '-'}</dd></div>
		<div><dt>Lines in band</dt><dd class="num">{grid ? grid.lines : '-'}</dd></div>
		<div><dt>Control loop</dt><dd class="num">{grid ? `${formatNumber(grid.loop_s, 3)} s` : '-'}</dd></div>
	</dl>
	<p class="gridnote aq-small" data-testid="grid-note">
		{#if actual}
			<span class="tag actual">Actual</span> The run armed: these are the rate and grid the station runs at.
		{:else}
			<span class="tag">Requested</span> The station may run at a nearby rate; the run reports the grid it uses when it arms.
		{/if}
	</p>

	<div class="aq-group">
		<h3>Acquisition</h3>
		<div class="aq-fields acq">
			<NumberField id="acq-fs" label="Sample rate" unit="Hz" bind:value={a.fs_hz} errors={E['acquisition.fs_hz'] ?? []} disabled={acq.locked} testid="acq-fs" />
			<div class="sel">
				<label for="acq-npts" class="lbl">FFT length</label>
				<select id="acq-npts" class="field num" bind:value={a.n_pts} disabled={acq.locked} aria-invalid={(E['acquisition.n_pts'] ?? []).length > 0}>
					{#each N_PTS_OPTIONS as n (n)}<option value={n}>{n} points</option>{/each}
				</select>
				{#each E['acquisition.n_pts'] ?? [] as m (m)}<p class="err">{m}</p>{/each}
			</div>
			<NumberField id="acq-navg" label="Averages per loop" integer min={1} step={1} bind:value={a.n_avg} errors={E['acquisition.n_avg'] ?? []} disabled={acq.locked} />
		</div>
	</div>

	<div class="aq-group status-group">
		<h3>Setup on the Service</h3>
		<p class="aq-small" data-testid="setup-status" data-status={status}>
			{#if status === 'none'}
				Not applied yet. Apply Setup sends it; the service checks it again and holds it for the run.
			{:else if status === 'applied'}
				Applied{run?.run_id ? ` to run ${run.run_id}` : ''}: {run ? STATE_WORDS[run.state].toLowerCase() : ''}.
			{:else if status === 'changed'}
				Changed on this screen since the service took it{acq.held?.config.title ? ` ("${acq.held.config.title}")` : ''}. Apply it again to run it, or put the service's setup back.
				<button class="btn sm ghost" onclick={() => acq.revertSetup()} disabled={acq.locked} data-testid="setup-put-back">Put Back</button>
			{:else}
				The service holds a setup{run?.title ? ` ("${run.title}")` : ''} this screen has not read back yet. Arm and Start run that one.
			{/if}
		</p>
		{#if feas}
			<p class="aq-small feas" data-testid="feasibility" data-verdict={feas.verdict}>
				<span class="tag" class:warn={feas.verdict === 'exceeded'}>Feasibility: {VERDICT[feas.verdict] ?? feas.verdict}</span>
				{feas.message}
			</p>
		{/if}
	</div>
</section>

<style>
	.check {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	.big {
		display: grid;
		grid-template-columns: repeat(3, minmax(0, 1fr));
		gap: var(--sp-2);
	}

	.big > div {
		display: grid;
		grid-template-columns: auto 1fr;
		align-items: baseline;
		column-gap: 5px;
		padding: 10px 12px;
		background: var(--bg-2);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-md);
		min-width: 0;
	}

	.big .v {
		font-size: var(--text-xl);
		color: var(--text-1);
	}

	.big .u {
		font-size: var(--text-xs);
		color: var(--text-3);
		white-space: nowrap;
	}

	.big .l {
		grid-column: 1 / -1;
		font-size: var(--text-2xs);
		color: var(--text-3);
		text-transform: uppercase;
		letter-spacing: 0.06em;
	}

	.facts {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
		gap: 6px var(--sp-4);
		margin: var(--sp-3) 0 0;
	}

	.facts div {
		display: flex;
		flex-direction: column;
	}

	.facts dt {
		font-size: var(--text-2xs);
		color: var(--text-3);
	}

	.facts dd {
		margin: 0;
		font-size: var(--text-sm);
	}

	.gridnote {
		display: flex;
		align-items: center;
		gap: 6px;
		margin-top: var(--sp-2);
		color: var(--text-3);
	}

	.tag {
		flex: none;
		padding: 0 6px;
		font-size: var(--text-2xs);
		font-weight: var(--weight-strong);
		letter-spacing: 0.04em;
		text-transform: uppercase;
		color: var(--text-2);
		border: 1px dashed var(--line-3);
		border-radius: 3px;
	}

	.tag.actual {
		color: var(--blue-1);
		border: 1px solid var(--blue-line);
	}

	.tag.warn {
		color: var(--amber-1);
		border: 1px solid var(--amber-2);
	}

	.summary .aq-group {
		margin-top: var(--sp-4);
		padding-top: var(--sp-4);
		border-top: 1px solid var(--line-1);
	}

	.acq {
		grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
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

	.err {
		font-size: var(--text-xs);
		color: var(--amber-1);
	}

	.status-group p {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 6px;
		color: var(--text-2);
	}

	.feas {
		margin-top: var(--sp-2);
	}

	@media (max-width: 520px) {
		.big {
			grid-template-columns: 1fr;
		}
	}
</style>
