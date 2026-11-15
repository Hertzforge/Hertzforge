<script lang="ts">
	/**
	 * The level ladder, the time between pulses, the pulse grid and the identification shot. The
	 * ladder climbs from its first level to full level, one level on a passing pulse; a level below
	 * full that spends its pulse allowance without a pass ends the run. Full level completes on its
	 * passing pulses. Identification plays independent band-limited random on every drive at once,
	 * on the pulse's own grid, before any pulse.
	 */
	import { formatNumber } from '$lib/viz/format';
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import { db } from '../words';
	import { parseLevels, RECORD_LENGTHS } from './msSetup';
	import type { MimoShockStore } from './msStore.svelte';

	interface Props {
		ms: MimoShockStore;
	}
	let { ms }: Props = $props();

	const d = $derived(ms.setup);
	const locked = $derived(ms.locked);
	const E = $derived(ms.fieldErrors);
	const errs = (k: string) => E[k] ?? [];
	let levelsText = $state('');
	let editing = $state(false);
	$effect(() => {
		if (!editing) levelsText = d.schedule.levels_db.map((v) => (Number.isFinite(v) ? String(v) : '')).join(', ');
	});
	const dry = $derived(ms.check && ms.checkedKey === ms.key ? ms.check.derived : null);
</script>

<div class="stack">
	<section class="aq-panel" aria-labelledby="ms-l-title" data-testid="ms-ladder">
		<div class="aq-head"><h2 id="ms-l-title">Level Ladder</h2><span class="aq-sub">The levels the run climbs, in dB from full level.</span></div>
		<div class="aq-fields">
			<div class="sel wide">
				<label for="ms-levels" class="lbl">Levels</label>
				<input
					id="ms-levels"
					class="field num"
					value={levelsText}
					disabled={locked}
					data-testid="ms-levels"
					onfocus={() => (editing = true)}
					onblur={() => (editing = false)}
					oninput={(e) => {
						levelsText = e.currentTarget.value;
						d.schedule.levels_db = parseLevels(levelsText);
					}}
				/>
				<span class="aq-small aq-muted">Rising, the last 0 dB. Usual: -12, -9, -6, -3, -1.5, 0, with a half step before full level.</span>
				<Findings items={errs('schedule.levels_db').map((message) => ({ level: 'error' as const, message }))} />
			</div>
			<div class="sel">
				<span class="lbl" id="ms-mode-l">Firing</span>
				<div class="aq-seg sm" role="radiogroup" aria-labelledby="ms-mode-l">
					<button type="button" role="radio" aria-checked={d.schedule.mode === 'auto'} disabled={locked} onclick={() => (d.schedule.mode = 'auto')}>Automatic</button>
					<button type="button" role="radio" aria-checked={d.schedule.mode === 'manual'} disabled={locked} onclick={() => (d.schedule.mode = 'manual')} data-testid="ms-manual">On Fire</button>
				</div>
				<span class="aq-small aq-muted">{d.schedule.mode === 'auto' ? 'Each pulse fires once the time between pulses has passed.' : 'Each pulse waits for you to press Fire. The ladder still sets the level.'}</span>
			</div>
			<NumberField id="ms-perlevel" label="Pulses a level at most" integer step={1} min={1} max={100} bind:value={d.schedule.max_pulses_per_level} errors={errs('schedule.max_pulses_per_level')} disabled={locked} testid="ms-per-level" help="Without a passing pulse, the run ends here. Usual: 3." />
			<NumberField id="ms-full" label="Passing pulses at full level" integer step={1} min={1} max={100} bind:value={d.schedule.full_level_pulses} errors={errs('schedule.full_level_pulses')} disabled={locked} testid="ms-full-pulses" help="The run completes on these. Usual: 3." />
		</div>
		<p class="aq-small aq-muted">When the first pulses predict an over-test at the next level, the control service may take a smaller step than the table's; the run shows the step it took and why.</p>
	</section>

	<section class="aq-panel" aria-labelledby="ms-sp-title">
		<div class="aq-head"><h2 id="ms-sp-title">Time Between Pulses</h2><span class="aq-sub">The largest of the three.</span></div>
		<div class="aq-fields">
			<NumberField id="ms-cool" label="Cooling" unit="s" step={0.5} min={0} bind:value={d.spacing.cooling_s} errors={errs('spacing.cooling_s')} disabled={locked} />
			<NumberField id="ms-settle" label="Settling" unit="s" step={0.1} min={0} bind:value={d.spacing.settling_s} errors={errs('spacing.settling_s')} disabled={locked} />
			<NumberField id="ms-compute" label="Law time" unit="s" step={0.05} min={0} bind:value={d.spacing.compute_s} errors={errs('spacing.compute_s')} disabled={locked} />
		</div>
		{#if dry}<p class="aq-small" data-testid="ms-spacing">{formatNumber(dry.spacing_s, 3)} s between pulses; the law has {formatNumber(dry.call_budget_s, 3)} s a call.</p>{/if}
	</section>

	<section class="aq-panel" aria-labelledby="ms-g-title">
		<div class="aq-head"><h2 id="ms-g-title">Pulse Grid</h2><span class="aq-sub">Every record of the run is on this grid.</span></div>
		<div class="aq-fields">
			<NumberField id="ms-fs" label="Sample rate" unit="Hz" step={1000} min={1000} bind:value={d.acquisition.fs_hz} errors={errs('acquisition.fs_hz')} disabled={locked} help="The run adopts the rate the station runs at." />
			<div class="sel">
				<label for="ms-rl" class="lbl">Record length</label>
				<select id="ms-rl" class="field num" value={d.acquisition.record_len ?? ''} disabled={locked} onchange={(e) => (d.acquisition.record_len = e.currentTarget.value ? Number(e.currentTarget.value) : null)} data-testid="ms-record-len">
					<option value="">Shortest that holds every target</option>
					{#each RECORD_LENGTHS as n (n)}<option value={n}>{n} samples</option>{/each}
				</select>
			</div>
			<NumberField id="ms-blo" label="Band from" unit="Hz" step={1} min={0} value={d.acquisition.band_hz[0]} onchange={(v) => (d.acquisition.band_hz = [v ?? NaN, d.acquisition.band_hz[1]])} errors={errs('acquisition.band_hz')} disabled={locked} help="Identification excites this band and the law corrects in it." />
			<NumberField id="ms-bhi" label="Band to" unit="Hz" step={100} min={0} value={d.acquisition.band_hz[1]} onchange={(v) => (d.acquisition.band_hz = [d.acquisition.band_hz[0], v ?? NaN])} disabled={locked} />
			<NumberField id="ms-lead" label="Target starts at" unit="s" step={0.01} min={0} bind:value={d.acquisition.lead_s} errors={errs('acquisition.lead_s')} disabled={locked} />
		</div>
		{#if dry}<p class="aq-small" data-testid="ms-grid">{formatNumber(dry.grid.fs_hz, 6)} Hz, {dry.grid.record_len} samples a pulse{dry.grid.actual ? '' : ', until the station arms'}.</p>{/if}
	</section>

	<section class="aq-panel" aria-labelledby="ms-id-title">
		<div class="aq-head"><h2 id="ms-id-title">Identification</h2><span class="aq-sub">One shot of band-limited random on every drive at once, before any pulse.</span></div>
		<div class="aq-fields">
			<NumberField id="ms-idv" label="Drive at most" unit="V rms" step={0.01} min={0} bind:value={d.identification.drive_vrms} errors={errs('identification.drive_vrms')} disabled={locked} help="Under each drive's ceiling at its peak; the shot's level is found by a probe at a tenth of it." />
			<NumberField id="ms-idf" label="Records averaged" integer nullable="Usual" fallback={64} step={16} min={1} bind:value={d.identification.frames} errors={errs('identification.frames')} disabled={locked} help="Usual: the larger of 64 and 10 times the drives." />
			<NumberField id="ms-idc" label="Coherence floor" step={0.05} min={0} max={1} bind:value={d.identification.coherence_floor} errors={errs('identification.coherence_floor')} disabled={locked} help="Lines where a control is measured with less are not trusted." />
			<label class="aq-switch">
				<input type="checkbox" role="switch" aria-checked={d.identification.review} bind:checked={d.identification.review} disabled={locked} data-testid="ms-review" />
				<span class="track" aria-hidden="true"><span class="thumb"></span></span>
				<span class="word">{d.identification.review ? 'Hold for review after identification' : 'Go straight on after identification'}</span>
			</label>
		</div>
		<p class="aq-small aq-muted">Whatever this says, a control whose best reachable response fails its own judge holds the run before the first pulse until you acknowledge it.</p>
		<p class="aq-small aq-muted">Levels: {d.schedule.levels_db.filter(Number.isFinite).map((v) => db(v)).join(', ') || '-'}.</p>
	</section>
</div>

<style>
	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
	}

	.sel {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.sel.wide {
		grid-column: 1 / -1;
	}

	.lbl {
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}
</style>
