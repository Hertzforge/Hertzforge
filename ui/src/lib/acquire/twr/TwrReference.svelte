<script lang="ts">
	/**
	 * The time history to replicate: read by the control service from a file on the computer that
	 * runs the test, by its full path, or from pasted text; the service keeps the raw record under its
	 * SHA-256 and the setup names it by that. Then how the service prepares it (resampling, detrend,
	 * drift high-pass, scale, end taper), the section to replicate, the quiet pads either side and the
	 * rate. Beside it the service's preparation as it would run: the record every play is played as,
	 * the section's peak and rms, how much of its energy the correction can reach, its kinematics and
	 * four views. The shell repeats none of that arithmetic.
	 */
	import ViewRenderer from '$lib/viz/ViewRenderer.svelte';
	import { formatNumber } from '$lib/viz/format';
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import TwrKinematics from './TwrKinematics.svelte';
	import { DETRENDS } from './twrSetup';
	import type { TwrStore } from './twrStore.svelte';

	interface Props {
		twr: TwrStore;
	}
	let { twr }: Props = $props();

	const d = $derived(twr.setup);
	const locked = $derived(twr.locked);
	const E = $derived(twr.fieldErrors);
	const W = $derived(twr.fieldWarnings);
	const errs = (k: string) => E[k] ?? [];
	const warns = (k: string) => W[k] ?? [];
	const facts = $derived(d.facts && d.facts.sha256 === d.reference.sha256 ? d.facts : null);
	const prepared = $derived(twr.prepared);
	const refViews = $derived(twr.imported && facts && twr.imported.facts.sha256 === facts.sha256 ? twr.imported.views : []);
	const previewState = $derived(!d.reference.sha256 ? 'none' : prepared ? 'ok' : twr.previewBusy ? 'busy' : twr.previewError ? 'error' : 'busy');

	let from = $state<'path' | 'text'>('path');
	let path = $state('');
	let text = $state('');
	let rate = $state<number | null>(null);
	let column = $state<number | null>(null);
	let units = $state('');
	let name = $state('');
	const reading = $derived(twr.busy === 'Read Reference');

	async function readIt() {
		if (locked || reading) return;
		const src = from === 'path' ? { path } : { text };
		if (from === 'path' ? !path.trim() : !text.trim()) return;
		const ok = await twr.importReference(src, { fs_hz: rate, column, units, name });
		if (ok && from === 'text') text = '';
	}

	const n = (v: number, sig = 4) => formatNumber(v, sig);
	const shortHash = (h: string) => `${h.slice(0, 12)}...${h.slice(-6)}`;
</script>

<div class="cols">
	<div class="stack">
		<section class="aq-panel" aria-labelledby="tr-ref" data-testid="twr-reference">
			<div class="aq-head"><h2 id="tr-ref">Reference</h2><span class="aq-sub">The recorded time history the run reproduces at the control point.</span></div>
			<div class="aq-fields one">
				<div class="sel">
					<label for="tr-title" class="lbl">Title</label>
					<input id="tr-title" class="field" bind:value={d.title} disabled={locked} data-testid="twr-title" />
					<Findings items={errs('title').map((message) => ({ level: 'error' as const, message }))} />
				</div>
			</div>

			<div class="aq-group">
				<h3>Read a Reference</h3>
				<div class="aq-seg" role="radiogroup" aria-label="Read the reference from">
					<button type="button" role="radio" aria-checked={from === 'path'} disabled={locked} onclick={() => (from = 'path')} data-testid="twr-ref-from-path">A file</button>
					<button type="button" role="radio" aria-checked={from === 'text'} disabled={locked} onclick={() => (from = 'text')} data-testid="twr-ref-from-text">Pasted text</button>
				</div>
				{#if from === 'path'}
					<div class="sel spaced">
						<label for="tr-path" class="lbl">File</label>
						<input id="tr-path" class="field mono" placeholder="/full/path/to/reference.csv" bind:value={path} disabled={locked} data-testid="twr-ref-path" onkeydown={(e) => e.key === 'Enter' && readIt()} />
						<p class="help">CSV, TSV, WAV or JSON, by its full path on the computer that runs the test. The file is read there, never uploaded.</p>
					</div>
				{:else}
					<div class="sel spaced">
						<label for="tr-text" class="lbl">Samples</label>
						<textarea id="tr-text" class="field mono" rows="6" placeholder={'time_s, accel_g\n0, 0\n0.0001, 0.002\n...'} bind:value={text} disabled={locked} data-testid="twr-ref-text"></textarea>
						<p class="help">One sample a line: a time and a value, or a value alone with the rate given below.</p>
					</div>
				{/if}
				<div class="aq-fields spaced">
					<NumberField id="tr-rate" label="Rate" unit="Hz" nullable="From the record" fallback={12800} step={100} min={0} bind:value={rate} disabled={locked} testid="twr-ref-rate" help="Needed for values without a time column." />
					<NumberField id="tr-col" label="Column, from 0" integer nullable="The first data column" fallback={1} step={1} min={0} bind:value={column} disabled={locked} testid="twr-ref-column" help="Of a delimited file, or a WAV file's channel." />
					<div class="sel">
						<label for="tr-units" class="lbl">Unit</label>
						<input id="tr-units" class="field" placeholder={twr.unit} bind:value={units} disabled={locked} data-testid="twr-ref-units" />
						<p class="help">The record's unit; it must be the control row's ({twr.unit}).</p>
					</div>
					<div class="sel">
						<label for="tr-name" class="lbl">Name</label>
						<input id="tr-name" class="field" placeholder={from === 'path' ? "The file's name" : 'pasted'} bind:value={name} disabled={locked} data-testid="twr-ref-name" />
					</div>
				</div>
				<div class="row">
					<button class="btn" disabled={locked || reading || (from === 'path' ? !path.trim() : !text.trim())} onclick={readIt} data-testid="twr-ref-read">{reading ? 'Reading' : 'Read Reference'}</button>
				</div>
			</div>

			<div class="aq-group" data-testid="twr-ref-facts" data-sha={d.reference.sha256}>
				<h3>Chosen</h3>
				{#if facts}
					<dl class="nums">
						<div class="wide"><dt>Record</dt><dd data-testid="twr-ref-name-shown">{facts.name}<span class="sub">, {facts.format.toUpperCase()}{facts.path ? `, ${facts.path}` : ''}</span></dd></div>
						<div><dt>Rate</dt><dd class="num">{n(facts.fs_hz, 6)} Hz</dd></div>
						<div><dt>Length</dt><dd class="num">{facts.samples} samples, {n(facts.duration_s)} s</dd></div>
						<div><dt>Peak</dt><dd class="num">{n(facts.peak)} {facts.units}</dd></div>
						<div><dt>RMS</dt><dd class="num">{n(facts.rms)} {facts.units}</dd></div>
						<div class="wide"><dt>SHA-256</dt><dd class="mono sub">{shortHash(facts.sha256)}</dd></div>
					</dl>
				{:else if d.reference.sha256}
					<p class="aq-small aq-muted">The stored reference <span class="mono">{shortHash(d.reference.sha256)}</span>{d.reference.path ? `, from ${d.reference.path}` : ''}.</p>
				{:else}
					<p class="aq-small aq-muted">None yet. Read one above.</p>
				{/if}
				<Findings items={[...errs('reference').map((message) => ({ level: 'error' as const, message })), ...(twr.imported?.warnings ?? []).map((w) => ({ level: 'warning' as const, message: w.message }))]} />
				{#each refViews as v, i (i)}<div class="plot"><ViewRenderer view={v} minHeight={150} /></div>{/each}
			</div>
		</section>

		<section class="aq-panel" aria-labelledby="tr-prep" data-testid="twr-prep">
			<div class="aq-head"><h2 id="tr-prep">Preparation</h2><span class="aq-sub">Applied by the control service when the run is set up.</span></div>
			<label class="aq-switch" for="tr-resample">
				<input id="tr-resample" type="checkbox" role="switch" checked={d.prep.auto_resample} disabled={locked} onchange={(e) => (d.prep.auto_resample = e.currentTarget.checked)} data-testid="twr-prep-resample" />
				<span class="track" aria-hidden="true"><span class="thumb"></span></span>
				<span>Resample to the station's rate</span>
				<span class="word">{d.prep.auto_resample ? 'On' : 'Off'}</span>
			</label>
			<p class="help">Upward only. Off, a rate different from the station's is refused.</p>
			<div class="sel spaced">
				<span class="lbl" id="tr-detrend-l">Detrend</span>
				<div class="aq-seg" role="radiogroup" aria-labelledby="tr-detrend-l">
					{#each DETRENDS as t (t.id)}
						<button type="button" role="radio" aria-checked={d.prep.detrend === t.id} disabled={locked} onclick={() => (d.prep.detrend = t.id)} data-testid="twr-detrend-{t.id}">{t.label}</button>
					{/each}
				</div>
			</div>
			<div class="aq-fields spaced">
				<NumberField id="tr-hp" label="Drift high-pass" unit="Hz" nullable="No high-pass" fallback={5} step={1} min={0} bind:value={d.prep.highpass_hz} errors={errs('prep.highpass_hz')} disabled={locked} testid="twr-prep-hp" help="At most the correction band's low edge. Usual: 5 Hz." />
				<NumberField id="tr-scale" label="Scale" step={0.1} bind:value={d.prep.scale} errors={errs('prep.scale')} disabled={locked} testid="twr-prep-scale" help="Multiplies the reference, for a unit correction. Usual: 1." />
				<NumberField id="tr-taper" label="End taper, share of the reference" nullable="No taper" fallback={0.02} step={0.01} min={0} max={0.5} bind:value={d.prep.taper_frac} errors={errs('prep.taper_frac')} disabled={locked} testid="twr-prep-taper" help="A cosine taper at each end. Usual: 0.02." />
			</div>
		</section>

		<section class="aq-panel" aria-labelledby="tr-rec" data-testid="twr-record">
			<div class="aq-head"><h2 id="tr-rec">Section and Record</h2><span class="aq-sub">Every play plays the whole record: the section between its quiet pads.</span></div>
			<label class="aq-switch" for="tr-whole">
				<input id="tr-whole" type="checkbox" role="switch" checked={d.section.whole} disabled={locked} onchange={(e) => (d.section.whole = e.currentTarget.checked)} data-testid="twr-section-whole" />
				<span class="track" aria-hidden="true"><span class="thumb"></span></span>
				<span>Replicate the whole reference</span>
				<span class="word">{d.section.whole ? 'On' : 'Off'}</span>
			</label>
			<div class="aq-fields spaced">
				{#if !d.section.whole}
					<NumberField id="tr-s0" label="Section starts at sample" integer step={1} min={0} bind:value={d.section.start} errors={errs('section.start')} disabled={locked} testid="twr-section-start" />
					<NumberField id="tr-sn" label="Section length" unit="samples" integer step={1} min={1} bind:value={d.section.len} errors={errs('section.len')} disabled={locked} testid="twr-section-len" />
				{/if}
				<NumberField id="tr-pre" label="Quiet lead-in" unit="samples" integer step={64} min={0} bind:value={d.pads.pre} errors={errs('pads.pre')} disabled={locked} testid="twr-pad-pre" help="Room for the drive's pre-ring. Usual: 512." />
				<NumberField id="tr-post" label="Quiet lead-out" unit="samples" integer step={64} min={0} bind:value={d.pads.post} errors={errs('pads.post')} disabled={locked} testid="twr-pad-post" help="Room for the ring-down. Usual: 1024." />
				<NumberField id="tr-fs" label="Rate asked of the station" unit="Hz" nullable="The reference's rate" fallback={12800} step={100} min={0} bind:value={d.acquisition.fs_hz} errors={[...errs('acquisition.fs_hz')]} warnings={warns('acquisition.fs_hz')} disabled={locked} testid="twr-fs" help="The run reports the rate it ran at." />
			</div>
			<Findings items={[...errs('section'), ...errs('pads')].map((message) => ({ level: 'error' as const, message }))} />
		</section>
	</div>

	<section class="aq-panel preview" aria-labelledby="tr-prev" data-testid="twr-preview" data-state={previewState}>
		<div class="aq-head"><h2 id="tr-prev">As the Service Prepares It</h2></div>
		{#if prepared}
			{@const g = prepared.derived.grid}
			{@const s = prepared.derived.section}
			{@const c = prepared.derived.coverage}
			<dl class="nums" data-testid="twr-grid">
				<div><dt>Record</dt><dd class="num">{g.record_len} samples, {n(g.record_s)} s</dd></div>
				<div><dt>Rate</dt><dd class="num">{n(g.fs_hz, 6)} Hz, {n(g.df_hz)} Hz a bin</dd></div>
				<div><dt>Section</dt><dd class="num">From sample {g.section_start}, {g.section_len} samples, {n(s.duration_s)} s</dd></div>
				<div><dt>Section peak, rms</dt><dd class="num" data-testid="twr-section-peak">{n(s.peak)}, {n(s.rms)} {twr.unit}</dd></div>
				<div class="wide" data-testid="twr-coverage"><dt>Reachable by the correction</dt><dd class="num">{n(c.fraction * 100)}% of the section's energy, {c.band_hz[0]} to {c.band_hz[1]} Hz above {c.ref_floor_db} dB under its peak; an error floor of {n(c.error_floor_pct, 3)}%</dd></div>
			</dl>
			<TwrKinematics kinematics={prepared.derived.kinematics} feasibility={prepared.derived.feasibility} unit={twr.unit} />
			{#each prepared.views as v, i (i)}<div class="plot" data-testid="twr-preview-view-{i}"><ViewRenderer view={v} minHeight={150} /></div>{/each}
			<p class="aq-small aq-muted">Prepared by the control service from the stored reference: a computation, never a measurement. Identification can only lower the reachable share, by its coherence.</p>
		{:else if previewState === 'error'}
			<p class="aq-small warnline" role="status">{twr.previewError}</p>
		{:else if previewState === 'busy'}
			<p class="aq-muted aq-small">Preparing the reference.</p>
		{:else}
			<p class="aq-muted aq-small">Read a reference to see how the service prepares it.</p>
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

	.one {
		grid-template-columns: minmax(0, 1fr);
	}

	.sel {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.sel > .aq-seg {
		align-self: flex-start;
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

	.spaced {
		margin-top: var(--sp-3);
	}

	.row {
		display: flex;
		gap: var(--sp-2);
		margin-top: var(--sp-3);
	}

	.nums {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: 6px var(--sp-3);
		margin: 0 0 var(--sp-2);
	}

	.nums .wide {
		grid-column: 1 / -1;
	}

	.nums dt {
		font-size: var(--text-2xs);
		color: var(--text-3);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	.nums dd {
		margin: 0;
		font-size: var(--text-sm);
	}

	.sub {
		color: var(--text-3);
		font-size: var(--text-xs);
	}

	.plot {
		min-width: 0;
		margin-top: var(--sp-2);
	}

	.preview {
		position: sticky;
		top: 0;
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}

	.warnline {
		margin: 0;
		font-weight: var(--weight-strong);
		color: var(--amber-1);
	}

	@media (max-width: 1240px) {
		.cols {
			grid-template-columns: minmax(0, 1fr);
		}
		.preview {
			position: static;
		}
	}
</style>
