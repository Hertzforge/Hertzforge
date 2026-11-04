<script lang="ts">
	/**
	 * The MIMO level at a glance, in the single-output card's form: what the run is doing in plain
	 * words, the level it is at, how long it has held this level against the schedule, how the
	 * controls stand against their bands (how many beyond alarm and abort, and the largest error),
	 * and the identification gates. Every number is the service's; shape and word carry each
	 * state, color only follows.
	 */
	import type { MimoStatus } from '$lib/protocol/mimo';
	import StatusMark from '$lib/viz/StatusMark.svelte';
	import { formatDb, formatNumber } from '$lib/viz/format';
	import { bandClass, DEFAULT_BAND, type RowBand } from '$lib/viz/scales';
	import { clock } from '../../words';
	import { mimoLevelStatus } from './words';

	interface Props {
		status: MimoStatus | null;
		bands?: RowBand[];
		compact?: boolean;
	}
	let { status, bands = [], compact = false }: Props = $props();

	const st = $derived(mimoLevelStatus(status));
	const driving = $derived(status?.phase === 'engaged' || status?.phase === 'full_level' || status?.phase === 'ramping_down');
	const errors = $derived(status?.control_error_db ?? []);
	const counts = $derived.by(() => {
		let alarm = 0;
		let abort = 0;
		let worst: { i: number; e: number } | null = null;
		errors.forEach((e, i) => {
			if (!Number.isFinite(e)) return;
			const k = bandClass(e, bands[i] ?? DEFAULT_BAND);
			if (k >= 1) alarm++;
			if (k === 2) abort++;
			if (!worst || Math.abs(e) > Math.abs(worst.e)) worst = { i, e };
		});
		return { alarm, abort, worst: worst as { i: number; e: number } | null };
	});
	const progress = $derived.by(() => {
		if (!status || status.at_level_s === null || !driving) return null;
		const total = status.remaining_at_level_s !== null ? status.at_level_s + status.remaining_at_level_s : null;
		return { at: status.at_level_s, total, left: status.remaining_at_level_s, frac: total && total > 0 ? Math.min(1, status.at_level_s / total) : null };
	});
	const names = $derived(status?.shape?.labels ?? []);
	const gates = $derived(status?.gates ?? null);
</script>

<section class="levelcard" class:compact aria-label="Level" data-testid="mimo-level-card">
	<div class="statusline {st.tone}" role="status" data-testid="mimo-level-status">
		<span class="mk" aria-hidden="true">
			<svg viewBox="0 0 14 14" width="14" height="14">
				{#if st.mark === 'control'}
					<circle cx="7" cy="7" r="5.5" fill="currentColor" />
				{:else if st.mark === 'ramp'}
					<path d="M2 11.5L7 4l5 7.5z" fill="currentColor" />
				{:else if st.mark === 'identify'}
					<circle cx="7" cy="7" r="5" fill="none" stroke="currentColor" stroke-width="1.5" /><circle cx="7" cy="7" r="1.8" fill="currentColor" />
				{:else if st.mark === 'hold'}
					<rect x="3.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" /><rect x="8.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" />
				{:else if st.mark === 'stop'}
					<path d="M2 3.5L7 11l5-7.5z" fill="currentColor" />
				{:else if st.mark === 'ended'}
					<rect x="2.5" y="2.5" width="9" height="9" rx="1.3" fill="currentColor" />
				{:else}
					<circle cx="7" cy="7" r="5" fill="none" stroke="currentColor" stroke-width="1.4" stroke-dasharray="2.2 2" />
				{/if}
			</svg>
		</span>
		<span class="words">{st.text}</span>
		{#if status?.loop}<span class="mode num">Loop {status.loop}</span>{/if}
	</div>

	<div class="now">
		<div class="big num" data-testid="mimo-level-now">{driving && status?.level_db !== null ? formatDb(status?.level_db) : '-'}</div>
		<div class="sub">{driving ? 'Level' : 'Not at test level'}</div>
	</div>

	<dl class="rms">
		<div class:warn={counts.alarm > 0}>
			<dt>Beyond alarm</dt>
			<dd class="num" data-testid="mimo-level-alarm">{errors.length ? `${counts.alarm} of ${errors.length}` : '-'}</dd>
		</div>
		<div class:fail={counts.abort > 0}>
			<dt>Beyond abort</dt>
			<dd class="num" data-testid="mimo-level-abort">{errors.length ? counts.abort : '-'}</dd>
		</div>
		<div>
			<dt>Largest error</dt>
			<dd class="num" data-testid="mimo-level-worst" title={counts.worst ? names[counts.worst.i] : undefined}>{counts.worst ? formatDb(counts.worst.e, 2) : '-'}</dd>
		</div>
	</dl>
	{#if counts.worst && names[counts.worst.i]}
		<p class="who">Largest at {names[counts.worst.i]}</p>
	{/if}

	{#if progress}
		<div class="progress" data-testid="mimo-level-progress">
			{#if progress.frac !== null}
				<div class="bar" role="progressbar" aria-label="Time at this level" aria-valuemin={0} aria-valuemax={progress.total ?? 0} aria-valuenow={progress.at}>
					<span style:width="{progress.frac * 100}%"></span>
				</div>
				<p class="num">{clock(progress.at)} of {clock(progress.total)} at this level{progress.left !== null ? `, ${clock(progress.left)} left` : ''}</p>
			{:else}
				<p class="num">{clock(progress.at)} at this level</p>
			{/if}
		</div>
	{/if}

	{#if status?.alarm}
		<ul class="alarms" data-testid="mimo-level-alarms">
			<li><StatusMark status="warn" word={false} /><span>Alarm: {counts.alarm} {counts.alarm === 1 ? 'control' : 'controls'} outside the alarm band</span></li>
		</ul>
	{/if}

	{#if gates && !compact}
		<dl class="gates" data-testid="mimo-level-gates">
			<div>
				<dt>Smallest multiple coherence</dt>
				<dd class="num"><StatusMark status={gates.coherence_min >= gates.coherence_floor ? 'pass' : 'fail'} word={false} />{formatNumber(gates.coherence_min, 3)} <span class="lim">floor {formatNumber(gates.coherence_floor, 3)}</span></dd>
			</div>
			<div>
				<dt>Largest condition number</dt>
				<dd class="num"><StatusMark status={gates.condition_max <= gates.condition_limit ? 'pass' : 'fail'} word={false} />{formatNumber(gates.condition_max, 4)} <span class="lim">limit {formatNumber(gates.condition_limit, 4)}</span></dd>
			</div>
		</dl>
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
		gap: 7px;
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
		color: var(--text-1);
	}

	.statusline .mk {
		display: inline-flex;
		color: var(--text-2);
	}

	.statusline.pass .mk {
		color: var(--blue-2);
	}

	.statusline.fail .mk,
	.statusline.warn .mk {
		color: var(--amber-1);
	}

	.words {
		flex: 1;
		min-width: 0;
	}

	.mode {
		font-size: var(--text-2xs);
		font-weight: var(--weight-medium);
		color: var(--text-3);
	}

	.now {
		display: flex;
		align-items: baseline;
		gap: var(--sp-3);
	}

	.big {
		font-size: 34px;
		line-height: 1.05;
		font-weight: var(--weight-strong);
		letter-spacing: -0.01em;
		color: var(--text-1);
	}

	.compact .big {
		font-size: 26px;
	}

	.sub {
		font-size: var(--text-sm);
		color: var(--text-2);
	}

	.rms {
		display: grid;
		grid-template-columns: repeat(3, minmax(0, 1fr));
		gap: 4px;
		margin: 0;
	}

	.rms div {
		display: flex;
		flex-direction: column;
		padding: 5px 7px;
		background: var(--bg-2);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-sm);
	}

	.rms div.warn {
		border: 1px dashed var(--amber-2);
	}

	.rms div.fail {
		border: 2px solid var(--amber-2);
	}

	.rms dt {
		font-size: var(--text-2xs);
		font-weight: var(--weight-strong);
		letter-spacing: 0.05em;
		text-transform: uppercase;
		color: var(--text-3);
	}

	.rms dd {
		margin: 0;
		font-size: var(--text-lg);
		font-weight: var(--weight-strong);
		color: var(--text-1);
		white-space: nowrap;
	}

	.who {
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	.progress p {
		margin-top: 3px;
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	.bar {
		height: 10px;
		overflow: hidden;
		background: var(--bg-2);
		border: 1px solid var(--line-2);
		border-radius: 5px;
	}

	.bar span {
		display: block;
		height: 100%;
		background: var(--blue-2);
	}

	.alarms {
		margin: 0;
		padding: 0;
		list-style: none;
	}

	.alarms li {
		display: flex;
		align-items: center;
		gap: 6px;
		padding: 3px 6px;
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
		color: var(--text-1);
		background: var(--amber-weak);
		border: 1px dashed var(--amber-2);
		border-radius: var(--radius-sm);
	}

	.gates {
		display: flex;
		flex-direction: column;
		gap: 4px;
		margin: 0;
		font-size: var(--text-xs);
	}

	.gates div {
		display: flex;
		justify-content: space-between;
		gap: var(--sp-2);
	}

	.gates dt {
		color: var(--text-2);
	}

	.gates dd {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		margin: 0;
		color: var(--text-1);
	}

	.lim {
		color: var(--text-3);
	}
</style>
