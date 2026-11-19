<script lang="ts">
	/**
	 * The run bar's status: where the data comes from, the run's state, phase, clock and level, the
	 * run's id, and who holds control (with Take Control and Release, and the countdown while nobody
	 * does during a run).
	 */
	import ProvenanceBadge from '$lib/shell/ProvenanceBadge.svelte';
	import type { Snippet } from 'svelte';
	import type { RunBarHost } from './actions';
	import StateChip from './StateChip.svelte';
	import { clock, db, phaseWord } from './words';

	interface Props {
		acq: RunBarHost;
		/** A window on another monitor: no Take Control or Release. */
		compact?: boolean;
		/** A test type's own numbers in place of the time at level (classical shock's pulse counts). */
		nums?: Snippet;
	}
	let { acq, compact = false, nums }: Props = $props();

	const run = $derived(acq.run);
	const frame = $derived(acq.frame);
	const levelNow = $derived(frame?.level_db ?? null);
	const levelCommanded = $derived(run?.level_db ?? null);
	const atLevel = $derived(frame?.at_level_s ?? run?.at_level_s ?? null);
	const remaining = $derived(frame?.remaining_at_level_s ?? run?.remaining_at_level_s ?? null);

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
	const busy = (verb: string) => acq.busy === verb;
</script>

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

<style>
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

	/* As wide as the run bar's primary verb. */
	.btn.primary {
		min-width: 92px;
	}

	.orphan svg {
		color: var(--amber-2);
	}

	@media (max-width: 900px) {
		.nums {
			order: 3;
			width: 100%;
			margin-left: 0;
		}
	}
</style>
