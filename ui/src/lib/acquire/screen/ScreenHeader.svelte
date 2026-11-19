<script lang="ts">
	/**
	 * A test screen's one header line: the way back to the test types, the test's name, its Setup
	 * and Run steps, the run's state (its reason on hover), the clock and the level, where the data
	 * comes from, and who holds control. Stop and Abort join it only while a run is live and the
	 * Run card is out of view, so they are always in reach.
	 */
	import ProvenanceBadge from '$lib/shell/ProvenanceBadge.svelte';
	import type { Snippet } from 'svelte';
	import type { RunBarHost } from '../actions';
	import StateChip from '../StateChip.svelte';
	import { clock, db, phaseWord } from '../words';

	interface Props {
		acq: RunBarHost;
		/** The test's name. */
		name: string;
		/** The step on show; null in a window with no steps (another monitor's). */
		step?: 'setup' | 'run' | null;
		onsetup?: () => void;
		onrun?: () => void;
		/** Why the Run step waits, or null. */
		runWhy?: string | null;
		/** Back to the test types; absent while the screen cannot be left. */
		onleave?: (() => void) | null;
		/** What the state chip says on hover, before the phase and the run's name. */
		reason?: string | null;
		/** Stop and Abort in the header: a run is live and the Run card is out of view. */
		ends?: boolean;
		abortKeys: string;
		/** A window on another monitor: no Take Control or Release. */
		compact?: boolean;
		/** A test type's own numbers in place of the time at level. */
		nums?: Snippet;
		/** The test's name, for the keyboard to land on. */
		heading?: HTMLElement | null;
	}
	let { acq, name, step = null, onsetup, onrun, runWhy = null, onleave = null, reason = null, ends = false, abortKeys, compact = false, nums, heading = $bindable(null) }: Props = $props();

	const run = $derived(acq.run);
	const frame = $derived(acq.frame);
	const levelNow = $derived(frame?.level_db ?? null);
	const levelCommanded = $derived(run?.level_db ?? null);
	const atLevel = $derived(frame?.at_level_s ?? run?.at_level_s ?? null);
	const remaining = $derived(frame?.remaining_at_level_s ?? run?.remaining_at_level_s ?? null);
	const liveDot = $derived(run?.state === 'running' || run?.state === 'review' || run?.state === 'stopping');
	const chipTitle = $derived.by(() => {
		const phase = run && run.state !== 'unconfigured' ? (acq.phaseWords?.[run.phase] ?? phaseWord(run.phase)) : null;
		const named = run?.run_id ? `Run ${run.run_id}${run.title ? `, ${run.title}` : ''}` : null;
		return [reason, phase, named].filter((x): x is string => !!x).map((x) => x.replace(/\.$/, '')).join('. ') || undefined;
	});

	// The orphan countdown ticks on this screen between the service's events.
	let now = $state(Date.now());
	$effect.pre(() => {
		if (acq.orphanDeadline === null || !acq.isLive) return;
		now = Date.now();
		const t = setInterval(() => (now = Date.now()), 250);
		return () => clearInterval(t);
	});
	const orphanLeft = $derived(acq.orphanDeadline === null || !acq.isLive ? null : Math.max(0, Math.ceil((acq.orphanDeadline - now) / 1000)));

	const seat = $derived(acq.session?.controller ?? null);
	const canClaim = $derived(acq.connected && !acq.isController && (!seat || !seat.present));
	const a = $derived(acq.actions);
</script>

<header class="head" data-testid="screen-header">
	<div class="where">
		{#if onleave}
			<button class="btn ghost sm back" data-testid="test-type-back" onclick={onleave}>
				<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M7.5 2.5L4 6l3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
				Test Type
			</button>
			<span class="sep" aria-hidden="true">/</span>
		{/if}
		<h1 class="here" tabindex="-1" bind:this={heading}>{name}</h1>
		{#if step}
			<ol class="flow" aria-label="Steps">
				<li>
					<button class="step" class:on={step === 'setup'} aria-current={step === 'setup' ? 'step' : undefined} data-testid="flow-setup" onclick={() => onsetup?.()}><span class="n">1</span>Setup</button>
				</li>
				<li aria-hidden="true" class="arrow">
					<svg viewBox="0 0 12 12" width="10" height="10"><path d="M4.5 2.5L8 6l-3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" /></svg>
				</li>
				<li>
					<button class="step" class:on={step === 'run'} aria-current={step === 'run' ? 'step' : undefined} data-testid="flow-run" aria-disabled={step !== 'run' && !!runWhy} title={step !== 'run' && runWhy ? runWhy : undefined} onclick={() => (step === 'run' ? null : onrun?.())}>
						<span class="n">2</span>Run{#if liveDot}<span class="dot" aria-label="running"><svg viewBox="0 0 10 10" width="8" height="8" aria-hidden="true"><circle cx="5" cy="5" r="4" fill="currentColor" /></svg></span>{/if}
					</button>
				</li>
			</ol>
		{/if}
	</div>

	<div class="state">
		<span class="chip" title={chipTitle}><StateChip {run} words={acq.outcomeWords} stateWords={acq.stateWords} /></span>
		<dl class="nums">
			<div class="opt2">
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
				<div class="opt">
					<dt>At Level</dt>
					<dd class="num" data-testid="run-at-level">{clock(atLevel)}{#if remaining !== null}<span class="to">, {clock(remaining)} left</span>{/if}</dd>
				</div>
			{/if}
		</dl>
	</div>

	<div class="seat" data-testid="session">
		{#if acq.source}<ProvenanceBadge source={acq.source.source} cls={acq.source.cls} compact measured={acq.frame !== null} />{/if}
		{#if orphanLeft !== null}
			<span class="orphan" role="alert" data-testid="orphan" title="Nobody holds control. The run stops with its ramp when the time runs out.">
				<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><path d="M7 1.6l5.6 10H1.4z" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round" /><path d="M7 5.4v3" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
				Nobody in control: stops in <span class="num">{orphanLeft}&nbsp;s</span>
			</span>
		{/if}
		{#if acq.isController}
			<span class="role mine" data-testid="session-role" data-role="controller">
				<svg viewBox="0 0 14 14" width="13" height="13" aria-hidden="true"><circle cx="5" cy="7" r="3.2" fill="currentColor" /><path d="M8 7h5M11 7v2.5M13 7v2" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" /></svg>
				In Control
			</span>
			{#if !compact}<button class="btn ghost sm" data-testid="release" onclick={() => acq.release()} disabled={!acq.connected || acq.busy !== null} title={acq.isLive ? `Hand control to another screen. The run continues; if nobody takes control within ${acq.session?.timeout_s ?? 30} s, it stops with its ramp.` : 'Let another screen take control.'}>Release</button>{/if}
		{:else}
			<span class="role" data-testid="session-role" data-role="observer" title={seat ? `${seat.name} ${seat.present ? 'has control' : 'held control and is disconnected'}` : undefined}>
				<svg viewBox="0 0 14 14" width="13" height="13" aria-hidden="true"><path d="M1 7s2.3-4 6-4 6 4 6 4-2.3 4-6 4-6-4-6-4z" fill="none" stroke="currentColor" stroke-width="1.3" /><circle cx="7" cy="7" r="1.8" fill="currentColor" /></svg>
				<span>Observing{#if seat}<span class="visually-hidden">, {seat.name} {seat.present ? 'has control' : 'held control and is disconnected'}</span>{/if}</span>
			</span>
			{#if !compact}<button class="btn sm primary" data-testid="claim" onclick={() => acq.claim()} disabled={!canClaim || acq.busy === 'Take Control'} title={canClaim ? 'Take control of the station and the run.' : seat?.present ? `${seat.name} has control. Control passes when it is released.` : 'The control service is not connected.'}>
				Take Control
			</button>{/if}
		{/if}
		{#if ends}
			<span class="ends" role="group" aria-label="Stop the run">
				{#if a.stop.visible}
					<button class="btn sm stop" data-testid="act-stop" aria-disabled={!a.stop.enabled} class:off={!a.stop.enabled} title={a.stop.reason ?? 'Ramp the run down and end it.'} onclick={() => a.stop.enabled && acq.stop()}>
						<svg viewBox="0 0 16 16" width="13" height="13" aria-hidden="true"><path d="M2 4h4l8 8" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" /></svg>
						Stop
					</button>
				{/if}
				{#if a.abort.visible}
					<button class="btn sm abort" data-testid="act-abort" aria-disabled={!a.abort.enabled} class:off={!a.abort.enabled} aria-keyshortcuts={abortKeys.replace('Cmd', 'Meta').replace('Ctrl', 'Control').replace(' ', '+')} title="Abort now ({abortKeys})" onclick={() => a.abort.enabled && acq.abort()}>
						<svg viewBox="0 0 14 14" width="13" height="13" aria-hidden="true"><path d="M4.5 1h5L13 4.5v5L9.5 13h-5L1 9.5v-5z" fill="currentColor" /><path d="M4.8 7h4.4" stroke="var(--status-fail-fill)" stroke-width="1.8" stroke-linecap="round" /></svg>
						Abort
					</button>
				{/if}
			</span>
		{/if}
	</div>
</header>

<style>
	.head {
		container-type: inline-size;
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 4px var(--sp-4);
		min-height: 40px;
		padding: 4px var(--sp-4);
		border-bottom: 1px solid var(--line-1);
	}

	.where,
	.state,
	.seat {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
	}

	.where {
		gap: var(--sp-1);
	}

	.state {
		flex: 1;
		gap: var(--sp-3);
	}

	.back {
		gap: 4px;
		margin-left: calc(-1 * var(--sp-2));
		color: var(--text-2);
	}

	.sep {
		color: var(--text-4);
	}

	.here {
		font-size: var(--text-md);
		font-weight: var(--weight-strong);
		color: var(--text-1);
		white-space: nowrap;
	}

	.here:focus {
		outline: none;
	}

	.flow {
		display: flex;
		align-items: center;
		gap: 2px;
		margin: 0 0 0 var(--sp-2);
		padding: 0;
		list-style: none;
	}

	.flow .arrow {
		display: inline-flex;
		color: var(--text-4);
	}

	.step {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		height: 26px;
		padding: 0 10px 0 4px;
		font-size: var(--text-sm);
		font-weight: var(--weight-medium);
		color: var(--text-3);
		background: transparent;
		border: 1px solid transparent;
		border-radius: 13px;
		white-space: nowrap;
	}

	.step:hover {
		color: var(--text-1);
		background: var(--bg-2);
	}

	.step .n {
		display: inline-grid;
		place-items: center;
		width: 18px;
		height: 18px;
		font-size: var(--text-2xs);
		font-weight: var(--weight-strong);
		border: 1.3px solid currentColor;
		border-radius: 50%;
	}

	.step.on {
		color: var(--text-1);
		font-weight: var(--weight-strong);
		background: var(--blue-weak);
		border-color: var(--blue-line);
	}

	.step.on .n {
		color: var(--bg-1);
		background: var(--blue-2);
		border-color: var(--blue-2);
	}

	.step[aria-disabled='true'] {
		cursor: default;
	}

	.dot {
		display: inline-flex;
		color: var(--blue-2);
		animation: pulse 1.2s ease-in-out infinite;
	}

	.chip {
		display: inline-flex;
	}

	.chip :global(.chip) {
		height: 24px;
	}

	/* One line: a number that does not fit drops whole to a second line, which is not shown. */
	.nums {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 0 var(--sp-4);
		height: 20px;
		margin: 0;
		min-width: 0;
		overflow: hidden;
	}

	/* A test type's own numbers come from its screen, so the rules reach them globally. */
	.nums > :global(div) {
		display: flex;
		flex: none;
		align-items: baseline;
		gap: 6px;
	}

	/* A narrow window keeps the level and drops the clocks, the time at level first. */
	@container (max-width: 1100px) {
		.opt {
			display: none !important;
		}
	}

	@container (max-width: 960px) {
		.opt2 {
			display: none !important;
		}
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

	.to {
		color: var(--text-3);
	}

	.role {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		font-size: var(--text-sm);
		color: var(--text-2);
		white-space: nowrap;
	}

	.role.mine {
		color: var(--blue-1);
		font-weight: var(--weight-strong);
	}

	.orphan {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		padding: 2px 8px;
		font-size: var(--text-xs);
		color: var(--text-1);
		background: var(--amber-weak);
		border: 1px solid var(--amber-line);
		border-radius: var(--radius-md);
		white-space: nowrap;
	}

	.orphan svg {
		color: var(--amber-2);
	}

	.ends {
		display: inline-flex;
		gap: 6px;
		margin-left: var(--sp-2);
	}

	.btn.off {
		opacity: 0.45;
		cursor: default;
	}

	.stop:not(.off) {
		color: var(--amber-1);
		background: var(--amber-weak);
		border-color: var(--amber-line);
	}

	.abort {
		font-weight: var(--weight-strong);
		color: var(--status-fail-text);
		background: var(--status-fail-fill);
		border: 2px solid var(--amber-1);
	}

	.abort:hover:not(.off) {
		background: var(--amber-1) !important;
		border-color: var(--amber-1) !important;
	}

	@keyframes pulse {
		50% {
			opacity: 0.35;
		}
	}
</style>
