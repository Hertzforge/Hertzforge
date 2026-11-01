<script lang="ts">
	/**
	 * The schedule's levels as rungs: done ones checked, the present one filled and marked Now,
	 * the rest hollow. Shape and word carry the state; color only follows.
	 */
	import type { RandomLive, RandomState } from '$lib/protocol/types';
	import { clock, db } from './words';

	interface Props {
		run: RandomState;
		frame: RandomLive | null;
	}
	let { run, frame }: Props = $props();

	const levels = $derived(run.levels_db ?? []);
	const identifying = $derived(run.phase === 'sysid' || run.phase === 'coherence_gate');
	const commanded = $derived(identifying ? null : run.level_db);
	const at = $derived(commanded === null ? -1 : levels.findIndex((l) => Math.abs(l - commanded) < 0.01));
	const below = $derived(commanded === null ? -1 : levels.filter((l) => l < commanded - 0.01).length);

	function stateOf(i: number): 'done' | 'now' | 'next' {
		if (at === i) return 'now';
		if (commanded !== null && levels[i] < commanded - 0.01) return 'done';
		return 'next';
	}
</script>

<section class="ladder" aria-label="Level schedule" data-testid="level-ladder">
	<div class="head">
		<h3>Levels</h3>
		<span class="mode">{run.mode === 'manual' ? 'Manual ladder' : 'Automatic ladder'}</span>
	</div>
	{#if identifying}
		<p class="ident">System identification at <span class="num">{db(run.level_db)}</span></p>
	{/if}
	<ol class="rungs">
		{#each levels as l, i (i)}
			{@const s = stateOf(i)}
			{#if at < 0 && commanded !== null && i === below}
				<li class="between now" aria-current="step">
					<span class="mk" aria-hidden="true"><svg viewBox="0 0 12 12" width="12" height="12"><path d="M6 1l5 5-5 5-5-5z" fill="currentColor" /></svg></span>
					<span class="db num">{db(commanded)}</span>
					<span class="tag">Now</span>
				</li>
			{/if}
			<li class={s} aria-current={s === 'now' ? 'step' : undefined}>
				<span class="mk" aria-hidden="true">
					{#if s === 'done'}
						<svg viewBox="0 0 12 12" width="12" height="12"><path d="M2.5 6.3l2.3 2.3 4.7-5" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" /></svg>
					{:else if s === 'now'}
						<svg viewBox="0 0 12 12" width="12" height="12"><path d="M6 1l5 5-5 5-5-5z" fill="currentColor" /></svg>
					{:else}
						<svg viewBox="0 0 12 12" width="12" height="12"><path d="M6 1.5l4.5 4.5L6 10.5 1.5 6z" fill="none" stroke="currentColor" stroke-width="1.3" /></svg>
					{/if}
				</span>
				<span class="db num">{db(l)}</span>
				{#if s === 'now'}<span class="tag">Now</span>{/if}
				{#if i === levels.length - 1}<span class="full">Full level</span>{/if}
			</li>
		{/each}
		{#if at < 0 && commanded !== null && below === levels.length}
			<li class="between now" aria-current="step">
				<span class="mk" aria-hidden="true"><svg viewBox="0 0 12 12" width="12" height="12"><path d="M6 1l5 5-5 5-5-5z" fill="currentColor" /></svg></span>
				<span class="db num">{db(commanded)}</span>
				<span class="tag">Now</span>
			</li>
		{/if}
	</ol>
	{#if frame && frame.level_db !== null && commanded !== null && Math.abs(frame.level_db - commanded) > 0.05}
		<p class="walk">Ramping: <span class="num">{db(frame.level_db)}</span> to <span class="num">{db(commanded)}</span></p>
	{/if}
	{#if frame?.at_level_s !== null && frame?.at_level_s !== undefined}
		<p class="time num">{clock(frame.at_level_s)} at this level{frame.remaining_at_level_s !== null ? `, ${clock(frame.remaining_at_level_s)} left` : ''}</p>
	{/if}
</section>

<style>
	.ladder {
		display: flex;
		flex-direction: column;
		gap: 6px;
	}

	.head {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
	}

	h3 {
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
	}

	.mode {
		font-size: var(--text-xs);
		color: var(--text-3);
	}

	.ident,
	.walk,
	.time {
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	.rungs {
		display: flex;
		flex-wrap: wrap;
		gap: 4px;
		margin: 0;
		padding: 0;
		list-style: none;
	}

	.rungs li {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		height: 26px;
		padding: 0 8px;
		font-size: var(--text-xs);
		color: var(--text-3);
		background: var(--bg-2);
		border: 1px solid var(--line-2);
		border-radius: var(--radius-md);
	}

	.rungs li.done {
		color: var(--text-2);
	}

	.rungs li.now {
		color: var(--text-1);
		font-weight: var(--weight-strong);
		border: 1.5px solid var(--blue-2);
		background: var(--blue-weak);
	}

	.rungs li.now .mk {
		color: var(--blue-1);
	}

	.mk {
		display: inline-flex;
	}

	.tag {
		font-size: var(--text-2xs);
		letter-spacing: 0.05em;
		text-transform: uppercase;
		color: var(--blue-1);
	}

	.full {
		font-size: var(--text-2xs);
		color: var(--text-3);
	}
</style>
