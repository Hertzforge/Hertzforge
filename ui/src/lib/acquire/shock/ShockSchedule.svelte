<script lang="ts">
	/**
	 * The level schedule: an automatic ladder that fires itself from the first level to full level,
	 * or a manual one that fires on the operator's word; each level's pulses; when a level is done
	 * (after its pulses, or on a passing pulse with a limit on tries); and the time between pulses.
	 * SRS shock's setup shows the same section; it has no trains or bursts.
	 */
	import { formatNumber } from '$lib/viz/format';
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import type { FieldErrors } from '../setup';
	import { db } from '../words';
	import type { ShockDraft } from './shockSetup';

	/** What the section reads of a test type's store (classical shock's, SRS shock's). */
	interface ScheduleStore {
		setup: { schedule: ShockDraft['schedule']; spacing: ShockDraft['spacing'] };
		locked: boolean;
		fieldErrors: FieldErrors;
		check: { derived: { spacing_s: number; call_budget_s: number } | null } | null;
		checkedKey: string;
		key: string;
	}

	interface Props {
		shock: ScheduleStore;
		/** The manual ladder also fires trains and bursts. */
		trains?: boolean;
	}
	let { shock, trains = true }: Props = $props();

	const d = $derived(shock.setup);
	const s = $derived(d.schedule);
	const locked = $derived(shock.locked);
	const E = $derived(shock.fieldErrors);
	const errs = (k: string) => E[k] ?? [];
	const dry = $derived(shock.check && shock.checkedKey === shock.key ? shock.check.derived : null);
	const total = $derived(s.levels.reduce((n, l) => n + (Number.isFinite(l.pulses) ? l.pulses : 0), 0));
	const tableErrors = $derived(Object.entries(E).filter(([k]) => k === 'schedule.levels' || k === 'schedule.pulses' || /^schedule\.levels\.\d+/.test(k)).flatMap(([, v]) => v));

	function addLevel() {
		const last = s.levels.at(-1);
		const before = s.levels.length > 1 ? s.levels[s.levels.length - 2].db : (last?.db ?? 0) - 3;
		s.levels.splice(Math.max(0, s.levels.length - 1), 0, { db: Math.round(((before + (last?.db ?? 0)) / 2) * 2) / 2, pulses: 1 });
	}
</script>

<div class="cols">
	<section class="aq-panel" aria-labelledby="sc-lad">
		<div class="aq-head"><h2 id="sc-lad">Levels</h2><span class="aq-sub">From the first level to full level, 0 dB, the last.</span></div>
		<div class="aq-seg" role="radiogroup" aria-label="Ladder">
			<button type="button" role="radio" aria-checked={s.mode === 'auto'} disabled={locked} onclick={() => (s.mode = 'auto')} data-testid="shock-mode-auto">Automatic</button>
			<button type="button" role="radio" aria-checked={s.mode === 'manual'} disabled={locked} onclick={() => (s.mode = 'manual')} data-testid="shock-mode-manual">Manual</button>
		</div>
		<p class="aq-small aq-muted">
			{s.mode === 'auto'
				? 'The run fires the schedule by itself, level by level, at the time between pulses.'
				: trains
					? 'The run waits for Fire. You step the level, go to a level of the table, fire a train or a burst. The mode is fixed for the run.'
					: 'The run waits for Fire. You step the level or go to a level of the table. The mode is fixed for the run.'}
		</p>
		<table class="aq-table levels" data-testid="shock-level-table">
			<thead><tr><th>Level</th><th class="r">dB</th><th class="r">Pulses</th><th aria-label="Remove"></th></tr></thead>
			<tbody>
				{#each s.levels as l, i (i)}
					<tr>
						<td>{i === s.levels.length - 1 ? 'Full level' : `Level ${i + 1}`}</td>
						<td class="num"><NumberField id="sc-db-{i}" label="Level {i + 1}, dB" compact step={1} bind:value={l.db} errors={errs(`schedule.levels.${i}.db`)} disabled={locked} testid="shock-level-{i}-db" /></td>
						<td class="num"><NumberField id="sc-n-{i}" label="Level {i + 1}, pulses" compact integer step={1} min={1} bind:value={l.pulses} errors={errs(`schedule.levels.${i}.pulses`)} disabled={locked} testid="shock-level-{i}-pulses" /></td>
						<td>
							<button class="aq-icon-btn" aria-label="Remove level {i + 1}" disabled={locked || s.levels.length <= 1} onclick={() => s.levels.splice(i, 1)}>
								<svg viewBox="0 0 12 12" width="10" height="10" aria-hidden="true"><path d="M3 3l6 6M9 3l-6 6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
							</button>
						</td>
					</tr>
				{/each}
			</tbody>
		</table>
		<div class="row">
			<button class="btn sm ghost" disabled={locked} onclick={addLevel} data-testid="shock-level-add">Add Level</button>
			<span class="aq-small aq-muted" data-testid="shock-planned">{total} {total === 1 ? 'pulse' : 'pulses'} planned</span>
		</div>
		<Findings items={tableErrors.map((message) => ({ level: 'error' as const, message }))} />

		{#if s.mode === 'manual'}
			<div class="sel spaced">
				<label for="sc-start" class="lbl">Start at</label>
				<select id="sc-start" class="field num" bind:value={s.start_db} disabled={locked} data-testid="shock-start-db">
					{#each s.levels as l, i (i)}<option value={l.db}>{db(l.db)}</option>{/each}
				</select>
				<Findings items={errs('schedule.start_db').map((message) => ({ level: 'error' as const, message }))} />
			</div>
		{/if}
	</section>

	<div class="stack">
		<section class="aq-panel" aria-labelledby="sc-adv">
			<div class="aq-head"><h2 id="sc-adv">When a Level Is Done</h2></div>
			<div class="aq-seg" role="radiogroup" aria-label="When a level is done">
				<button type="button" role="radio" aria-checked={s.advance === 'on_pass'} disabled={locked} onclick={() => (s.advance = 'on_pass')} data-testid="shock-advance-on_pass">On passing pulses</button>
				<button type="button" role="radio" aria-checked={s.advance === 'count'} disabled={locked} onclick={() => (s.advance = 'count')} data-testid="shock-advance-count">After its pulses</button>
			</div>
			<p class="aq-small aq-muted">
				{s.advance === 'on_pass'
					? "A level counts only passing pulses, so the law keeps correcting until a level's pulses pass. Too many tries at one level end the run as not converged."
					: 'A level is done once its pulses have fired, passed or not.'}
			</p>
			{#if s.advance === 'on_pass'}
				<div class="aq-fields">
					<NumberField id="sc-max" label="Pulses at one level without a pass" integer step={1} min={1} bind:value={s.max_pulses_per_level} errors={errs('schedule.max_pulses_per_level')} disabled={locked} testid="shock-max-per-level" help="In manual, failing pulses in a row at one level. Usual: 5." />
				</div>
			{/if}
		</section>

		<section class="aq-panel" aria-labelledby="sc-sp">
			<div class="aq-head"><h2 id="sc-sp">Time Between Pulses</h2></div>
			<p class="aq-lead aq-small">The time between pulses is at least the largest of the three. It also sets the control law's time budget.</p>
			<div class="aq-fields">
				<NumberField id="sc-cool" label="Cooling" unit="s" step={0.5} min={0} bind:value={d.spacing.cooling_s} errors={errs('spacing.cooling_s')} disabled={locked} testid="shock-cooling" help="For the shaker and the amplifier between pulses." />
				<NumberField id="sc-set" label="Settling" unit="s" step={0.1} min={0} bind:value={d.spacing.settling_s} errors={errs('spacing.settling_s')} disabled={locked} testid="shock-settling" help="For the table to come to rest." />
				<NumberField id="sc-cmp" label="Computing" unit="s" step={0.05} min={0} bind:value={d.spacing.compute_s} errors={errs('spacing.compute_s')} disabled={locked} testid="shock-compute" help="For the next drive to be computed and checked." />
			</div>
			{#if dry}
				<p class="aq-small derived" data-testid="shock-spacing">Time between pulses: <span class="num">{formatNumber(dry.spacing_s, 3)} s</span>. The law's time budget: <span class="num">{formatNumber(dry.call_budget_s * 1000, 3)} ms</span> a call.</p>
			{/if}
		</section>
	</div>
</div>

<style>
	.cols {
		display: grid;
		grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
		gap: var(--sp-3);
		align-items: start;
	}

	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
	}

	.levels {
		margin-top: var(--sp-3);
	}

	.r {
		text-align: right;
	}

	.row {
		display: flex;
		align-items: center;
		gap: var(--sp-3);
		margin-top: var(--sp-2);
	}

	.sel {
		display: flex;
		flex-direction: column;
		gap: 4px;
		max-width: 220px;
	}

	.lbl {
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}

	.spaced {
		margin-top: var(--sp-3);
	}

	.derived {
		margin: var(--sp-3) 0 0;
		color: var(--text-2);
	}

	@media (max-width: 1240px) {
		.cols {
			grid-template-columns: minmax(0, 1fr);
		}
	}
</style>
