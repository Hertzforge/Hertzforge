<script lang="ts">
	/**
	 * How the pulse is built from the target: the synthesis engine, the component table it starts
	 * from (one component per band, or a table typed here or read from a file), the offline fit of
	 * that table to the target before any station, and how each pulse is recorded. Beside it the
	 * control service draws the synthesized pulse at full level with its velocity and displacement,
	 * and says what the fit reached: a computation, never a measurement.
	 */
	import ViewRenderer from '$lib/viz/ViewRenderer.svelte';
	import { formatNumber } from '$lib/viz/format';
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import { ENGINES, newDampedSine, newWavelet } from './srsSetup';
	import type { SrsStore } from './srsStore.svelte';

	interface Props {
		srs: SrsStore;
	}
	let { srs }: Props = $props();

	const d = $derived(srs.setup);
	const s = $derived(d.synthesis);
	const a = $derived(d.acquisition);
	const locked = $derived(srs.locked);
	const E = $derived(srs.fieldErrors);
	const W = $derived(srs.fieldWarnings);
	const errs = (k: string) => E[k] ?? [];
	const warns = (k: string) => W[k] ?? [];
	const synthesis = $derived(srs.synthesis);
	const facts = $derived(synthesis?.derived ?? null);
	const views = $derived((synthesis?.views ?? []).filter((v) => v.view === 'spectrum' && !/^(target|synthesis)$/i.test(v.title)));
	const tableErrors = $derived(Object.entries(E).filter(([k]) => k === 'synthesis.components').flatMap(([, v]) => v));
	let path = $state('');
	let reading = $state(false);
	const note = $derived(srs.imported?.what === 'table' ? srs.imported : null);

	async function read() {
		if (!path.trim()) return;
		reading = true;
		if (await srs.importTable(path)) path = '';
		reading = false;
	}

	function addRow() {
		const f = srs.bandsHz?.[0] ?? 100;
		if (s.engine === 'wavsyn') s.wavelets.push(newWavelet(s.wavelets.at(-1) ? s.wavelets.at(-1)!.freq_hz * 2 : f));
		else s.sines.push(newDampedSine(s.sines.at(-1) ? s.sines.at(-1)!.freq_hz * 2 : f));
	}

	const cell = (i: number, k: string) => errs(`synthesis.components.${i}.${k}`);
</script>

<div class="cols">
	<div class="stack">
		<section class="aq-panel" aria-labelledby="ry-eng">
			<div class="aq-head"><h2 id="ry-eng">Synthesis</h2><span class="aq-sub">How each pulse is built to meet the target.</span></div>
			<div class="aq-seg" role="radiogroup" aria-label="Engine">
				{#each ENGINES as e (e.id)}
					<button type="button" role="radio" aria-checked={s.engine === e.id} disabled={locked} onclick={() => (s.engine = e.id)} data-testid="srs-engine-{e.id}">{e.label}</button>
				{/each}
			</div>
			<p class="aq-small aq-muted">{ENGINES.find((e) => e.id === s.engine)?.help}</p>

			<div class="aq-group">
				<h3>Components</h3>
				<div class="aq-seg" role="radiogroup" aria-label="Components to start from">
					<button type="button" role="radio" aria-checked={!s.table} disabled={locked} onclick={() => (s.table = false)} data-testid="srs-components-per-band">One per band</button>
					<button type="button" role="radio" aria-checked={s.table} disabled={locked} onclick={() => (s.table = true)} data-testid="srs-components-table">From a table</button>
				</div>
				{#if !s.table}
					<p class="aq-small aq-muted">The control service places one component at each band's centre and fits their sizes to the target.</p>
				{:else}
					<table class="aq-table comps" data-testid="srs-component-table">
						<thead>
							<tr>
								<th class="r">Frequency, Hz</th>
								<th class="r">Amplitude, g</th>
								{#if s.engine === 'wavsyn'}<th class="r">Half cycles</th>{:else}<th class="r">Damping ratio</th>{/if}
								<th class="r">Delay, s</th>
								{#if s.engine === 'seds'}<th>Polarity</th>{/if}
								<th aria-label="Remove"></th>
							</tr>
						</thead>
						<tbody>
							{#if s.engine === 'wavsyn'}
								{#each s.wavelets as r, i (i)}
									<tr data-testid="srs-component-{i}">
										<td class="num"><NumberField id="ry-w{i}-f" label="Component {i + 1}, frequency" compact step={10} bind:value={r.freq_hz} errors={cell(i, 'freq_hz')} disabled={locked} /></td>
										<td class="num"><NumberField id="ry-w{i}-a" label="Component {i + 1}, amplitude" compact step={1} bind:value={r.amplitude_g} errors={cell(i, 'amplitude_g')} disabled={locked} /></td>
										<td class="num"><NumberField id="ry-w{i}-n" label="Component {i + 1}, half cycles" compact integer step={2} min={3} bind:value={r.half_cycles} errors={cell(i, 'half_cycles')} disabled={locked} /></td>
										<td class="num"><NumberField id="ry-w{i}-d" label="Component {i + 1}, delay" compact step={0.001} min={0} bind:value={r.delay_s} errors={cell(i, 'delay_s')} disabled={locked} /></td>
										<td><button class="aq-icon-btn" aria-label="Remove component {i + 1}" disabled={locked} onclick={() => s.wavelets.splice(i, 1)}><svg viewBox="0 0 12 12" width="10" height="10" aria-hidden="true"><path d="M3 3l6 6M9 3l-6 6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg></button></td>
									</tr>
								{/each}
							{:else}
								{#each s.sines as r, i (i)}
									<tr data-testid="srs-component-{i}">
										<td class="num"><NumberField id="ry-s{i}-f" label="Component {i + 1}, frequency" compact step={10} bind:value={r.freq_hz} errors={cell(i, 'freq_hz')} disabled={locked} /></td>
										<td class="num"><NumberField id="ry-s{i}-a" label="Component {i + 1}, amplitude" compact step={1} bind:value={r.amplitude_g} errors={cell(i, 'amplitude_g')} disabled={locked} /></td>
										<td class="num"><NumberField id="ry-s{i}-z" label="Component {i + 1}, damping ratio" compact step={0.01} min={0} bind:value={r.zeta} errors={cell(i, 'zeta')} disabled={locked} /></td>
										<td class="num"><NumberField id="ry-s{i}-d" label="Component {i + 1}, delay" compact step={0.001} min={0} bind:value={r.delay_s} errors={cell(i, 'delay_s')} disabled={locked} /></td>
										<td>
											<select class="field" aria-label="Component {i + 1}, polarity" bind:value={r.polarity} disabled={locked}>
												<option value="positive">Positive</option>
												<option value="negative">Negative</option>
											</select>
										</td>
										<td><button class="aq-icon-btn" aria-label="Remove component {i + 1}" disabled={locked} onclick={() => s.sines.splice(i, 1)}><svg viewBox="0 0 12 12" width="10" height="10" aria-hidden="true"><path d="M3 3l6 6M9 3l-6 6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg></button></td>
									</tr>
								{/each}
							{/if}
						</tbody>
					</table>
					<button class="btn sm ghost spaced" disabled={locked} onclick={addRow} data-testid="srs-component-add">Add Component</button>
					<Findings items={tableErrors.map((message) => ({ level: 'error' as const, message }))} />
					<div class="aq-group">
						<h3>Read From a File</h3>
						<p class="aq-lead aq-small">A table for {s.engine === 'wavsyn' ? 'wavelets' : 'damped sines'}, its columns named in a header row ({s.engine === 'wavsyn' ? 'freq_hz, amplitude_g, half_cycles, delay_s' : 'freq_hz, amplitude_g, zeta, delay_s, polarity'}). Give the file's full path on the computer that runs the test.</p>
						<div class="row">
							<input class="field mono grow" aria-label="Component table file path" placeholder="/full/path/to/components.csv" bind:value={path} disabled={locked} data-testid="srs-table-path" onkeydown={(e) => e.key === 'Enter' && void read()} />
							<button class="btn sm" disabled={locked || !path.trim() || reading || !srs.connected} onclick={read} data-testid="srs-table-read">Read Table</button>
						</div>
						{#if note}
							<Findings items={[...note.errors.map((e) => ({ level: 'error' as const, message: e.message })), ...note.warnings.map((e) => ({ level: 'warning' as const, message: e.message }))]} testid="srs-table-import-findings" />
							{#if !note.errors.length}<p class="aq-small aq-muted" data-testid="srs-table-imported">Read from {note.path}.</p>{/if}
						{/if}
					</div>
				{/if}
			</div>

			<div class="aq-group">
				<h3>Offline Fit</h3>
				<p class="aq-lead">Before any station, the control service fits the components to the target. This does not set the run's control law; the law refines from where the fit ends.</p>
				<div class="aq-fields">
					<NumberField id="ry-it" label="Passes at most" integer step={10} min={1} bind:value={s.max_iters} errors={errs('synthesis.max_iters')} disabled={locked} testid="srs-max-iters" help="Usual: 50." />
					<NumberField id="ry-tol" label="Done within" unit="dB" step={0.5} min={0} bind:value={s.tol_db} errors={errs('synthesis.tol_db')} warnings={warns('synthesis.tol_db')} disabled={locked} testid="srs-tol" help="Every band this close to the target. Usual: 1.5 dB." />
					<NumberField id="ry-gain" label="Largest move a pass" unit="dB" step={0.5} min={0} bind:value={s.max_gain_db_per_iter} errors={errs('synthesis.max_gain_db_per_iter')} disabled={locked} testid="srs-max-gain" help="Usual: 3 dB." />
				</div>
			</div>
		</section>

		<section class="aq-panel" aria-labelledby="ry-acq">
			<div class="aq-head"><h2 id="ry-acq">Recording Each Pulse</h2></div>
			<div class="aq-fields">
				<NumberField id="ry-fs" label="Sample rate" unit="Hz" step={1024} min={0} bind:value={a.fs_hz} errors={errs('acquisition.fs_hz')} disabled={locked} testid="srs-fs" help="Asked of the station; the run reports the rate it ran at." />
				<NumberField id="ry-ring" label="Ring-out" unit="s" step={0.01} min={0} bind:value={a.ring_out_s} errors={errs('acquisition.ring_out_s')} disabled={locked} testid="srs-ring-out" help="Measured after each pulse's drive ends. Usual: 0.05 s." />
			</div>
			<label class="aq-switch spaced" for="ry-pvss">
				<input id="ry-pvss" type="checkbox" role="switch" checked={a.pvss} disabled={locked} onchange={(e) => (a.pvss = e.currentTarget.checked)} data-testid="srs-pvss" />
				<span class="track" aria-hidden="true"><span class="thumb"></span></span>
				<span>Also compute each pulse's pseudo-velocity SRS</span>
				<span class="word">{a.pvss ? 'On' : 'Off'}</span>
			</label>
		</section>
	</div>

	<section class="aq-panel preview" aria-labelledby="ry-prev" data-testid="srs-synthesis-preview" data-state={srs.previewBusy ? 'busy' : synthesis ? 'ok' : 'none'}>
		<div class="aq-head"><h2 id="ry-prev">The Synthesized Pulse at Full Level</h2><span class="aq-sub">A computation by the control service, never a measurement.</span></div>
		{#if facts}
			<dl class="facts" data-testid="srs-synthesis-facts">
				<div><dt>Components</dt><dd class="num">{facts.synthesis.components}</dd></div>
				<div class:warn={!facts.synthesis.converged}>
					<dt>Fit</dt>
					<dd class="num" data-testid="srs-synthesis-converged" data-converged={facts.synthesis.converged}>
						{#if !facts.synthesis.converged}<svg viewBox="0 0 14 14" width="11" height="11" aria-hidden="true"><path d="M7 1.6l5.6 10H1.4z" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round" /></svg>{/if}
						{facts.synthesis.converged ? 'Within' : 'Not within'} {formatNumber(s.tol_db, 3)} dB after {facts.synthesis.iters} {facts.synthesis.iters === 1 ? 'pass' : 'passes'}{facts.synthesis.worst_err_db === null ? '' : `; worst band ${formatNumber(facts.synthesis.worst_err_db, 3)} dB`}
					</dd>
				</div>
				<div><dt>Pulse length</dt><dd class="num">{formatNumber(facts.synthesis.duration_s * 1000, 4)} ms</dd></div>
				<div><dt>Peak</dt><dd class="num">{formatNumber(facts.reference.peak_g, 4)} g</dd></div>
				<div><dt>Peak velocity</dt><dd class="num">{formatNumber(facts.reference.velocity_pk_ips, 4)} in/s</dd></div>
				<div><dt>Displacement, peak to peak</dt><dd class="num">{formatNumber(facts.reference.displacement_pk_pk_in, 4)} in</dd></div>
				<div><dt>Left at the end</dt><dd class="num">{formatNumber(facts.reference.residual_velocity_ips, 3)} in/s, {formatNumber(facts.reference.residual_disp_in, 3)} in</dd></div>
			</dl>
			{#if !facts.synthesis.converged}
				<p class="aq-notice warn aq-small" role="alert" data-testid="srs-synthesis-note">The offline fit did not reach the target in every band. The test may still run; the control law refines from here.</p>
			{/if}
			<div class="plots">
				{#each views as v, i (i)}
					<div class="plot" data-testid="srs-synthesis-view-{i}"><ViewRenderer view={v} minHeight={i === 0 ? 200 : 140} /></div>
				{/each}
			</div>
		{:else if srs.previewBusy}
			<p class="aq-muted aq-small">Synthesizing the pulse.</p>
		{:else if srs.previewError}
			<p class="aq-notice warn aq-small">{srs.previewError}</p>
		{:else}
			<p class="aq-muted aq-small">{srs.connected ? 'Give the target, the band grid and the components to draw the pulse.' : 'The pulse is drawn by the control service once it is connected.'}</p>
		{/if}
	</section>
</div>

<style>
	.cols {
		display: grid;
		grid-template-columns: minmax(0, 1fr) minmax(0, 1.1fr);
		gap: var(--sp-3);
		align-items: start;
	}

	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
	}

	.comps {
		margin-top: var(--sp-2);
	}

	.r {
		text-align: right;
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

	.facts {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: 6px var(--sp-3);
		margin: 0 0 var(--sp-2);
	}

	.facts dt {
		font-size: var(--text-2xs);
		color: var(--text-3);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	.facts dd {
		margin: 0;
		font-size: var(--text-sm);
	}

	.facts .warn dd {
		font-weight: var(--weight-strong);
		color: var(--amber-1);
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
