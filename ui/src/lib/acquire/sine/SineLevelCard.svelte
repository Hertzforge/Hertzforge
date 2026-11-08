<script lang="ts">
	/**
	 * The Sine run at a glance: what it is doing in plain words, the tone's frequency large, the
	 * sweep under way with its direction, the level the drive is at against the level commanded, the
	 * target against the measured amplitude with the difference in dB, how far through the schedule
	 * the run is, the drive as played, the law's unusable calls and capped ticks, the identification,
	 * and, once the run ends, how and why. Every number is the service's; shape and word carry each
	 * state, color only follows.
	 */
	import type { SineConfig, SineLive, SineState } from '$lib/protocol/sine';
	import { formatNumber } from '$lib/viz/format';
	import { clock, db } from '../words';
	import { errorWords, hz, identificationWords, progressOf, sineLevelStatus, sineTripFacts, sweepWords, SINE_TRIP_WORDS } from './sineWords';

	interface Props {
		/** The state with the live frame laid over it. */
		status: SineState | null;
		/** The live frame, when it is this run's. */
		live: SineLive | null;
		config?: SineConfig | null;
		compact?: boolean;
	}
	let { status, live, config = null, compact = false }: Props = $props();

	const st = $derived(sineLevelStatus(status, live, config));
	const freq = $derived(live?.frequency_hz ?? status?.frequency_hz ?? null);
	const dir = $derived(live?.direction ?? status?.direction ?? null);
	const sweep = $derived(sweepWords(status));
	const unit = $derived(live?.channels.find((c) => c.role === 'control')?.unit ?? 'g');
	const err = $derived(errorWords(live, unit));
	const progress = $derived(progressOf(status));
	const level = $derived(live?.level_db ?? null);
	const commanded = $derived(live?.commanded_db ?? status?.level_db ?? null);
	const trip = $derived(status?.end?.trip ?? null);
	const idWords = $derived(identificationWords(status?.identification ?? null));
	const law = $derived(status?.law ?? null);
	const errTone = $derived(live?.alarm ? 'warn' : 'pass');
</script>

<section class="levelcard" class:compact aria-label="Level" data-testid="sine-level-card">
	<div class="statusline {st.tone}" role="status" data-testid="sine-level-status">
		<span class="mk" aria-hidden="true">
			<svg viewBox="0 0 14 14" width="14" height="14">
				{#if st.mark === 'control'}
					<circle cx="7" cy="7" r="5.5" fill="currentColor" />
				{:else if st.mark === 'identify'}
					<circle cx="7" cy="7" r="5" fill="none" stroke="currentColor" stroke-width="1.5" /><circle cx="7" cy="7" r="1.8" fill="currentColor" />
				{:else if st.mark === 'hold'}
					<rect x="3.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" /><rect x="8.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" />
				{:else if st.mark === 'ramp'}
					<path d="M2 11L12 3v8z" fill="currentColor" />
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

	<div class="freq" data-testid="sine-frequency" data-hz={freq ?? ''} data-direction={dir ?? ''} data-held={status?.held ? 'true' : 'false'}>
		<span class="big num">{freq === null ? '-' : hz(freq)}</span>
		{#if dir && status?.test === 'sweep'}
			<span class="dir" title={dir === 'up' ? 'Sweeping up' : 'Sweeping down'}>
				<svg viewBox="0 0 12 12" width="14" height="14" aria-hidden="true">
					{#if dir === 'up'}<path d="M6 2v8M2.5 5.5L6 2l3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" />{:else}<path d="M6 10V2M2.5 6.5L6 10l3.5-3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" />{/if}
				</svg>
				<span class="visually-hidden">{dir}</span>
			</span>
		{/if}
		{#if status?.held}<span class="tag">Held</span>{/if}
	</div>
	{#if sweep}<p class="line" data-testid="sine-sweep-words">{sweep}</p>{/if}

	<dl class="nums">
		<div>
			<dt>Level</dt>
			<dd class="num" data-testid="sine-level-words">{db(level ?? commanded)}{#if level !== null && commanded !== null && Math.abs(level - commanded) > 0.05}<span class="sub"> to {db(commanded)}</span>{/if}</dd>
		</div>
		<div><dt>Drive</dt><dd class="num" data-testid="sine-drive">{live?.drive_vpk === null || live?.drive_vpk === undefined ? '-' : `${formatNumber(live.drive_vpk, 4)} V pk`}{#if live?.cola_vpk !== null && live?.cola_vpk !== undefined}<span class="sub">, reference {formatNumber(live.cola_vpk, 3)} V pk</span>{/if}</dd></div>
	</dl>
	{#if err}<p class="line err {errTone}" data-testid="sine-error-words">{err}</p>{/if}

	{#if progress !== null}
		<div class="progress" data-testid="sine-progress" data-progress={progress.toFixed(3)}>
			<div class="bar" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow={Math.round(progress * 100)} aria-label="Schedule done"><span style:width="{progress * 100}%"></span></div>
			<span class="aq-small aq-muted">{clock(status?.elapsed_s)} of test time, {clock(status?.remaining_s)} left</span>
		</div>
	{:else if status?.elapsed_s !== null && status?.elapsed_s !== undefined}
		<p class="aq-small aq-muted">{clock(status.elapsed_s)} of test time</p>
	{/if}

	{#if law && !compact}
		<p class="aq-small law" data-testid="sine-law-counts">{law.name}: {law.misses === 0 ? 'every call answered' : `${law.misses} unusable ${law.misses === 1 ? 'call' : 'calls'} in a row`}, a cap bound on {law.capped_ticks} {law.capped_ticks === 1 ? 'tick' : 'ticks'}</p>
	{/if}
	{#if idWords && !compact}<p class="aq-small aq-muted" data-testid="sine-identification-facts">{idWords}</p>{/if}

	{#if status?.end}
		<div class="end" data-testid="sine-end">
			<p class="aq-small">{status.end.message}</p>
			{#if status.end.ramp_s !== null}<p class="aq-small aq-muted">The drive ramped down in {formatNumber(status.end.ramp_s, 3)} s of station time.</p>{/if}
			{#if trip}
				<h4>{SINE_TRIP_WORDS[trip.kind]}</h4>
				<dl class="facts" data-testid="sine-trip">
					{#each sineTripFacts(trip) as f (f.label)}<div><dt>{f.label}</dt><dd class="num">{f.value}</dd></div>{/each}
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
		gap: 6px;
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
		color: var(--text-1);
	}

	.statusline .mk {
		display: inline-flex;
	}

	.statusline.pass .mk {
		color: var(--blue-2);
	}

	.statusline.warn .mk,
	.statusline.fail .mk {
		color: var(--amber-1);
	}

	.statusline.info .mk {
		color: var(--text-3);
	}

	.words {
		flex: 1;
		min-width: 0;
	}

	.mode {
		font-size: var(--text-2xs);
		font-weight: var(--weight-medium);
		color: var(--text-3);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	.freq {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
	}

	.big {
		font-size: 28px;
		font-weight: var(--weight-strong);
		color: var(--text-1);
		letter-spacing: -0.01em;
	}

	.compact .big {
		font-size: 22px;
	}

	.dir {
		display: inline-flex;
		color: var(--text-2);
	}

	.tag {
		padding: 1px 6px;
		font-size: var(--text-2xs);
		font-weight: var(--weight-strong);
		color: var(--text-2);
		border: 1px solid var(--line-2);
		border-radius: 8px;
		text-transform: uppercase;
	}

	.line {
		margin: 0;
		font-size: var(--text-sm);
		color: var(--text-2);
	}

	.err.warn {
		color: var(--amber-1);
		font-weight: var(--weight-strong);
	}

	.nums,
	.facts {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: var(--sp-2);
		margin: 0;
	}

	.nums dt,
	.facts dt {
		font-size: var(--text-2xs);
		color: var(--text-3);
	}

	.nums dd,
	.facts dd {
		margin: 0;
		font-size: var(--text-sm);
	}

	.sub {
		color: var(--text-3);
	}

	.progress {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.bar {
		height: 6px;
		background: var(--bg-2);
		border: 1px solid var(--line-1);
		border-radius: 3px;
		overflow: hidden;
	}

	.bar span {
		display: block;
		height: 100%;
		background: var(--blue-2);
	}

	.law {
		margin: 0;
		color: var(--text-2);
	}

	.end {
		padding-top: var(--sp-2);
		border-top: 1px solid var(--line-1);
	}

	.end p {
		margin: 0 0 4px;
	}

	h4 {
		margin: var(--sp-2) 0 4px;
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
	}

	p {
		margin: 0;
	}
</style>
