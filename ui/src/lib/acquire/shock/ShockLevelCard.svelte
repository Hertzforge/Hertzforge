<script lang="ts">
	/**
	 * The classical shock run at a glance: what it is doing in plain words, the pulse coming against
	 * the plan and when it is due, the last pulse's commanded and measured peak with its error and
	 * velocity change, its three verdicts, the prediction against the drive ceiling and abort, and,
	 * once the run ends, how and why. Every number is the service's; shape and word carry each state,
	 * color only follows.
	 */
	import type { ShockState } from '$lib/protocol/shock';
	import StatusMark from '$lib/viz/StatusMark.svelte';
	import { formatNumber } from '$lib/viz/format';
	import { db } from '../words';
	import { lastPulseWords, nextPulseWords, pulseNumber, pulseWords, shockLevelStatus, shockTripFacts, SHOCK_TRIP_WORDS, verdictWord } from './shockWords';

	interface Props {
		status: ShockState | null;
		/** The pulses the schedule plans. */
		planned: number;
		compact?: boolean;
	}
	let { status, planned, compact = false }: Props = $props();

	const st = $derived(shockLevelStatus(status));
	const continuing = $derived(status?.state === 'running');
	const coming = $derived(status ? pulseWords(status, planned) : null);
	const next = $derived(status ? nextPulseWords(status) : null);
	const last = $derived(status?.last_pulse ?? null);
	const lastWords = $derived(lastPulseWords(last, continuing));
	const pred = $derived(status?.prediction ?? null);
	const trip = $derived(status?.end?.trip ?? null);
	const commandedPeak = $derived(last && last.peak_g !== null && last.peak_ratio ? last.peak_g / last.peak_ratio : null);
</script>

<section class="levelcard" class:compact aria-label="Level" data-testid="shock-level-card">
	<div class="statusline {st.tone}" role="status" data-testid="shock-level-status">
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

	{#if coming}<p class="coming num" data-testid="shock-pulse-words">{coming}</p>{/if}
	{#if next}<p class="next" data-testid="shock-next-pulse">{next}</p>{/if}

	{#if lastWords}
		<p class="last {lastWords.tone}" data-testid="shock-last-pulse">
			<StatusMark status={lastWords.tone} word={false} />
			{lastWords.text}
		</p>
	{/if}

	{#if last}
		<dl class="nums" data-testid="shock-last-numbers">
			<div><dt>Commanded</dt><dd class="num">{commandedPeak === null ? '-' : `${formatNumber(commandedPeak, 4)} g`}</dd></div>
			<div><dt>Measured</dt><dd class="num">{last.peak_g === null ? '-' : `${formatNumber(last.peak_g, 4)} g`}</dd></div>
			<div><dt>Error</dt><dd class="num">{last.error_pct === null ? '-' : `${formatNumber(last.error_pct, 3)}%`}</dd></div>
			<div><dt>Velocity change</dt><dd class="num">{last.velocity_change_ips === null ? '-' : `${formatNumber(last.velocity_change_ips, 4)} in/s`}</dd></div>
			<div><dt>Drive</dt><dd class="num">{formatNumber(last.drive_vpk, 4)} V pk{#if last.limit_scale < 1}<span class="sub"> scaled to {formatNumber(last.limit_scale * 100, 3)}%</span>{/if}</dd></div>
			<div><dt>Level</dt><dd class="num">{db(last.commanded_db)}</dd></div>
		</dl>
		{#if last.verdicts}
			<ul class="verdicts" aria-label="Pulse {pulseNumber(last.index)} verdicts" data-testid="shock-verdicts">
				{#each [['Envelope', last.verdicts.envelope], ['Velocity change', last.verdicts.velocity], ['Substitution', last.verdicts.substitution]] as [label, ok] (label)}
					<li class:fail={!ok}><StatusMark status={ok ? 'pass' : 'fail'} word={false} /><span>{label}</span><span class="w">{verdictWord(ok as boolean)}</span></li>
				{/each}
			</ul>
		{/if}
	{/if}

	{#if pred && !compact}
		<dl class="nums pred" data-testid="shock-prediction">
			<div class="wide"><dt>Predicted at full level</dt><dd class="num">{formatNumber(pred.drive_vpk, 4)} V pk drive; ceiling {formatNumber(pred.drive_vpk_ceiling, 4)}, abort {formatNumber(pred.drive_vpk_abort, 4)}</dd></div>
			<div><dt>Response peak</dt><dd class="num">{formatNumber(pred.response_peak_g, 4)} g</dd></div>
			<div><dt>Displacement</dt><dd class="num">{formatNumber(pred.displacement_pk_pk_in, 4)} in pk-pk</dd></div>
		</dl>
	{/if}

	{#if status?.state === 'finished' && status.end}
		<div class="end" role="status" data-testid="shock-end">
			<p class="msg">{status.end.message}</p>
			{#if trip}
				<h4>{SHOCK_TRIP_WORDS[trip.kind]}</h4>
				<dl class="nums">
					{#each shockTripFacts(trip) as f (f.label)}<div><dt>{f.label}</dt><dd class="num">{f.value}</dd></div>{/each}
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

	.next {
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

	.sub {
		color: var(--text-3);
		font-size: var(--text-xs);
	}

	.pred {
		padding-top: var(--sp-2);
		border-top: 1px solid var(--line-1);
	}

	.verdicts {
		display: flex;
		flex-wrap: wrap;
		gap: 4px var(--sp-3);
		margin: 0;
		padding: 0;
		list-style: none;
		font-size: var(--text-xs);
	}

	.verdicts li {
		display: inline-flex;
		align-items: center;
		gap: 4px;
	}

	.verdicts .w {
		color: var(--text-3);
	}

	.verdicts li.fail {
		font-weight: var(--weight-strong);
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
