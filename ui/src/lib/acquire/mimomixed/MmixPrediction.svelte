<script lang="ts">
	/**
	 * The prediction and the budget forecast, once the structure is identified: per drive, the
	 * floor's random (rms and its peak at the crest factor) and the tones' peak sum, against the drive's
	 * ceilings; then the forecast's moment nearest a ceiling, its need drawn against its ceiling as a
	 * bar with the share written beside it. A drive that cannot fit is said in words with its time,
	 * and a forecast the service refuses is said by its name. The service names each drive and control
	 * by its channel. Shapes and words carry every state; the color only repeats them.
	 */
	import type { MimoMixedPrediction } from '$lib/protocol/mimoMixed';
	import { MMIX_REFUSAL_WORDS, needWords, volts } from './mmixWords';

	interface Props {
		prediction: MimoMixedPrediction;
		/** A channel's name. */
		names: (channel: number) => string;
		testid?: string;
	}
	let { prediction, names, testid = 'mmix-prediction' }: Props = $props();

	const name = (c: number) => names(c) || `Channel ${c}`;
	const share = (need: number, ceiling: number) => (ceiling > 0 ? Math.min(1.25, need / ceiling) : 0);
	const fit = $derived(prediction.forecast.cannot_fit);
	const worst = $derived(prediction.forecast.worst);
	const refused = $derived(prediction.forecast.refused);
	const num = (v: number | null, unit = 'g') => (v === null || !Number.isFinite(v) ? '-' : `${v.toFixed(3)} ${unit}`);
</script>

<div class="pred" data-testid={testid} data-within={prediction.within} data-refused={refused ?? ''}>
	{#if refused}
		<p class="aq-notice warn aq-small" role="alert" data-testid="{testid}-refused">{MMIX_REFUSAL_WORDS[refused as keyof typeof MMIX_REFUSAL_WORDS] ?? refused}: the run cannot engage on this setup, and no override passes it.</p>
	{/if}
	{#if fit}
		<p class="aq-notice warn aq-small" data-testid="{testid}-cannot-fit">The budget cannot fit: drive {name(fit.drive)} needs {volts(fit.need_v)} against its {volts(fit.ceiling_v)} peak ceiling at {fit.time_s ?? '-'} s into the schedule.</p>
	{/if}
	{#if worst && worst.need_v !== null && worst.ceiling_v !== null}
		{@const s = share(worst.need_v, worst.ceiling_v)}
		{@const over = worst.need_v > worst.ceiling_v}
		<h3>The Forecast's Nearest Approach to a Ceiling</h3>
		<ul class="bars" data-testid="{testid}-forecast">
			<li data-testid="{testid}-drive-{worst.drive}" data-over={over}>
				<span class="nm">Drive {name(worst.drive)}</span>
				<span class="bar" aria-hidden="true"><span class="fill" class:over style:width="{(s * 80).toFixed(1)}%"></span><span class="ceil" style:left="80%"></span></span>
				<span class="words aq-small">{over ? 'Over: ' : ''}{needWords(worst.need_v, worst.ceiling_v)}{worst.time_s ? ` at ${worst.time_s} s` : ''}</span>
			</li>
		</ul>
		<p class="aq-small step">Forecast in steps of {prediction.forecast.step_s} s along the schedule.</p>
	{/if}
	<div class="aq-scroll">
		<table class="aq-table" data-testid="{testid}-drives">
			<thead><tr><th>Drive</th><th class="num">Random rms</th><th class="num">Random peak</th><th class="num">Tones' peak sum</th><th class="num">Total rms</th><th class="num">Total peak</th><th class="num">Peak ceiling</th><th class="num">RMS ceiling</th><th>Fits</th></tr></thead>
			<tbody>
				{#each prediction.drives as x (x.drive)}
					<tr>
						<td>{name(x.drive)}</td>
						<td class="num">{volts(x.random_rms_v)}</td>
						<td class="num">{volts(x.random_peak_v)}</td>
						<td class="num">{volts(x.tone_peak_sum_v)}</td>
						<td class="num">{volts(x.total_rms_v)}</td>
						<td class="num">{volts(x.peak_v)}</td>
						<td class="num">{volts(x.ceiling_peak_v)}</td>
						<td class="num">{volts(x.ceiling_vrms)}</td>
						<td>{x.within ? 'Yes' : 'No, over a ceiling'}</td>
					</tr>
				{/each}
			</tbody>
		</table>
	</div>
	{#if prediction.controls.length}
		<div class="aq-scroll">
			<table class="aq-table" data-testid="{testid}-controls">
				<thead><tr><th>Control</th><th class="num">Random rms</th><th class="num">Tones rms</th><th class="num">Total rms</th></tr></thead>
				<tbody>
					{#each prediction.controls as c (c.control)}
						<tr><td>{name(c.control)}</td><td class="num">{num(c.random_rms)}</td><td class="num">{num(c.tone_rms)}</td><td class="num">{num(c.total_rms)}</td></tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
</div>

<style>
	.pred {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		min-width: 0;
	}

	h3 {
		margin: 0;
		font-size: var(--text-sm);
	}

	.bars {
		margin: 0;
		padding: 0;
		list-style: none;
		display: flex;
		flex-direction: column;
		gap: 6px;
	}

	.bars li {
		display: grid;
		grid-template-columns: minmax(6em, 10em) minmax(120px, 1fr) auto;
		align-items: center;
		gap: var(--sp-2);
	}

	.bar {
		position: relative;
		height: 10px;
		background: var(--bg-3);
		border-radius: 3px;
	}

	.fill {
		position: absolute;
		inset: 0 auto 0 0;
		background: var(--blue-2);
		border-radius: 3px;
	}

	.fill.over {
		background: var(--amber-1);
	}

	.ceil {
		position: absolute;
		top: -3px;
		bottom: -3px;
		width: 2px;
		background: var(--text-1);
	}

	.words {
		white-space: nowrap;
	}

	.step {
		margin: 0;
		color: var(--text-2);
	}
</style>
