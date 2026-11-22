<script lang="ts">
	/**
	 * The run in numbers: the level and the schedule's mode, the run's time, the time at the rung or
	 * at full level and what is left of it (shown only while the service reports them), whether the
	 * floor has equalized, its worst error now and its loops, and how the run ended, by the part,
	 * control or drive the end names (the service names controls and drives by channel).
	 */
	import { clock, db } from '../words';
	import { endWords, errDb } from './mmixWords';
	import type { MimoMixedStore } from './mmixStore.svelte';

	interface Props {
		mm: MimoMixedStore;
	}
	let { mm }: Props = $props();

	const s = $derived(mm.now);
	const worst = $derived(s?.floor_error_db.length ? s.floor_error_db.reduce<number>((w, e) => (e !== null && Math.abs(e) > Math.abs(w) ? e : w), 0) : null);
	const floor = $derived(mm.status?.floor ?? null);
</script>

<section aria-label="Level" data-testid="mmix-level-card">
	<dl>
		<div><dt>Level</dt><dd class="num" data-testid="mmix-level">{s?.level_db === null || s?.level_db === undefined ? '-' : db(s.level_db)}</dd></div>
		<div><dt>Schedule</dt><dd data-testid="mmix-mode">{s?.mode === 'manual' ? 'Manual' : s?.mode === 'auto' ? 'Automatic' : '-'}</dd></div>
		<div><dt>Run time</dt><dd class="num" data-testid="mmix-t">{s?.t_s === null || s?.t_s === undefined ? '-' : clock(s.t_s)}</dd></div>
		{#if s?.at_level_s !== null && s?.at_level_s !== undefined}
			<div><dt>At this level</dt><dd class="num" data-testid="mmix-at-level">{clock(s.at_level_s)}{#if s.remaining_at_level_s !== null && s.remaining_at_level_s !== undefined}<span class="sub">, {clock(s.remaining_at_level_s)} left</span>{/if}</dd></div>
		{/if}
		<div><dt>Floor</dt><dd data-testid="mmix-floor-state">{s?.floor_equalized ? 'Equalized' : s?.phase === 'floor_only' ? 'Equalizing alone' : '-'}{worst !== null ? `, worst error ${errDb(worst)}` : ''}</dd></div>
		{#if floor}
			<div><dt>Floor loops</dt><dd class="num" data-testid="mmix-floor-loops" title="Missed loops, underruns, dropped and clamped frames">{floor.loops}{floor.misses || floor.underruns || floor.dropped || floor.clamped ? `, ${floor.misses} missed, ${floor.underruns} short, ${floor.dropped} dropped, ${floor.clamped} clamped` : ''}</dd></div>
		{/if}
	</dl>
	{#if s?.end}
		<p class="aq-small end" data-testid="mmix-end">{endWords(s.end, { part: (id) => mm.partName(id), channel: (c) => mm.channelLabel(c) || String(c) })}</p>
	{/if}
</section>

<style>
	dl {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(9em, 1fr));
		gap: 6px var(--sp-3);
		margin: 0;
	}

	dt {
		font-size: var(--text-2xs);
		color: var(--text-3);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	dd {
		margin: 0;
		font-size: var(--text-sm);
	}

	.sub {
		color: var(--text-3);
	}

	.end {
		margin: var(--sp-2) 0 0;
	}
</style>
