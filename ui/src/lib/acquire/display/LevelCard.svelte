<script lang="ts">
	/**
	 * The level at a glance: what the run is doing in plain words, the level the drive is at (and
	 * the one it is heading for while it ramps), the target and the measured control rms side by
	 * side with their difference, and how long the run has held this level against the time the
	 * schedule gives it. Any alarm holding now is named under it. The schedule's full ladder stays
	 * below, smaller. Every number is the service's; shape and word carry each state, color only
	 * follows.
	 */
	import type { RandomLiveHead, RandomState } from '$lib/protocol/types';
	import StatusMark from '$lib/viz/StatusMark.svelte';
	import { formatNumber } from '$lib/viz/format';
	import { alarmText, clock, db, levelStatus, ramping } from '../words';

	interface Props {
		run: RandomState | null;
		frame: RandomLiveHead | null;
		/** Channel names for an overload alarm. */
		channelName?: (index: number) => string;
		/** A narrow window's card: the numbers without the ladder. */
		compact?: boolean;
	}
	let { run, frame, channelName, compact = false }: Props = $props();

	const f = $derived(frame && run && frame.run_id === run.run_id ? frame : null);
	const status = $derived(levelStatus(run, f));
	const driving = $derived(run?.state === 'running' || run?.state === 'stopping');
	const identifying = $derived(run?.phase === 'sysid' || run?.phase === 'coherence_gate');
	const now = $derived(driving && !identifying ? (f?.level_db ?? null) : null);
	const moving = $derived(ramping(f) && !identifying);
	const progress = $derived.by(() => {
		if (!f || f.at_level_s === null || identifying) return null;
		const total = f.level_s;
		const frac = total && total > 0 ? Math.min(1, Math.max(0, f.at_level_s / total)) : null;
		return { at: f.at_level_s, total, left: f.remaining_at_level_s, frac };
	});
	const unit = $derived(f?.channels.find((c) => c.role === 'control')?.unit ?? 'g');
	/** As the service sent it: no digits it did not give. */
	const rms = (v: number | null | undefined) => (v === null || v === undefined ? '-' : formatNumber(v, 4));

	const levels = $derived(run?.levels_db ?? []);
	const commanded = $derived(identifying ? null : (run?.level_db ?? null));
	function rung(l: number): 'done' | 'now' | 'next' {
		if (commanded === null) return 'next';
		if (Math.abs(l - commanded) < 0.01) return 'now';
		return l < commanded ? 'done' : 'next';
	}
</script>

<section class="levelcard" class:compact aria-label="Level" data-testid="level-card" data-mark={status.mark}>
	<div class="statusline {status.tone}" role="status" data-testid="level-status">
		<span class="mk" aria-hidden="true">
			<svg viewBox="0 0 14 14" width="14" height="14">
				{#if status.mark === 'control'}
					<circle cx="7" cy="7" r="5.5" fill="currentColor" />
				{:else if status.mark === 'ramp'}
					<path d="M2 11.5L7 4l5 7.5z" fill="currentColor" />
				{:else if status.mark === 'identify'}
					<circle cx="7" cy="7" r="5" fill="none" stroke="currentColor" stroke-width="1.5" /><circle cx="7" cy="7" r="1.8" fill="currentColor" />
				{:else if status.mark === 'hold'}
					<rect x="3.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" /><rect x="8.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" />
				{:else if status.mark === 'stop'}
					<path d="M2 3.5L7 11l5-7.5z" fill="currentColor" />
				{:else if status.mark === 'done'}
					<circle cx="7" cy="7" r="6" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M4.2 7.2l1.9 1.9 3.8-4" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" />
				{:else if status.mark === 'ended'}
					<rect x="2.5" y="2.5" width="9" height="9" rx="1.3" fill="currentColor" />
				{:else if status.mark === 'armed'}
					<circle cx="7" cy="7" r="5" fill="none" stroke="currentColor" stroke-width="1.5" /><path d="M7 2a5 5 0 010 10z" fill="currentColor" />
				{:else}
					<circle cx="7" cy="7" r="5" fill="none" stroke="currentColor" stroke-width="1.4" stroke-dasharray="2.2 2" />
				{/if}
			</svg>
		</span>
		<span class="words">{status.text}</span>
		{#if run?.mode && driving}<span class="mode">{run.mode === 'manual' ? 'Manual ladder' : 'Automatic ladder'}</span>{/if}
	</div>

	<div class="now">
		<div class="big num" data-testid="level-now">{now === null ? '-' : db(now)}</div>
		<div class="sub">
			{#if moving}
				<span class="to" data-testid="level-commanded">to <span class="num">{db(f?.commanded_db)}</span></span>
			{:else if identifying}
				<span class="aq-muted">Not at test level</span>
			{:else if now === null}
				<span class="aq-muted">Not driving</span>
			{:else}
				<span class="aq-muted">Level</span>
			{/if}
		</div>
	</div>

	<dl class="rms">
		<div>
			<dt>Target <span class="u">{unit} rms</span></dt>
			<dd class="num" data-testid="level-target">{rms(f?.reference_rms_g)}</dd>
		</div>
		<div>
			<dt>Control <span class="u">{unit} rms</span></dt>
			<dd class="num" data-testid="level-control">{rms(f?.control_rms_g)}</dd>
		</div>
		<div>
			<dt>Difference</dt>
			<dd class="num" data-testid="level-error">{db(f?.control_error_db, 2)}</dd>
		</div>
	</dl>

	{#if progress}
		<div class="progress" data-testid="level-progress">
			{#if progress.frac !== null}
				<div class="bar" role="progressbar" aria-label="Time at this level" aria-valuemin={0} aria-valuemax={progress.total ?? 0} aria-valuenow={progress.at}>
					<span style:width="{progress.frac * 100}%"></span>
				</div>
				<p class="num">{clock(progress.at)} of {clock(progress.total)} at this level{progress.left !== null ? `, ${clock(progress.left)} left` : ''}</p>
			{:else}
				<p class="num">{clock(progress.at)} at this level{run?.mode === 'manual' ? ', no set time on the manual ladder' : ''}</p>
			{/if}
		</div>
	{/if}

	{#if f?.alarms.length}
		<ul class="alarms" data-testid="level-alarms">
			{#each f.alarms as a, i (i)}
				<li><StatusMark status="warn" word={false} /><span>Alarm: {alarmText(a, channelName)}</span></li>
			{/each}
		</ul>
	{/if}

	{#if !compact && levels.length}
		<ol class="rungs" aria-label="Level schedule" data-testid="level-ladder">
			{#each levels as l, i (i)}
				{@const s = rung(l)}
				<li class={s} aria-current={s === 'now' ? 'step' : undefined}>
					<span class="rk" aria-hidden="true">
						{#if s === 'done'}
							<svg viewBox="0 0 12 12" width="10" height="10"><path d="M2.5 6.3l2.3 2.3 4.7-5" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" /></svg>
						{:else if s === 'now'}
							<svg viewBox="0 0 12 12" width="10" height="10"><path d="M6 1l5 5-5 5-5-5z" fill="currentColor" /></svg>
						{:else}
							<svg viewBox="0 0 12 12" width="10" height="10"><path d="M6 1.5l4.5 4.5L6 10.5 1.5 6z" fill="none" stroke="currentColor" stroke-width="1.3" /></svg>
						{/if}
					</span>
					<span class="num">{db(l)}</span>
					{#if i === levels.length - 1}<span class="full">Full</span>{/if}
				</li>
			{/each}
		</ol>
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

	.statusline .words {
		flex: 1;
		min-width: 0;
	}

	.mode {
		font-size: var(--text-2xs);
		font-weight: var(--weight-medium);
		color: var(--text-3);
		white-space: nowrap;
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

	.to {
		font-weight: var(--weight-strong);
		color: var(--blue-1);
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

	.u {
		margin-left: 2px;
		font-weight: var(--weight-regular);
		letter-spacing: 0;
		text-transform: none;
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

	.rungs {
		display: flex;
		flex-wrap: wrap;
		gap: 3px;
		margin: 2px 0 0;
		padding: var(--sp-2) 0 0;
		list-style: none;
		border-top: 1px solid var(--line-1);
	}

	.rungs li {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		height: 22px;
		padding: 0 6px;
		font-size: var(--text-2xs);
		color: var(--text-3);
		border: 1px solid var(--line-2);
		border-radius: var(--radius-sm);
	}

	.rungs li.done {
		color: var(--text-2);
	}

	.rungs li.now {
		color: var(--text-1);
		font-weight: var(--weight-strong);
		border-color: var(--blue-2);
		background: var(--blue-weak);
	}

	.rk {
		display: inline-flex;
	}

	.full {
		color: var(--text-3);
	}
</style>
