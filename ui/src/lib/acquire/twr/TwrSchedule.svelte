<script lang="ts">
	/**
	 * The level ladder and the playback: an automatic ladder that corrects at each level until a play
	 * meets the gate and moves on, or a manual one that waits for Iterate, Play and the level verbs;
	 * the levels, the last of them full level; what happens when a level spends its correction plays
	 * without a gate; how often the converged drive plays back once full level has gated; and whether
	 * the law keeps correcting during playback.
	 */
	import { formatNumber } from '$lib/viz/format';
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import { RUN_SCHEDULES } from './twrSetup';
	import type { TwrStore } from './twrStore.svelte';

	interface Props {
		twr: TwrStore;
	}
	let { twr }: Props = $props();

	const d = $derived(twr.setup);
	const s = $derived(d.schedule);
	const locked = $derived(twr.locked);
	const E = $derived(twr.fieldErrors);
	const errs = (k: string) => E[k] ?? [];
	const dry = $derived(twr.check && twr.checkedKey === twr.key ? twr.check.derived : null);
	const tableErrors = $derived(Object.entries(E).filter(([k]) => k === 'schedule.levels_db').flatMap(([, v]) => v));

	function addLevel() {
		const last = s.levels_db.at(-1) ?? 0;
		const before = s.levels_db.length > 1 ? s.levels_db[s.levels_db.length - 2] : last - 3;
		s.levels_db.splice(Math.max(0, s.levels_db.length - 1), 0, Math.round(((before + last) / 2) * 2) / 2);
	}
</script>

<div class="cols">
	<section class="aq-panel" aria-labelledby="ts-lad" data-testid="twr-ladder">
		<div class="aq-head"><h2 id="ts-lad">Levels</h2><span class="aq-sub">From the first level to full level, the last.</span></div>
		<div class="aq-seg" role="radiogroup" aria-label="Ladder">
			<button type="button" role="radio" aria-checked={s.mode === 'auto'} disabled={locked} onclick={() => (s.mode = 'auto')} data-testid="twr-mode-auto">Automatic</button>
			<button type="button" role="radio" aria-checked={s.mode === 'manual'} disabled={locked} onclick={() => (s.mode = 'manual')} data-testid="twr-mode-manual">Manual</button>
		</div>
		<p class="aq-small aq-muted">
			{s.mode === 'auto'
				? 'The run corrects at each level until a play meets the gate, then moves to the next; once full level gates, playback runs by itself.'
				: 'The run waits between plays: Iterate plays one correction, the level steps 1 dB or goes to a level of the table, and Play begins playback once the level has gated. The mode is fixed for the run.'}
		</p>
		<table class="aq-table levels" data-testid="twr-level-table">
			<thead><tr><th>Level</th><th class="r">dB</th><th aria-label="Remove"></th></tr></thead>
			<tbody>
				{#each s.levels_db as _l, i (i)}
					<tr>
						<td>{i === s.levels_db.length - 1 ? 'Full level' : `Level ${i + 1}`}</td>
						<td class="num"><NumberField id="ts-db-{i}" label="Level {i + 1}, dB" compact step={1} bind:value={s.levels_db[i]} errors={errs(`schedule.levels_db.${i}`)} disabled={locked} testid="twr-level-{i}" /></td>
						<td>
							<button class="aq-icon-btn" aria-label="Remove level {i + 1}" disabled={locked || s.levels_db.length <= 1} onclick={() => s.levels_db.splice(i, 1)}>
								<svg viewBox="0 0 12 12" width="10" height="10" aria-hidden="true"><path d="M3 3l6 6M9 3l-6 6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
							</button>
						</td>
					</tr>
				{/each}
			</tbody>
		</table>
		<div class="row">
			<button class="btn sm ghost" disabled={locked} onclick={addLevel} data-testid="twr-level-add">Add Level</button>
			<span class="aq-small aq-muted">{s.levels_db.length} {s.levels_db.length === 1 ? 'level' : 'levels'}, from -60 to 12 dB, rising.</span>
		</div>
		<Findings items={tableErrors.map((message) => ({ level: 'error' as const, message }))} />

		<div class="aq-group">
			<h3>When a Level Does Not Meet Its Gate</h3>
			<div class="aq-seg" role="radiogroup" aria-label="When a level does not meet its gate">
				<button type="button" role="radio" aria-checked={s.on_gate_fail === 'abort'} disabled={locked} onclick={() => (s.on_gate_fail = 'abort')} data-testid="twr-gate-fail-abort">End the run</button>
				<button type="button" role="radio" aria-checked={s.on_gate_fail === 'hold'} disabled={locked} onclick={() => (s.on_gate_fail = 'hold')} data-testid="twr-gate-fail-hold">Hold at the level</button>
			</div>
			<p class="aq-small aq-muted">
				{s.on_gate_fail === 'abort'
					? `After ${Number.isFinite(s.max_iterations) ? s.max_iterations : '-'} correction plays without a gate the run ends as not converged.`
					: `After ${Number.isFinite(s.max_iterations) ? s.max_iterations : '-'} correction plays without a gate the run holds at the level for a level move or an abort.`}
			</p>
		</div>
	</section>

	<div class="stack">
		<section class="aq-panel" aria-labelledby="ts-play" data-testid="twr-playback">
			<div class="aq-head"><h2 id="ts-play">Playback</h2><span class="aq-sub">Once full level has gated.</span></div>
			<div class="aq-seg" role="radiogroup" aria-label="Playback">
				{#each RUN_SCHEDULES as r (r.id)}
					<button type="button" role="radio" aria-checked={d.run.schedule === r.id} disabled={locked} onclick={() => (d.run.schedule = r.id)} data-testid="twr-run-{r.id}">{r.label}</button>
				{/each}
			</div>
			<div class="aq-fields spaced">
				{#if d.run.schedule === 'loop'}
					<NumberField id="ts-count" label="Plays" integer step={1} min={1} max={100000} bind:value={d.run.count} errors={errs('run.count')} disabled={locked} testid="twr-run-count" />
				{:else if d.run.schedule === 'duration'}
					<NumberField id="ts-dur" label="Section time" unit="s" step={10} min={0} max={86400} bind:value={d.run.duration_s} errors={errs('run.duration_s')} disabled={locked} testid="twr-run-duration" help="The section plays this long over its length times, rounded up." />
				{/if}
			</div>
			{#if dry}<p class="aq-small derived" data-testid="twr-plays-target">Playback plays: <span class="num">{dry.plays_target}</span>.</p>{/if}
			<label class="aq-switch spaced" for="ts-adapt">
				<input id="ts-adapt" type="checkbox" role="switch" checked={d.adaptation.during_playback} disabled={locked} onchange={(e) => (d.adaptation.during_playback = e.currentTarget.checked)} data-testid="twr-adapt" />
				<span class="track" aria-hidden="true"><span class="thumb"></span></span>
				<span>Keep correcting during playback</span>
				<span class="word">{d.adaptation.during_playback ? 'On' : 'Off'}</span>
			</label>
			<p class="help">On, the law is called after each playback play too. Off, the converged drive replays unchanged. Freeze holds the corrections off during a run either way.</p>
		</section>
		{#if dry}
			<section class="aq-panel" aria-label="Record">
				<p class="aq-small derived" data-testid="twr-schedule-record">Each play is {formatNumber(dry.grid.record_s, 4)} s of station time: the {formatNumber(dry.section.duration_s, 4)} s section between its pads.</p>
			</section>
		{/if}
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

	.spaced {
		margin-top: var(--sp-3);
	}

	.help {
		margin: 2px 0 0;
		font-size: var(--text-xs);
		color: var(--text-3);
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
