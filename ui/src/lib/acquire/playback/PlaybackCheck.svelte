<script lang="ts">
	/**
	 * The drive file's check, as the control service made it: the dry run of the draft while it is
	 * edited, the run's own once applied. Per channel its peak, its first and last samples, and its
	 * velocity and displacement (the drive taken as an acceleration and integrated with no
	 * high-pass) with their end values against their peaks, pass or each fault's words; and a
	 * small preview of each drive with its velocity and displacement. When the ends fail, Fix Ends
	 * has the analysis service write a copy whose ends are at rest, shows each channel before and
	 * after, and the setup plays the copy only when the operator says so. Nothing here is a guard:
	 * the control service refuses a file that fails, whatever this screen shows.
	 */
	import ViewRenderer from '$lib/viz/ViewRenderer.svelte';
	import type { PlaybackEnvelope } from '$lib/protocol/playback';
	import type { SpectrumView } from '$lib/protocol/types';
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import type { PlaybackStore } from './playbackStore.svelte';
	import { endsOnly, residual, sci, verdict, volts } from './playbackWords';

	interface Props {
		playback: PlaybackStore;
	}
	let { playback }: Props = $props();

	const check = $derived(playback.checkOf);
	const report = $derived(playback.checkReport);
	const errors = $derived(playback.fieldErrors['check'] ?? []);
	const fixable = $derived(!!report && !report.pass && endsOnly(report.refusals));
	const fix = $derived(playback.fix);
	const locked = $derived(playback.locked);
	let taper = $state<number | null>(null);

	function envView(title: string, e: PlaybackEnvelope, unit: string): SpectrumView {
		const x = Float64Array.from(e.t_s);
		return {
			view: 'spectrum',
			title,
			x: { label: 'Time', unit: 's', scale: 'linear' },
			y: { label: title, unit, scale: 'linear' },
			series: [{ name: title, role: title === 'Drive' ? 'drive' : 'measured', x, y: Float64Array.from(e.max), y_min: Float64Array.from(e.min) }]
		};
	}
</script>

<div class="stack" data-testid="playback-check" data-pass={report ? String(report.pass) : ''} data-fixable={String(fixable)}>
	<section class="aq-panel" aria-labelledby="pb-check">
		<div class="aq-head">
			<h2 id="pb-check">Drive Check</h2>
			<span class="aq-sub">{playback.setupStatus === 'applied' ? 'As the control service checked it for this run.' : 'The control service checks the drive file as the setup stands.'}</span>
		</div>
		{#if !check}
			<p class="aq-small aq-muted">
				{#if playback.checking}Checking the drive file.{:else if !playback.connected}The control service checks the drive once it is connected.{:else}The drive is checked once a drive file is converted and every channel has a drive row.{/if}
			</p>
		{:else if !report}
			<Findings items={check.refusals.map((r) => ({ level: 'error' as const, message: r.message }))} />
		{:else}
			<p class="verdict {report.pass ? 'pass' : 'fail'}" data-testid="pb-check-verdict">
				{report.pass ? 'The drive checks clean: it may be played at this level and ceiling.' : 'The control service refuses this drive as it stands.'}
			</p>
			<p class="aq-small aq-muted">{report.samples} samples at {report.rate_hz} Hz ({report.duration_s} s), at {report.level_db} dB within a ceiling of {report.ceiling_v} V. Ends within {report.tolerances.ends_fraction_of_peak * 100} % of the peak; velocity and displacement ending within {report.tolerances.velocity_fraction_of_peak * 100} % and {report.tolerances.displacement_fraction_of_peak * 100} % of theirs.</p>
			<div class="scroll">
				<table class="aq-table" data-testid="pb-check-table">
					<thead><tr><th>Channel</th><th>Row</th><th class="r">Peak</th><th class="r">Played peak</th><th class="r">Start</th><th class="r">End</th><th>Velocity end</th><th>Displacement end</th><th>Verdict</th></tr></thead>
					<tbody>
						{#each report.channels as c (c.channel)}
							<tr class:bad={!c.pass} data-testid="pb-check-{c.channel}" data-pass={String(c.pass)}>
								<td>{c.channel}, {c.name}</td>
								<td class="num">{c.drive_row ?? '-'}</td>
								<td class="r num">{volts(c.peak_v)}</td>
								<td class="r num">{volts(c.played_peak_v)}</td>
								<td class="r num">{sci(c.start_v, 'V')}</td>
								<td class="r num">{sci(c.end_v, 'V')}</td>
								<td class="num">{residual(c.velocity_v_s, 'V s')}</td>
								<td class="num">{residual(c.displacement_v_s2, 'V s²')}</td>
								<td class="aq-small">{verdict(c)}</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
			<Findings items={[...report.refusals.map((r) => r.message), ...errors].filter((m, i, a) => a.indexOf(m) === i).map((message) => ({ level: 'error' as const, message }))} />
			<p class="aq-small aq-muted">{report.limits}</p>
			{#if report.previews?.length}
				<div class="previews" data-testid="pb-previews">
					{#each report.previews as p (p.channel)}
						<figure class="pv">
							<figcaption class="aq-small">Channel {p.channel}{report.channels[p.channel] ? `, ${report.channels[p.channel].name}` : ''}</figcaption>
							<div class="three">
								<ViewRenderer view={envView('Drive', p.drive_v, 'V')} minHeight={110} />
								<ViewRenderer view={envView('Velocity', p.velocity_v_s, 'V s')} minHeight={110} />
								<ViewRenderer view={envView('Displacement', p.displacement_v_s2, 'V s²')} minHeight={110} />
							</div>
						</figure>
					{/each}
				</div>
			{/if}
		{/if}
	</section>

	{#if fixable || fix}
		<section class="aq-panel" aria-labelledby="pb-fix" data-testid="pb-fix">
			<div class="aq-head"><h2 id="pb-fix">Fix Ends</h2></div>
			{#if !fix}
				<p class="aq-small">Writes a copy of the drive with its ends at rest, beside it: a half-cosine taper at each end, then a small correction (one half and one whole sine across the record) so its velocity and displacement end at zero. The original is not changed. You see each channel before and after, and the setup plays the copy only when you choose it.</p>
				<div class="aq-fields">
					<NumberField id="pb-taper" label="Taper at each end" unit="s" nullable="1 % of the record" fallback={0.01} step={0.01} min={0} bind:value={taper} disabled={locked} testid="pb-taper" help="At least 10 ms and at most 1 s when left to the record." />
				</div>
				<div class="row">
					<button class="btn primary" disabled={locked || !!playback.working} onclick={() => void playback.fixEnds(taper ?? undefined)} data-testid="pb-fix-ends">{playback.working === 'Fix Ends' ? 'Fixing' : 'Fix Ends'}</button>
				</div>
				{#if playback.fileError}<Findings items={[{ level: 'error', message: playback.fileError }]} />{/if}
			{:else}
				{@const r = fix.result}
				<p class="aq-small">Written to <span class="mono">{r.folder}</span>, from <span class="mono">{r.from.folder}</span>: a taper of {r.taper_s} s ({r.taper_samples} samples) at each end, then the correction.</p>
				<div class="scroll">
					<table class="aq-table" data-testid="pb-fix-table">
						<thead><tr><th>Channel</th><th></th><th class="r">Start</th><th class="r">End</th><th class="r">Peak</th><th>Velocity end</th><th>Displacement end</th><th class="r">Rms</th></tr></thead>
						<tbody>
							{#each r.channels as c, k (k)}
								<tr><td rowspan="2">{k}, {c.name}</td><td>Before</td><td class="r num">{sci(c.before.start, 'V')}</td><td class="r num">{sci(c.before.end, 'V')}</td><td class="r num">{volts(c.before.peak)}</td><td class="num">{residual({ end: c.before.velocity_end, peak: c.before.velocity_peak }, 'V s')}</td><td class="num">{residual({ end: c.before.displacement_end, peak: c.before.displacement_peak }, 'V s²')}</td><td class="r num">{volts(c.rms_before)}</td></tr>
								<tr><td>After</td><td class="r num">{sci(c.after.start, 'V')}</td><td class="r num">{sci(c.after.end, 'V')}</td><td class="r num">{volts(c.after.peak)}</td><td class="num">{residual({ end: c.after.velocity_end, peak: c.after.velocity_peak }, 'V s')}</td><td class="num">{residual({ end: c.after.displacement_end, peak: c.after.displacement_peak }, 'V s²')}</td><td class="r num">{volts(c.rms_after)}</td></tr>
							{/each}
						</tbody>
					</table>
				</div>
				<p class="aq-small aq-muted">A fix changes the drive the test plays. A long taper or a large correction changes what the test asks of the shaker; look at both before you choose.</p>
				<div class="row">
					<button class="btn primary" disabled={locked} onclick={() => playback.acceptFix()} data-testid="pb-fix-use">Play the Fixed Drive</button>
					<button class="btn" onclick={() => playback.discardFix()} data-testid="pb-fix-keep">Keep the Original</button>
				</div>
			{/if}
		</section>
	{/if}
</div>

<style>
	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		max-width: 1100px;
	}

	.verdict {
		margin: 0 0 var(--sp-2);
		font-size: var(--text-sm);
		font-weight: var(--weight-medium);
	}

	.verdict.pass {
		color: var(--blue-1);
	}

	.verdict.fail {
		color: var(--status-fail-text, var(--text-1));
	}

	.scroll {
		overflow-x: auto;
	}

	tr.bad td {
		background: var(--amber-weak);
	}

	.r {
		text-align: right;
	}

	.mono {
		font-family: var(--font-mono);
		word-break: break-all;
	}

	.previews {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		margin-top: var(--sp-3);
	}

	.pv {
		margin: 0;
	}

	.three {
		display: grid;
		grid-template-columns: repeat(3, minmax(0, 1fr));
		gap: var(--sp-2);
	}

	.row {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		margin-top: var(--sp-3);
	}
</style>
