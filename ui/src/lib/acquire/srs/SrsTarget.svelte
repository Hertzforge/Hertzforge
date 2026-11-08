<script lang="ts">
	/**
	 * The target and the band grid: the test's title, the target SRS (one level for every band, or
	 * breakpoints joined on log-log axes, typed here or read from a file on the computer that runs
	 * the test), and the grid every spectrum is on. Beside it the control service draws the target
	 * on its grid, with the alarm and abort bands, and the offline synthesis against it: a
	 * computation, never a measurement. The shell computes none of it.
	 */
	import ViewRenderer from '$lib/viz/ViewRenderer.svelte';
	import { formatNumber } from '$lib/viz/format';
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import PointsTable from './PointsTable.svelte';
	import { BANDS_PER_OCTAVE } from './srsSetup';
	import type { SrsStore } from './srsStore.svelte';

	interface Props {
		srs: SrsStore;
	}
	let { srs }: Props = $props();

	const d = $derived(srs.setup);
	const t = $derived(d.target);
	const b = $derived(d.bands);
	const locked = $derived(srs.locked);
	const E = $derived(srs.fieldErrors);
	const errs = (k: string) => E[k] ?? [];
	const synthesis = $derived(srs.synthesis);
	const views = $derived((synthesis?.views ?? []).filter((v) => v.view === 'spectrum' && /^(target|synthesis)$/i.test(v.title)));
	const bands = $derived(srs.bandsHz);
	let path = $state('');
	let reading = $state(false);
	const note = $derived(srs.imported?.what === 'target' ? srs.imported : null);

	async function read() {
		if (!path.trim()) return;
		reading = true;
		if (await srs.importTarget(path)) path = '';
		reading = false;
	}
</script>

<div class="cols">
	<div class="stack">
		<section class="aq-panel" aria-labelledby="rt-title">
			<div class="aq-head"><h2 id="rt-title">Target</h2><span class="aq-sub">The shock response spectrum the control point should see at full level.</span></div>
			<div class="aq-fields">
				<div class="sel wide">
					<label for="rt-name" class="lbl">Test title</label>
					<input id="rt-name" class="field" bind:value={d.title} disabled={locked} aria-invalid={errs('title').length > 0} data-testid="srs-title" />
					<Findings items={errs('title').map((message) => ({ level: 'error' as const, message }))} />
				</div>
				<div class="sel wide">
					<span class="lbl" id="rt-kind-l">Target</span>
					<div class="aq-seg" role="radiogroup" aria-labelledby="rt-kind-l">
						<button type="button" role="radio" aria-checked={t.kind === 'breakpoints'} disabled={locked} onclick={() => (t.kind = 'breakpoints')} data-testid="srs-target-breakpoints">Breakpoints</button>
						<button type="button" role="radio" aria-checked={t.kind === 'flat'} disabled={locked} onclick={() => (t.kind = 'flat')} data-testid="srs-target-flat">One level</button>
					</div>
				</div>
			</div>
			{#if t.kind === 'flat'}
				<div class="aq-fields spaced">
					<NumberField id="rt-flat" label="Every band" unit="g" step={10} min={0} bind:value={t.accel_g} errors={errs('target.accel_g')} disabled={locked} testid="srs-target-accel" help="The same peak acceleration in every band." />
				</div>
			{:else}
				<p class="aq-lead aq-small spaced">Points of the target, frequency rising, joined by straight lines on log-log axes. They cover the band grid below, from its first band to its last; the last band sits at or just above the grid's top.</p>
				<PointsTable
					rows={t.points}
					columns={[
						{ key: 'freq_hz', label: 'Frequency', unit: 'Hz', step: 10 },
						{ key: 'accel_g', label: 'SRS', unit: 'g', step: 10 }
					]}
					prefix="target.points"
					errors={E}
					{locked}
					min={2}
					make={(last) => ({ freq_hz: last ? Math.round(last.freq_hz * 2) : 100, accel_g: last?.accel_g ?? 100 })}
					testid="srs-target-points"
					label="Target breakpoints"
				/>
				<div class="aq-group">
					<h3>Read From a File</h3>
					<p class="aq-lead aq-small">Two columns, frequency in Hz and SRS in g, separated by tabs, commas or spaces; a header row and comment lines are allowed. Give the file's full path on the computer that runs the test.</p>
					<div class="row">
						<input class="field mono grow" aria-label="Target file path" placeholder="/full/path/to/target.csv" bind:value={path} disabled={locked} data-testid="srs-target-path" onkeydown={(e) => e.key === 'Enter' && void read()} />
						<button class="btn sm" disabled={locked || !path.trim() || reading || !srs.connected} onclick={read} data-testid="srs-target-read">Read Target</button>
					</div>
					{#if note}
						<Findings items={[...note.errors.map((e) => ({ level: 'error' as const, message: e.message })), ...note.warnings.map((e) => ({ level: 'warning' as const, message: e.message }))]} testid="srs-target-import-findings" />
						{#if !note.errors.length}<p class="aq-small aq-muted" data-testid="srs-target-imported">Read from {note.path}.</p>{/if}
					{/if}
				</div>
			{/if}
		</section>

		<section class="aq-panel" aria-labelledby="rt-grid">
			<div class="aq-head"><h2 id="rt-grid">Band Grid</h2><span class="aq-sub">Every spectrum of the test is on this grid.</span></div>
			<div class="aq-fields">
				<NumberField id="rt-lo" label="From" unit="Hz" step={10} min={0} bind:value={b.f_lo_hz} errors={errs('bands.f_lo_hz')} disabled={locked} testid="srs-band-lo" help="The first band's centre." />
				<NumberField id="rt-hi" label="To" unit="Hz" step={100} min={0} bind:value={b.f_hi_hz} errors={errs('bands.f_hi_hz')} disabled={locked} testid="srs-band-hi" help="The grid runs to the first centre at or above it, below half the sample rate." />
				<div class="sel">
					<label for="rt-n" class="lbl">Bands an octave</label>
					<select id="rt-n" class="field num" bind:value={b.bands_per_octave} disabled={locked} data-testid="srs-bands-per-octave">
						{#if !BANDS_PER_OCTAVE.includes(b.bands_per_octave)}<option value={b.bands_per_octave}>{b.bands_per_octave}</option>{/if}
						{#each BANDS_PER_OCTAVE as n (n)}<option value={n}>{n === 1 ? 'Whole octave' : `1/${n} octave`}</option>{/each}
					</select>
					<Findings items={errs('bands.bands_per_octave').map((message) => ({ level: 'error' as const, message }))} />
				</div>
				<NumberField id="rt-q" label="Quality factor" step={1} min={0} bind:value={b.q} errors={errs('bands.q')} disabled={locked} testid="srs-q" help="Every SRS of the test is computed at it. Usual: 10." />
			</div>
			{#if bands}
				<p class="aq-small derived" data-testid="srs-band-count">{bands.length} bands, {formatNumber(bands[0], 4)} to {formatNumber(bands[bands.length - 1], 4)} Hz.</p>
			{/if}
		</section>
	</div>

	<section class="aq-panel preview" aria-labelledby="rt-prev" data-testid="srs-target-preview" data-state={srs.previewBusy ? 'busy' : synthesis ? 'ok' : 'none'}>
		<div class="aq-head"><h2 id="rt-prev">The Target on Its Grid</h2><span class="aq-sub">A computation by the control service, never a measurement.</span></div>
		{#if synthesis}
			<div class="plots">
				{#each views as v, i (i)}
					<div class="plot" data-testid="srs-target-view-{i}"><ViewRenderer view={v} minHeight={220} /></div>
				{/each}
			</div>
			<p class="aq-small aq-muted">The second plot is the offline synthesis: the component table fitted to the target before any station. The run's control law refines it pulse by pulse.</p>
		{:else if srs.previewBusy}
			<p class="aq-muted aq-small">Synthesizing the target.</p>
		{:else if srs.previewError}
			<p class="aq-notice warn aq-small" data-testid="srs-preview-error">{srs.previewError}</p>
		{:else}
			<p class="aq-muted aq-small">{srs.connected ? 'Give the target and the band grid to draw them.' : 'The target is drawn by the control service once it is connected.'}</p>
		{/if}
	</section>
</div>

<style>
	.cols {
		display: grid;
		grid-template-columns: minmax(0, 1fr) minmax(0, 1.2fr);
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

	.sel.wide {
		grid-column: 1 / -1;
	}

	.lbl {
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}

	.spaced {
		margin-top: var(--sp-3);
	}

	.row {
		display: flex;
		gap: var(--sp-2);
		align-items: center;
	}

	.grow {
		flex: 1;
		min-width: 0;
	}

	.mono {
		font-family: var(--font-mono);
	}

	.derived {
		margin: var(--sp-3) 0 0;
		color: var(--text-2);
	}

	.plots {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}

	.plot {
		min-width: 0;
	}

	@media (max-width: 1240px) {
		.cols {
			grid-template-columns: minmax(0, 1fr);
		}
	}
</style>
