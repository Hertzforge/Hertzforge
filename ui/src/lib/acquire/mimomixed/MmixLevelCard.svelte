<script lang="ts">
	/**
	 * The run in numbers: the level and its time, parts time, whether the floor has equalized and its
	 * worst error now, and how the run ended, by the part, control or drive the end names.
	 */
	import { clock, db } from '../words';
	import { endWords, errDb } from './mmixWords';
	import type { MimoMixedStore } from './mmixStore.svelte';

	interface Props {
		mm: MimoMixedStore;
	}
	let { mm }: Props = $props();

	const s = $derived(mm.now);
	const worst = $derived(s?.floor_error_db.length ? s.floor_error_db.reduce((w, e) => (Math.abs(e) > Math.abs(w) ? e : w), 0) : null);
	const t = (v: number | null | undefined) => (v === null || v === undefined ? '-' : clock(v));
</script>

<section aria-label="Level" data-testid="mmix-level-card">
	<dl>
		<div><dt>Level</dt><dd class="num" data-testid="mmix-level">{s?.level_db === null || s?.level_db === undefined ? '-' : db(s.level_db)}</dd></div>
		<div><dt>At level</dt><dd class="num">{t(s?.at_level_s)}{s?.remaining_at_level_s !== null && s?.remaining_at_level_s !== undefined ? `, ${clock(s.remaining_at_level_s)} left` : ''}</dd></div>
		<div><dt>Parts time</dt><dd class="num">{t(s?.parts_s)}</dd></div>
		<div><dt>Floor</dt><dd data-testid="mmix-floor-state">{s?.floor_equalized ? 'Equalized' : s?.phase === 'floor_only' ? 'Equalizing alone' : '-'}{worst !== null ? `, worst error ${errDb(worst)}` : ''}</dd></div>
	</dl>
	{#if s?.end}
		<p class="aq-small end" data-testid="mmix-end">{endWords(s.end, { part: (id) => mm.partName(id), control: (i) => mm.runControlLabels[i] ?? `Control ${i + 1}`, drive: (i) => mm.runDriveLabels[i] ?? `Drive ${i + 1}` })}</p>
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

	.end {
		margin: var(--sp-2) 0 0;
	}
</style>
