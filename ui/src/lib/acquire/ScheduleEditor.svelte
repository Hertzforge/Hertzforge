<script lang="ts">
	/**
	 * The level schedule: levels in dB relative to the reference, rising to full level. In the
	 * automatic ladder each level holds for its dwell and full level for its time; in the manual
	 * ladder the run starts at the first level and moves only when the operator steps it.
	 */
	import Findings from './Findings.svelte';
	import NumberField from './NumberField.svelte';
	import { under } from './setup';
	import type { ScheduleHost } from './setup';

	interface Props {
		acq: ScheduleHost;
	}
	let { acq }: Props = $props();

	const s = $derived(acq.setup.schedule);
	const locked = $derived(acq.locked);
	const E = $derived(acq.fieldErrors);
	const last = $derived(s.levels.length - 1);

	/** Findings on a level from the service name the level; the editor's name the cell. */
	function levelFindings(i: number) {
		const own = new Set([...(E[`schedule.levels.${i}.db`] ?? []), ...(E[`schedule.levels.${i}.dwell_s`] ?? [])]);
		return under(E, `schedule.levels.${i}`)
			.filter((m) => !own.has(m))
			.map((message) => ({ level: 'error' as const, message }));
	}

	function addLevel() {
		const top = s.levels[last];
		s.levels.splice(Math.max(0, last), 0, { db: (top?.db ?? 0) - 3, dwell_s: 30 });
	}
</script>

<section class="aq-panel schedule" aria-labelledby="schedule-title" data-testid="schedule-panel">
	<div class="aq-head">
		<h2 id="schedule-title">Schedule</h2>
		<span class="aq-spacer"></span>
		<div class="aq-seg" role="radiogroup" aria-label="Ladder">
			<button type="button" role="radio" aria-checked={s.mode === 'auto'} disabled={locked} onclick={() => (s.mode = 'auto')} data-testid="schedule-auto">Automatic</button>
			<button type="button" role="radio" aria-checked={s.mode === 'manual'} disabled={locked} onclick={() => (s.mode = 'manual')} data-testid="schedule-manual">Manual</button>
		</div>
	</div>
	<p class="aq-muted aq-small lead">
		{#if s.mode === 'auto'}
			The run steps up through the levels, holds each for its dwell, holds full level for its time, and ends complete.
		{:else}
			The run starts at the first level and moves only when you step it, between the first and the last level, until stopped.
		{/if}
	</p>

	<Findings items={(E['schedule.levels'] ?? []).map((message) => ({ level: 'error' as const, message }))} />

	<table class="aq-table levels" data-testid="level-table">
		<thead>
			<tr>
				<th>Step</th>
				<th class="num">Level</th>
				<th class="num">Holds For</th>
				<th><span class="visually-hidden">Remove</span></th>
			</tr>
		</thead>
		<tbody>
			{#each s.levels as l, i (l)}
				{@const lf = levelFindings(i)}
				<tr>
					<td class="aq-muted aq-small">{i === last ? 'Full level' : `Level ${i + 1}`}</td>
					<td class="num"><NumberField id="lv-{i}-db" label="Level {i + 1}" compact unit="dB" bind:value={l.db} errors={E[`schedule.levels.${i}.db`] ?? []} disabled={locked} /></td>
					<td class="num">
						{#if i === last}
							<NumberField id="lv-full" label="Full level time" compact unit="s" bind:value={s.full_level_s} errors={E['schedule.full_level_s'] ?? []} disabled={locked} testid="full-level-s" />
						{:else}
							<NumberField id="lv-{i}-dw" label="Level {i + 1} dwell" compact unit="s" bind:value={l.dwell_s} errors={E[`schedule.levels.${i}.dwell_s`] ?? []} disabled={locked} />
						{/if}
					</td>
					<td class="acts">
						<button class="aq-icon-btn" aria-label="Remove level {i + 1}" disabled={locked || s.levels.length <= 1} onclick={() => s.levels.splice(i, 1)}>
							<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M3 3l6 6M9 3l-6 6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
						</button>
					</td>
				</tr>
				{#if lf.length}
					<tr class="aq-findrow"><td colspan="4"><Findings items={lf} /></td></tr>
				{/if}
			{/each}
		</tbody>
	</table>
	<div class="aq-actions add">
		<button class="btn sm" disabled={locked} onclick={addLevel}>Add Level</button>
		{#if s.mode === 'manual'}<span class="aq-muted aq-small">Dwells apply if the ladder is handed back to automatic during the run.</span>{/if}
	</div>
</section>

<style>
	.lead {
		margin: -6px 0 var(--sp-3);
	}

	.levels td.num :global(.nf) {
		min-width: 110px;
	}

	.acts {
		width: 1%;
	}

	.add {
		margin-top: var(--sp-2);
	}
</style>
