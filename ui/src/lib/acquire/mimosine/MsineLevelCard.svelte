<script lang="ts">
	/**
	 * The run in numbers, in the dock: the tone's frequency, direction and sweep rate, the passes or
	 * steps done, the level, the notch scale and whether the run is held; the readings across the
	 * shakers (shown, never judged); then per control its amplitude against its reference, its error
	 * and its phase error and any notch depth; per limit channel its margin to its bound; per drive
	 * its amplitude and phase as played. After the run, how it ended and what tripped. Every number
	 * is the service's.
	 */
	import type { MimoSineLiveHead, MimoSineState } from '$lib/protocol/mimoSine';
	import { formatNumber } from '$lib/viz/format';
	import { db } from '../words';
	import { hz, MSINE_OUTCOME_WORDS, MSINE_TRIP_WORDS, msineTripFacts, readingsWords } from './msineWords';

	interface Props {
		status: MimoSineState | null;
		live: MimoSineLiveHead | null;
		compact?: boolean;
	}
	let { status, live, compact = false }: Props = $props();

	const n = (v: number | null | undefined, sig = 4) => (v === null || v === undefined || !Number.isFinite(v) ? '-' : formatNumber(v, sig));
	const rate = $derived.by(() => {
		const oct = live?.sweep_rate_oct_per_min;
		if (typeof oct === 'number') return `${n(Math.abs(oct), 3)} oct/min`;
		const lin = live?.sweep_rate_hz_per_s;
		if (typeof lin === 'number') return `${n(Math.abs(lin), 3)} Hz/s`;
		return null;
	});
	const progress = $derived.by(() => {
		const s = live ?? status;
		if (s?.sweeps) return `Sweep ${s.sweeps.done}${s.sweeps.of !== null ? ` of ${s.sweeps.of}` : ''}`;
		if (s?.steps) return `Step ${s.steps.done} of ${s.steps.of}`;
		return null;
	});
	const holdWords = $derived.by(() => {
		if (live?.held ?? status?.held) return 'Held by the operator';
		if (live?.request_held ?? status?.request_held) return 'Held or slowed at the law\'s request';
		if (live?.request && !live.request.granted) return `The law asks to ${live.request.kind === 'hold' ? 'hold' : 'slow'} the sweep; not acted on`;
		return null;
	});
	const scale = $derived(live?.notch_scale ?? status?.notch_scale ?? 1);
	const readings = $derived(readingsWords(live?.readings));
	const end = $derived(status?.state === 'finished' ? status.end : null);
</script>

<div class="card" class:compact data-testid="msine-level-card">
	<dl class="head">
		<div><dt>Frequency</dt><dd class="num big" data-testid="msine-frequency">{hz(live?.frequency_hz ?? status?.frequency_hz)}</dd></div>
		<div><dt>Level</dt><dd class="num">{db(live?.level_db ?? status?.level_db ?? null)}</dd></div>
		{#if (live?.direction ?? status?.direction) && rate}<div><dt>Sweep</dt><dd class="num">{live?.direction ?? status?.direction}, {rate}</dd></div>{/if}
		{#if progress}<div><dt>Progress</dt><dd class="num">{progress}</dd></div>{/if}
		<div><dt>Notch</dt><dd class="num" class:warn={scale < 1} data-testid="msine-notch">{scale < 1 ? `Scaled to ${n(scale, 3)} (${n(-20 * Math.log10(scale), 3)} dB)` : 'None'}</dd></div>
		{#if holdWords}<div><dt>Hold</dt><dd data-testid="msine-hold">{holdWords}</dd></div>{/if}
		{#if live?.alarm ?? status?.alarm}<div><dt>Alarm</dt><dd class="warn">A control is outside its alarm band</dd></div>{/if}
	</dl>

	{#if readings.length && live}
		<dl class="readings" data-testid="msine-readings">
			{#each readings as r (r.label)}<div title={r.help}><dt>{r.label}</dt><dd class="num">{r.value}</dd></div>{/each}
		</dl>
	{/if}

	{#if live?.controls.length && !compact}
		<table class="aq-table small" data-testid="msine-controls-table">
			<thead><tr><th>Control</th><th class="num">Measured</th><th class="num">Reference</th><th class="num">Error</th><th class="num">Phase error</th><th class="num">Notch</th></tr></thead>
			<tbody>
				{#each live.controls as c (c.channel)}
					<tr>
						<td>{c.channel}</td>
						<td class="num">{n(c.amplitude)} {c.unit}</td>
						<td class="num">{n(c.reference)}</td>
						<td class="num">{c.error_db === null ? '-' : `${n(c.error_db, 3)} dB`}</td>
						<td class="num">{c.phase_error_deg === null ? '-' : `${n(c.phase_error_deg, 3)} deg`}</td>
						<td class="num">{c.notch_depth_db > 0 ? `${n(c.notch_depth_db, 3)} dB` : '-'}</td>
					</tr>
				{/each}
			</tbody>
		</table>
		{#if live.limits.length}
			<table class="aq-table small" data-testid="msine-limits-table">
				<thead><tr><th>Limit</th><th class="num">Measured</th><th class="num">Bound</th><th class="num">Margin</th></tr></thead>
				<tbody>
					{#each live.limits as l (l.channel)}
						<tr><td>{l.channel}</td><td class="num">{n(l.amplitude)} {l.unit}</td><td class="num">{n(l.bound)}</td><td class="num">{l.margin_db === null ? '-' : `${n(l.margin_db, 3)} dB`}</td></tr>
					{/each}
				</tbody>
			</table>
		{/if}
		<table class="aq-table small" data-testid="msine-drives-table">
			<thead><tr><th>Drive</th><th class="num">Amplitude</th><th class="num">Phase</th><th class="num">Ceiling</th></tr></thead>
			<tbody>
				{#each live.drives as x (x.channel)}
					<tr><td>{x.channel}</td><td class="num">{n(x.vpk)} V pk</td><td class="num">{n(x.phase_deg, 3)} deg</td><td class="num">{n(x.ceiling_vpk, 3)} V pk</td></tr>
				{/each}
			</tbody>
		</table>
	{/if}

	{#if end}
		<div class="end" data-testid="msine-end">
			<p><strong>{MSINE_OUTCOME_WORDS[end.outcome] ?? end.outcome}</strong>{end.trip ? `: ${MSINE_TRIP_WORDS[end.trip.kind]}` : ''}</p>
			{#if end.trip}
				<dl>
					{#each msineTripFacts(end.trip) as f (f.label)}<div><dt>{f.label}</dt><dd class="num">{f.value}</dd></div>{/each}
				</dl>
			{/if}
		</div>
	{/if}
</div>

<style>
	.card {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		font-size: var(--text-xs);
	}

	dl {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: 4px var(--sp-3);
		margin: 0;
	}

	dt {
		font-size: var(--text-2xs);
		color: var(--text-3);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	dd {
		margin: 0;
	}

	.big {
		font-size: var(--text-md, var(--text-sm));
		font-weight: var(--weight-strong);
	}

	.warn {
		color: var(--amber-1);
	}

	.small {
		font-size: var(--text-xs);
	}

	.end p {
		margin: 0 0 4px;
	}
</style>
