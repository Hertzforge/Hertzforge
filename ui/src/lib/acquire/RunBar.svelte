<script lang="ts">
	/**
	 * The run bar: where the data comes from, the run's state, phase, clock and level, who holds
	 * control, and the verbs that apply now. Stop and Abort are offered to every screen while a
	 * run is live; everything else needs control. A verb that cannot run says why, in words, when
	 * it is focused or hovered, and the next step's reason is always shown.
	 */
	import Icon from '$lib/shell/Icon.svelte';
	import ProvenanceBadge from '$lib/shell/ProvenanceBadge.svelte';
	import type { Snippet } from 'svelte';
	import type { ActionId, RunBarHost } from './actions';
	import StateChip from './StateChip.svelte';
	import { clock, db, phaseWord } from './words';

	interface Props {
		acq: RunBarHost;
		/** Open the acknowledgement dialog for arming past a feasibility exceedance. */
		onoverride: () => void;
		abortKeys: string;
		/**
		 * A window that shows plots on another monitor: the run's state, the level, who holds
		 * control, and Stop and Abort. The verbs that set up and steer the run stay with the main
		 * window.
		 */
		compact?: boolean;
		/** A test type's own verbs, after the shared ones (classical shock's Fire and level moves). */
		verbs?: Snippet;
		/** A test type's own numbers in place of the time at level (classical shock's pulse counts). */
		nums?: Snippet;
	}
	let { acq, onoverride, abortKeys, compact = false, verbs, nums }: Props = $props();

	const run = $derived(acq.run);
	const frame = $derived(acq.frame);
	const a = $derived(acq.actions);
	const primary = $derived(acq.primary);
	const levelNow = $derived(frame?.level_db ?? null);
	const levelCommanded = $derived(run?.level_db ?? null);
	const atLevel = $derived(frame?.at_level_s ?? run?.at_level_s ?? null);
	const remaining = $derived(frame?.remaining_at_level_s ?? run?.remaining_at_level_s ?? null);

	/** The verb under the pointer or the keyboard focus; its reason is read live from the state. */
	let focusId = $state<ActionId | null>(null);
	let pressedId = $state<ActionId | null>(null);
	let stepDb = $state(1);

	const hint = $derived.by(() => {
		for (const id of [pressedId, focusId]) if (id && a[id].visible && !a[id].enabled) return a[id].reason;
		if (primary && a[primary].visible && !a[primary].enabled) return a[primary].reason;
		return null;
	});

	// The orphan countdown ticks on this screen between the service's events.
	let now = $state(Date.now());
	// Before the countdown is drawn: `now` is otherwise the last tick's, which may be long ago.
	$effect.pre(() => {
		if (acq.orphanDeadline === null || !acq.isLive) return;
		now = Date.now();
		const t = setInterval(() => (now = Date.now()), 250);
		return () => clearInterval(t);
	});
	// The countdown only means something while a run is live.
	const orphanLeft = $derived(acq.orphanDeadline === null || !acq.isLive ? null : Math.max(0, Math.ceil((acq.orphanDeadline - now) / 1000)));

	const seat = $derived(acq.session?.controller ?? null);
	const canClaim = $derived(acq.connected && !acq.isController && (!seat || !seat.present));

	function run_(id: ActionId, fn: () => unknown) {
		if (!a[id].enabled) {
			pressedId = id;
			return;
		}
		pressedId = null;
		void fn();
	}

	function tip(id: ActionId) {
		const leave = () => {
			if (focusId === id) focusId = null;
			if (pressedId === id) pressedId = null;
		};
		return { onfocus: () => (focusId = id), onblur: leave, onmouseenter: () => (focusId = id), onmouseleave: leave };
	}

	const busy = (verb: string) => acq.busy === verb;
</script>

<section class="runbar" class:compact aria-label="Run control" data-testid="run-bar">
	<div class="status">
		{#if acq.source}<ProvenanceBadge source={acq.source.source} cls={acq.source.cls} size="lg" measured={acq.frame !== null} />{/if}
		<StateChip {run} words={acq.outcomeWords} stateWords={acq.stateWords} />
		{#if run && run.state !== 'unconfigured'}
			<span class="phase" data-testid="run-phase">{acq.phaseWords?.[run.phase] ?? phaseWord(run.phase)}</span>
		{/if}
		<dl class="nums">
			<div>
				<dt>Elapsed</dt>
				<dd class="num" data-testid="run-elapsed">{clock(acq.elapsed)}</dd>
			</div>
			{#if !acq.levelHidden}
				<div>
					<dt>Level</dt>
					<dd class="num" data-testid="run-level">
						{#if levelNow !== null}
							{db(levelNow)}{#if levelCommanded !== null && Math.abs(levelCommanded - levelNow) > 0.05}<span class="to" title="Commanded level">{' '}to {db(levelCommanded)}</span>{/if}
						{:else}
							{db(levelCommanded)}
						{/if}
					</dd>
				</div>
			{/if}
			{#if nums}
				{@render nums()}
			{:else}
				<div>
					<dt>At Level</dt>
					<dd class="num">{clock(atLevel)}{#if remaining !== null}<span class="left">, {clock(remaining)} left</span>{/if}</dd>
				</div>
			{/if}
			{#if run?.run_id}
				<div class="runid">
					<dt>Run</dt>
					<dd><span class="num">{run.run_id}</span>{#if run.title}<span class="title" title={run.title}>{run.title}</span>{/if}</dd>
				</div>
			{/if}
		</dl>

		<div class="seat" data-testid="session">
			{#if acq.isController}
				<span class="role mine" data-testid="session-role" data-role="controller">
					<svg viewBox="0 0 14 14" width="13" height="13" aria-hidden="true"><circle cx="5" cy="7" r="3.2" fill="currentColor" /><path d="M8 7h5M11 7v2.5M13 7v2" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" /></svg>
					In Control
				</span>
				{#if !compact}<button class="btn sm" data-testid="release" onclick={() => acq.release()} disabled={!acq.connected || acq.busy !== null} title={acq.isLive ? `Hand control to another screen. The run continues; if nobody takes control within ${acq.session?.timeout_s ?? 30} s, it stops with its ramp.` : 'Let another screen take control.'}>Release</button>{/if}
			{:else}
				<span class="role" data-testid="session-role" data-role="observer">
					<svg viewBox="0 0 14 14" width="13" height="13" aria-hidden="true"><path d="M1 7s2.3-4 6-4 6 4 6 4-2.3 4-6 4-6-4-6-4z" fill="none" stroke="currentColor" stroke-width="1.3" /><circle cx="7" cy="7" r="1.8" fill="currentColor" /></svg>
					<span>Observing{#if seat}<span class="holder">, {seat.name} {seat.present ? 'has control' : 'held control and is disconnected'}</span>{/if}</span>
				</span>
				{#if !compact}<button class="btn sm primary" data-testid="claim" onclick={() => acq.claim()} disabled={!canClaim || busy('Take Control')} title={canClaim ? 'Take control of the station and the run.' : seat?.present ? `${seat.name} has control. Control passes when it is released.` : 'The control service is not connected.'}>
					Take Control
				</button>{/if}
			{/if}
			{#if orphanLeft !== null}
				<span class="orphan" role="alert" data-testid="orphan">
					<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><path d="M7 1.6l5.6 10H1.4z" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round" /><path d="M7 5.4v3" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
					Nobody holds control. The run stops with its ramp in <span class="num">{orphanLeft}&nbsp;s</span>
				</span>
			{/if}
		</div>
	</div>

	<div class="actions">
		{#snippet verb(id: ActionId, label: string, icon: 'play' | 'stop' | 'reset' | 'action' | null, onclick: () => unknown, testid: string)}
			{@const s = a[id]}
			{#if s.visible}
				<button class="btn" class:primary={primary === id} data-testid={testid} aria-disabled={!s.enabled} class:off={!s.enabled} onclick={() => run_(id, onclick)} {...tip(id)}>
					{#if acq.busy === label}
						<svg class="spin" viewBox="0 0 12 12" width="12" height="12" aria-hidden="true"><path d="M6 1.5a4.5 4.5 0 104.5 4.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" /></svg>
					{:else if icon}
						<Icon name={icon} size={13} />
					{/if}
					{label}
				</button>
			{/if}
		{/snippet}

		{#if !compact}
		<div class="verbs">
			{@render verb('apply', 'Apply Setup', 'action', () => acq.apply(), 'act-apply')}
			{@render verb('arm', 'Arm', null, () => acq.arm(), 'act-arm')}
			{@render verb('override', 'Arm Past Feasibility', null, onoverride, 'act-override')}
			{@render verb('start', 'Start', 'play', () => acq.start(), 'act-start')}
			{@render verb('proceed', 'Proceed', 'play', () => acq.proceed(), 'act-proceed')}
			{@render verb('reset', 'New Run', 'reset', () => acq.reset(), 'act-reset')}
		</div>

		{#if verbs}{@render verbs()}{/if}

		{#if a.mode.visible}
			<div class="ladder">
				<span class="eyebrow">Ladder</span>
				<div class="aq-seg sm" role="radiogroup" aria-label="Level ladder" data-testid="mode">
					{#each [['auto', 'Automatic'], ['manual', 'Manual']] as [m, label] (m)}
						<button
							type="button"
							role="radio"
							data-testid="mode-{m}"
							aria-checked={run?.mode === m}
							aria-disabled={!a.mode.enabled}
							onclick={() => run_('mode', () => run?.mode !== m && acq.setMode(m as 'auto' | 'manual'))}
							{...tip('mode')}>{label}</button
						>
					{/each}
				</div>
			</div>
		{/if}

		{#if a.level.visible}
			<div class="level" role="group" aria-label="Level">
				<button class="btn icon" data-testid="level-down" aria-label="Level down {stepDb} dB" aria-disabled={!a.level.enabled} class:off={!a.level.enabled} onclick={() => run_('level', () => acq.level(-stepDb))} {...tip('level')}>
					<svg viewBox="0 0 12 12" width="12" height="12" aria-hidden="true"><path d="M2 6h8" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" /></svg>
				</button>
				<select class="field step num" aria-label="Level step" bind:value={stepDb} data-testid="level-step">
					{#each [0.5, 1, 2, 3] as s (s)}<option value={s}>{s} dB</option>{/each}
				</select>
				<button class="btn icon" data-testid="level-up" aria-label="Level up {stepDb} dB" aria-disabled={!a.level.enabled} class:off={!a.level.enabled} onclick={() => run_('level', () => acq.level(stepDb))} {...tip('level')}>
					<svg viewBox="0 0 12 12" width="12" height="12" aria-hidden="true"><path d="M2 6h8M6 2v8" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" /></svg>
				</button>
				<select
					class="field goto num"
					aria-label="Go to a level of the schedule"
					data-testid="level-goto"
					value=""
					disabled={!a.level.enabled}
					title={a.level.reason ?? 'Go to a level of the schedule'}
					onchange={(e) => {
						const v = Number(e.currentTarget.value);
						e.currentTarget.value = '';
						if (Number.isFinite(v)) void acq.goTo(v);
					}}
				>
					<option value="" disabled>Go to level</option>
					{#each run?.levels_db ?? [] as l (l)}<option value={l}>{db(l)}</option>{/each}
				</select>
			</div>
		{/if}

		{/if}

		<div class="ends">
			{#if a.stop.visible}
				<button class="btn stop" data-testid="act-stop" aria-disabled={!a.stop.enabled} class:off={!a.stop.enabled} onclick={() => run_('stop', () => acq.stop())} {...tip('stop')}>
					<svg viewBox="0 0 16 16" width="15" height="15" aria-hidden="true"><path d="M2 4h4l8 8" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" /></svg>
					<span class="two"><span>Stop</span><small>Ramped</small></span>
				</button>
			{/if}
			{#if a.abort.visible}
				<button class="btn abort" data-testid="act-abort" aria-disabled={!a.abort.enabled} class:off={!a.abort.enabled} aria-keyshortcuts={abortKeys.replace('Cmd', 'Meta').replace('Ctrl', 'Control').replace(' ', '+')} onclick={() => run_('abort', () => acq.abort())} {...tip('abort')}>
					<svg viewBox="0 0 14 14" width="16" height="16" aria-hidden="true"><path d="M4.5 1h5L13 4.5v5L9.5 13h-5L1 9.5v-5z" fill="currentColor" /><path d="M4.8 7h4.4" stroke="var(--status-fail-fill)" stroke-width="1.8" stroke-linecap="round" /></svg>
					<span class="two"><span>Abort</span><small>Immediate</small></span>
					<kbd>{abortKeys}</kbd>
				</button>
			{/if}
		</div>
	</div>

	{#if hint}
		<p class="hint" data-testid="run-hint" aria-live="polite">
			<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><circle cx="7" cy="7" r="6" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M7 6.2v4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /><circle cx="7" cy="4.1" r="0.9" fill="currentColor" /></svg>
			{hint}
		</p>
	{/if}
</section>

<style>
	.runbar {
		--abort-ink: var(--status-fail-text);
		display: flex;
		flex-direction: column;
		background: var(--bg-1);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-lg);
	}

	.status {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2) var(--sp-3);
		padding: 10px var(--sp-4);
		border-bottom: 1px solid var(--line-1);
	}

	.phase {
		font-size: var(--text-sm);
		color: var(--text-2);
		white-space: nowrap;
	}

	.nums {
		display: flex;
		flex-wrap: wrap;
		gap: 4px var(--sp-5);
		margin: 0 auto 0 var(--sp-2);
		min-width: 0;
	}

	/* A test type's own numbers come from its screen, so the rules reach them globally. */
	.nums > :global(div) {
		display: flex;
		align-items: baseline;
		gap: 6px;
		min-width: 0;
	}

	.nums :global(dt) {
		font-size: var(--text-2xs);
		color: var(--text-3);
		white-space: nowrap;
	}

	.nums :global(dd) {
		margin: 0;
		font-size: var(--text-sm);
		color: var(--text-1);
		white-space: nowrap;
	}

	.to,
	.left {
		color: var(--text-3);
	}

	.runid dd {
		display: flex;
		gap: 6px;
		min-width: 0;
	}

	.runid .title {
		max-width: 22ch;
		overflow: hidden;
		text-overflow: ellipsis;
		color: var(--text-2);
		font-size: var(--text-xs);
	}

	.seat {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2);
	}

	.role {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		font-size: var(--text-sm);
		color: var(--text-2);
	}

	.role.mine {
		color: var(--blue-1);
		font-weight: var(--weight-strong);
	}

	.holder {
		color: var(--text-3);
	}

	.orphan {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		padding: 3px 8px;
		font-size: var(--text-xs);
		color: var(--text-1);
		background: var(--amber-weak);
		border: 1px solid var(--amber-line);
		border-radius: var(--radius-md);
	}

	.orphan svg {
		color: var(--amber-2);
	}

	.actions {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2) var(--sp-4);
		padding: 10px var(--sp-4);
	}

	.verbs {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-2);
	}

	.verbs:empty {
		display: none;
	}

	.btn.off {
		opacity: 0.45;
		cursor: default;
	}

	.btn.primary {
		min-width: 92px;
	}

	.ladder,
	.level {
		display: flex;
		align-items: center;
		gap: 6px;
	}

	.ladder .eyebrow {
		margin-right: 2px;
	}

	.level .field {
		height: var(--control-height);
		width: auto;
		font-size: var(--text-xs);
	}

	.level .step {
		min-width: 74px;
	}

	.level .goto {
		min-width: 118px;
	}

	.ends {
		display: flex;
		gap: var(--sp-2);
		margin-left: auto;
	}

	.two {
		display: inline-flex;
		flex-direction: column;
		align-items: flex-start;
		line-height: 1.05;
	}

	.two small {
		font-size: var(--text-2xs);
		font-weight: var(--weight-regular);
		opacity: 0.8;
	}

	.stop,
	.abort {
		height: 38px;
		padding: 0 14px 0 11px;
	}

	.stop:not(.off) {
		color: var(--amber-1);
		background: var(--amber-weak);
		border-color: var(--amber-line);
	}

	.abort {
		min-width: 132px;
		font-weight: var(--weight-strong);
		color: var(--abort-ink);
		background: var(--status-fail-fill);
		border: 2px solid var(--amber-1);
	}

	.abort:hover:not(.off) {
		background: var(--amber-1) !important;
		border-color: var(--amber-1) !important;
	}

	.abort kbd {
		margin-left: 4px;
		color: var(--abort-ink);
		background: transparent;
		border-color: color-mix(in srgb, var(--abort-ink) 40%, transparent);
	}

	.hint {
		display: flex;
		align-items: center;
		gap: 6px;
		margin: -2px var(--sp-4) 10px;
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	.hint svg {
		flex: none;
		color: var(--text-3);
	}

	.spin {
		animation: spin 0.8s linear infinite;
	}

	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}

	@media (max-width: 900px) {
		.nums {
			order: 3;
			width: 100%;
			margin-left: 0;
		}
		.ends {
			width: 100%;
			margin-left: 0;
		}
		.ends .btn {
			flex: 1;
		}
	}
</style>
