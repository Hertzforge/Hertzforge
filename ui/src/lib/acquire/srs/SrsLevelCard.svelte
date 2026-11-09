<script lang="ts">
	/**
	 * The SRS shock run at a glance: what it is doing in plain words, the pulse coming against the
	 * plan and when it is due, the last pulse's worst band and how many bands sat outside the
	 * envelope, the drive it played and its kinematics, the identification against its cap and its
	 * hold, the prediction against the drive ceiling and abort, the limit notch, and, once the run
	 * ends, how and why. Every number is the service's; shape and word carry each state, color only
	 * follows.
	 */
	import type { SrsState } from '$lib/protocol/srs';
	import StatusMark from '$lib/viz/StatusMark.svelte';
	import { formatNumber } from '$lib/viz/format';
	import { db } from '../words';
	import { lastPulseWords, nextPulseWords, notchWords, pulseWords, srsLevelStatus, srsTripFacts, SRS_TRIP_WORDS, worstBandWords } from './srsWords';

	interface Props {
		status: SrsState | null;
		/** The pulses the schedule plans. */
		planned: number;
		compact?: boolean;
	}
	let { status, planned, compact = false }: Props = $props();

	const st = $derived(srsLevelStatus(status));
	const continuing = $derived(status?.state === 'running');
	const coming = $derived(status ? pulseWords(status, planned) : null);
	const next = $derived(status ? nextPulseWords(status) : null);
	const last = $derived(status?.last_pulse ?? null);
	const lastWords = $derived(lastPulseWords(last, continuing));
	const worst = $derived(worstBandWords(last));
	const id = $derived(status?.identification ?? null);
	const pred = $derived(status?.prediction ?? null);
	const notch = $derived(notchWords(status?.notch ?? null));
	const trip = $derived(status?.end?.trip ?? null);
	const n = (v: number | null, sig: number, unit: string) => (v === null ? '-' : `${formatNumber(v, sig)} ${unit}`);
</script>

<section class="levelcard" class:compact aria-label="Level" data-testid="srs-level-card">
	<div class="statusline {st.tone}" role="status" data-testid="srs-level-status">
		<span class="mk" aria-hidden="true">
			<svg viewBox="0 0 14 14" width="14" height="14">
				{#if st.mark === 'control'}
					<circle cx="7" cy="7" r="5.5" fill="currentColor" />
				{:else if st.mark === 'identify'}
					<circle cx="7" cy="7" r="5" fill="none" stroke="currentColor" stroke-width="1.5" /><circle cx="7" cy="7" r="1.8" fill="currentColor" />
				{:else if st.mark === 'hold'}
					<rect x="3.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" /><rect x="8.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" />
				{:else if st.mark === 'stop'}
					<path d="M2 3.5L7 11l5-7.5z" fill="currentColor" />
				{:else if st.mark === 'done'}
					<circle cx="7" cy="7" r="6" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M4.2 7.2l1.9 1.9 3.8-4" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" />
				{:else if st.mark === 'ended'}
					<rect x="2.5" y="2.5" width="9" height="9" rx="1.3" fill="currentColor" />
				{:else if st.mark === 'armed'}
					<circle cx="7" cy="7" r="5" fill="none" stroke="currentColor" stroke-width="1.5" /><path d="M7 2a5 5 0 010 10z" fill="currentColor" />
				{:else}
					<circle cx="7" cy="7" r="5" fill="none" stroke="currentColor" stroke-width="1.4" stroke-dasharray="2.2 2" />
				{/if}
			</svg>
		</span>
		<span class="words">{st.text}</span>
		{#if status?.mode}<span class="mode">{status.mode === 'auto' ? 'Automatic' : 'Manual'}</span>{/if}
	</div>

	{#if coming}<p class="coming" data-testid="srs-pulse-words">{coming}</p>{/if}
	{#if next}<p class="next" data-testid="srs-next-pulse">{next}</p>{/if}

	{#if lastWords}
		<p class="last {lastWords.tone}" data-testid="srs-last-pulse">
			<StatusMark status={lastWords.tone} word={false} />
			{lastWords.text}
		</p>
	{/if}
	{#if worst}<p class="worst" data-testid="srs-worst-band">{worst}</p>{/if}

	{#if last}
		<dl class="nums" data-testid="srs-last-numbers">
			<div><dt>Level</dt><dd class="num">{db(last.commanded_db)}</dd></div>
			<div><dt>Drive</dt><dd class="num">{formatNumber(last.drive_vpk, 4)} V pk{#if last.limit_scale < 1}<span class="sub"> scaled to {formatNumber(last.limit_scale * 100, 3)}%</span>{/if}</dd></div>
			<div><dt>Bands outside the alarm</dt><dd class="num">{last.alarm_bands ?? '-'}</dd></div>
			<div><dt>Outside the abort</dt><dd class="num">{last.abort_bands ?? '-'}</dd></div>
			<div><dt>Peak</dt><dd class="num">{n(last.peak_g, 4, 'g')}</dd></div>
			<div><dt>Displacement</dt><dd class="num">{n(last.displacement_pk_pk_in, 4, 'in pk-pk')}</dd></div>
			{#if last.limited_components !== null}
				<div class="wide"><dt>Held by the limit channels</dt><dd class="num">{last.limited_components} {last.limited_components === 1 ? 'component' : 'components'}{last.worst_limit_db === null ? '' : `; worst prediction ${formatNumber(Math.abs(last.worst_limit_db), 3)} dB ${last.worst_limit_db > 0 ? 'over' : 'under'} its allowed level`}</dd></div>
			{/if}
		</dl>
	{/if}

	{#if id && !compact}
		<dl class="nums sub-block" data-testid="srs-identification-facts">
			<div class="wide"><dt>Identification</dt><dd class="num">Burst {formatNumber(id.drive_vpk, 4)} V pk, held at {formatNumber(id.max_vpk, 4)}</dd></div>
			<div class="wide" class:fail={!id.passed}><dt>Coherence</dt><dd class="num"><StatusMark status={id.passed ? 'pass' : 'fail'} word={false} /> {formatNumber(id.coherence_mean, 3)} mean in the band; it must reach {formatNumber(id.min_mean_coherence, 3)}</dd></div>
		</dl>
	{/if}

	{#if pred && !compact}
		<dl class="nums sub-block" data-testid="srs-prediction">
			<div class="wide"><dt>Predicted at full level</dt><dd class="num">{formatNumber(pred.drive_vpk, 4)} V pk drive; ceiling {formatNumber(pred.drive_vpk_ceiling, 4)}, abort {formatNumber(pred.drive_vpk_abort, 4)}</dd></div>
			<div><dt>Response peak</dt><dd class="num">{formatNumber(pred.response_peak_g, 4)} g</dd></div>
			<div><dt>Displacement</dt><dd class="num">{formatNumber(pred.displacement_pk_pk_in, 4)} in pk-pk</dd></div>
		</dl>
	{/if}

	{#if notch && !compact}
		<p class="notch" data-testid="srs-notch-card">{notch}</p>
	{/if}

	{#if status?.state === 'finished' && status.end}
		<div class="end" role="status" data-testid="srs-end">
			<p class="msg">{status.end.message}</p>
			{#if trip}
				<h4>{SRS_TRIP_WORDS[trip.kind]}</h4>
				<dl class="nums">
					{#each srsTripFacts(trip) as f (f.label)}<div><dt>{f.label}</dt><dd class="num">{f.value}</dd></div>{/each}
				</dl>
			{/if}
		</div>
	{/if}
</section>

<style>
	.levelcard {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}

	.statusline {
		display: flex;
		align-items: center;
		gap: 8px;
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
	}

	.statusline.pass .mk {
		color: var(--status-pass);
	}

	.statusline.warn .mk,
	.statusline.fail .mk {
		color: var(--amber-1);
	}

	.statusline.info .mk {
		color: var(--text-3);
	}

	.mk {
		display: inline-flex;
	}

	.words {
		flex: 1;
	}

	.mode {
		font-size: var(--text-2xs);
		font-weight: var(--weight-medium);
		color: var(--text-3);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	.coming {
		margin: 0;
		font-size: var(--text-lg);
		font-weight: var(--weight-strong);
		color: var(--text-1);
	}

	.next,
	.worst,
	.notch {
		margin: 0;
		font-size: var(--text-sm);
		color: var(--text-2);
	}

	.last {
		display: flex;
		align-items: center;
		gap: 6px;
		margin: 0;
		font-size: var(--text-sm);
	}

	.last.warn {
		font-weight: var(--weight-strong);
		color: var(--amber-1);
	}

	.nums {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: 6px var(--sp-3);
		margin: 0;
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

	.nums .fail dd {
		font-weight: var(--weight-strong);
	}

	.sub {
		color: var(--text-3);
		font-size: var(--text-xs);
	}

	.sub-block {
		padding-top: var(--sp-2);
		border-top: 1px solid var(--line-1);
	}

	.end {
		padding-top: var(--sp-2);
		border-top: 1px solid var(--line-1);
	}

	.end .msg {
		margin: 0 0 var(--sp-2);
		font-size: var(--text-sm);
	}

	.end h4 {
		margin: 0 0 4px;
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
	}
</style>
