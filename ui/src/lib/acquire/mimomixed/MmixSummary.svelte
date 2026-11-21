<script lang="ts">
	/**
	 * The MIMO Mixed Mode setup in a few lines, beside every section: the shape, the parts, the
	 * schedule, the five choices under Laws, and what the service derived from the draft (the band,
	 * the grid, the identification time and the tracker's delay); then what is left to fix, each a
	 * click from its section.
	 */
	import { db } from '../words';
	import { BAND_LAW_WORDS, CEILING_RULE_WORDS, FILL_RULE_WORDS, MMIX_SECTIONS, type MmixSection } from './mmixSetup';
	import type { MimoMixedStore } from './mmixStore.svelte';

	interface Props {
		mm: MimoMixedStore;
		onopen: (s: MmixSection) => void;
	}
	let { mm, onopen }: Props = $props();

	const d = $derived(mm.setup);
	const dry = $derived(mm.check && mm.checkedKey === mm.key && !mm.check.errors.length ? mm.check.derived : null);
	const todo = $derived(MMIX_SECTIONS.filter((s) => mm.counts[s.id].errors > 0));
	const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;
</script>

<aside class="aq-panel summary" aria-label="Setup summary" data-testid="mmix-summary">
	<h2>Summary</h2>
	<dl>
		<div><dt>Shape</dt><dd data-testid="mmix-summary-shape">{plural(d.controls.length, 'control', 'controls')} by {plural(d.drives.length, 'drive', 'drives')}{d.limits.length ? `, ${plural(d.limits.length, 'limit', 'limits')}` : ''}</dd></div>
		<div><dt>Parts</dt><dd>The floor, {plural(d.tones.length, 'tone', 'tones')}, {plural(d.bands.length, 'narrowband', 'narrowbands')}</dd></div>
		<div><dt>Schedule</dt><dd>{d.schedule.mode === 'auto' ? 'Automatic' : 'Manual'}, {plural(d.schedule.levels.length, 'level', 'levels')} from {db(d.schedule.levels[0]?.db)}</dd></div>
		<div><dt>Laws</dt><dd data-testid="mmix-summary-laws">Floor: {mm.floorLaw ? `${mm.floorLaw.name} ${mm.floorLaw.version}` : '-'}; tones: {mm.toneLaw ? `${mm.toneLaw.name} ${mm.toneLaw.version}` : '-'}</dd></div>
		<div><dt>Rules</dt><dd>Bands: {BAND_LAW_WORDS[d.laws.bands].label}; ceiling: {CEILING_RULE_WORDS[d.laws.ceiling_rule].label}; fill: {FILL_RULE_WORDS[d.laws.fill_rule].label}</dd></div>
	</dl>

	{#if dry}
		<h3>From the Control Service</h3>
		<dl data-testid="mmix-summary-derived">
			<div><dt>Band</dt><dd class="num">{dry.band_hz[0]} to {dry.band_hz[1]} Hz, {dry.grid.lines} lines every {dry.grid.df_hz} Hz</dd></div>
			<div><dt>Identification</dt><dd class="num">About {dry.identification_s} s</dd></div>
			<div><dt>Test</dt><dd class="num">{dry.test_s === null ? 'Until stopped (manual)' : `About ${Math.round(dry.test_s)} s at level`}</dd></div>
			<div><dt>Tracker delay</dt><dd class="num" data-testid="mmix-summary-tracker">About {dry.tracker_delay_s} s</dd></div>
		</dl>
	{/if}

	<div class="ready" data-testid="mmix-ready" data-ready={todo.length === 0 && !!dry}>
		{#if todo.length}
			<h3>Left to Fix</h3>
			<ul>
				{#each todo as s (s.id)}
					<li><button class="btn ghost sm" onclick={() => onopen(s.id)} data-testid="mmix-todo-{s.id}">{s.label}: {plural(mm.counts[s.id].errors, 'error', 'errors')}</button></li>
				{/each}
			</ul>
		{:else if dry}
			<p class="aq-small ok">Nothing is missing: the setup checks clean.</p>
		{:else if mm.pending}
			<p class="aq-small aq-muted">{mm.pending}</p>
		{/if}
	</div>
</aside>

<style>
	.summary {
		position: sticky;
		top: 0;
	}

	h2 {
		margin-bottom: var(--sp-2);
		font-size: var(--text-md);
		font-weight: var(--weight-strong);
	}

	h3 {
		margin: var(--sp-3) 0 var(--sp-1);
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
	}

	dl {
		display: flex;
		flex-direction: column;
		gap: 6px;
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
		color: var(--text-1);
	}

	ul {
		margin: 0;
		padding: 0;
		list-style: none;
	}

	.ok {
		margin: var(--sp-3) 0 0;
		color: var(--blue-1);
	}
</style>
