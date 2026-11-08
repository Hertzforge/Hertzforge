<script lang="ts">
	/**
	 * The TWR run at a glance: what it is doing in plain words, the play in flight with its kind,
	 * level and rung and how far through the record it is, the iteration against the level's budget,
	 * the playback against its schedule, the last play's rms error against the gate, the drive in
	 * force against the ceiling, the identification's facts, and, once the run ends, how and why.
	 * Every number is the service's; shape and word carry each state, color only follows.
	 */
	import StatusMark from '$lib/viz/StatusMark.svelte';
	import { formatNumber } from '$lib/viz/format';
	import { db } from '../words';
	import type { TwrNow } from './twrStore.svelte';
	import { errorAgainstGate, iterationWords, lastPlayWords, levelWords, playbackWords, playProgress, playWords, twrLevelStatus, twrTripFacts, TWR_TRIP_WORDS } from './twrWords';

	interface Props {
		/** The state with the latest frame laid over it. */
		status: TwrNow | null;
		/** The gate the run holds each play to, percent. */
		gatePct: number | null;
		compact?: boolean;
	}
	let { status, gatePct, compact = false }: Props = $props();

	const st = $derived(twrLevelStatus(status, status?.identification?.excitation ?? 'random'));
	const running = $derived(status?.state === 'running' || status?.state === 'stopping');
	const inFlight = $derived(playWords(status));
	const progress = $derived(playProgress(status?.play));
	const iter = $derived(running && status?.phase !== 'sysid' && status?.level_db !== null ? iterationWords(status?.plays, status?.play) : null);
	const back = $derived(running ? playbackWords(status?.plays, status?.play) : null);
	const gate = $derived(errorAgainstGate(status?.last ?? null, gatePct));
	const lastWords = $derived(lastPlayWords(status?.last_play ?? null, status?.levels_db));
	const pred = $derived(status?.prediction ?? null);
	const id = $derived(status?.identification ?? null);
	const trip = $derived(status?.end?.trip ?? null);
	const rungWords = $derived(status?.rung !== null && status?.rung !== undefined && status.levels_db ? `rung ${status.rung + 1} of ${status.levels_db.length}` : null);
</script>

<section class="levelcard" class:compact aria-label="Level" data-testid="twr-level-card">
	<div class="statusline {st.tone}" role="status" data-testid="twr-level-status">
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
		<span class="words" data-testid="twr-level-words">{st.text}</span>
		{#if status?.mode}<span class="mode">{status.mode === 'auto' ? 'Automatic' : 'Manual'}</span>{/if}
	</div>

	{#if inFlight}<p class="big" data-testid="twr-play-words">{inFlight}</p>{/if}

	{#if progress !== null}
		<div class="progress" data-testid="twr-progress" data-progress={progress.toFixed(3)} data-frames={status?.play?.frames_done ?? ''}>
			<div class="bar" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow={Math.round(progress * 100)} aria-label="Through the record"><span style:width="{progress * 100}%"></span></div>
			<span class="aq-small aq-muted">{status?.play?.frames_done ?? 0} of {status?.play?.frames ?? 0} frames of the record</span>
		</div>
	{/if}

	{#if running && status?.level_db !== null && status?.level_db !== undefined}
		<dl class="nums" data-testid="twr-level-numbers">
			<div><dt>Level</dt><dd class="num">{levelWords(status.level_db, status.levels_db)}{#if rungWords}<span class="sub">, {rungWords}</span>{/if}</dd></div>
			<div><dt>Plays</dt><dd class="num">{status.plays.done} judged</dd></div>
			{#if iter}<div class="wide"><dt>At this level</dt><dd class="num" data-testid="twr-iteration-words">{iter}</dd></div>{/if}
			{#if back}<div class="wide"><dt>Playback</dt><dd class="num" data-testid="twr-playback-words">{back}</dd></div>{/if}
		</dl>
	{/if}

	{#if gate}
		<p class="line {gate.tone}" data-testid="twr-gate-words"><StatusMark status={gate.tone} word={false} /> Last play: {gate.text}</p>
	{/if}
	{#if lastWords && !compact}
		<p class="aq-small aq-muted" data-testid="twr-last-play">{lastWords.text}</p>
	{/if}
	{#if status?.frozen}
		<p class="line warn" data-testid="twr-frozen"><StatusMark status="warn" word={false} /> Corrections frozen: the drive in force plays again.</p>
	{/if}

	{#if pred && !compact}
		<dl class="nums pred" data-testid="twr-prediction">
			<div class="wide"><dt>Drive in force at {db(pred.level_db)}</dt><dd class="num">{formatNumber(pred.drive_vpk, 4)} V pk; ceiling {formatNumber(pred.drive_vpk_ceiling, 4)} V pk</dd></div>
		</dl>
	{/if}

	{#if id && !compact}
		<dl class="nums ident" data-testid="twr-identification-facts">
			<div><dt>Coherence</dt><dd class="num">{formatNumber(id.coherence_mean, 4)}<span class="sub">, holds under {formatNumber(id.min_mean_coherence, 3)}</span></dd></div>
			<div><dt>Coverage</dt><dd class="num">{formatNumber(id.coverage * 100, 4)}%</dd></div>
			<div><dt>Lag</dt><dd class="num">{formatNumber(id.lag_frames, 4)} frames, {id.lag_source}</dd></div>
			<div><dt>Drive</dt><dd class="num">{formatNumber(id.drive_vrms, 3)} V rms, {formatNumber(id.drive_vpk, 3)} V pk</dd></div>
		</dl>
	{/if}

	{#if status?.state === 'finished' && status.end}
		<div class="end" role="status" data-testid="twr-end">
			<p class="msg">{status.end.message}</p>
			{#if status.end.frames_done !== null}<p class="aq-small aq-muted">The station had taken {status.end.frames_done} frames of the play when it was cancelled.</p>{/if}
			{#if trip}
				<h4>{TWR_TRIP_WORDS[trip.kind]}</h4>
				<dl class="nums">
					{#each twrTripFacts(trip) as f (f.label)}<div><dt>{f.label}</dt><dd class="num">{f.value}</dd></div>{/each}
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

	.big {
		margin: 0;
		font-size: var(--text-lg);
		font-weight: var(--weight-strong);
		color: var(--text-1);
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

	.line {
		display: flex;
		align-items: center;
		gap: 6px;
		margin: 0;
		font-size: var(--text-sm);
	}

	.line.warn {
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

	.pred,
	.ident {
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
		margin: var(--sp-2) 0 4px;
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
	}

	p {
		margin: 0;
	}
</style>
