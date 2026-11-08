<script lang="ts">
	/**
	 * The level schedule and the identification before it. Levels are relative to the profile, in
	 * dB, rising; the last is full level. In automatic the ladder holds each level for some one-way
	 * sweeps (a sweep test only) or seconds, then moves on; in manual it moves only when the operator
	 * says. Before any level, the control service identifies the system with a noise burst under full
	 * level, checks the coherence it must reach, and holds for review when asked.
	 */
	import { formatNumber } from '$lib/viz/format';
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import { db } from '../words';
	import type { SineStore } from './sineStore.svelte';

	interface Props {
		sine: SineStore;
	}
	let { sine }: Props = $props();

	const s = $derived(sine.setup.schedule);
	const id = $derived(sine.setup.identification);
	const sweep = $derived(sine.setup.test.kind === 'sweep');
	const locked = $derived(sine.locked);
	const E = $derived(sine.fieldErrors);
	const errs = (k: string) => E[k] ?? [];
	const dry = $derived(sine.check && sine.checkedKey === sine.key ? sine.check.derived : null);

	function addLevel() {
		const last = s.levels.at(-1) ?? -6;
		s.levels.push(Number.isFinite(last) ? Math.min(0, last + 3) : 0);
	}

	function removeLevel(i: number) {
		const was = s.levels[i];
		s.levels.splice(i, 1);
		if (s.start_db === was) s.start_db = s.levels[0];
	}
</script>

<div class="cols">
	<section class="aq-panel" aria-labelledby="ns-title" data-testid="sine-schedule">
		<div class="aq-head"><h2 id="ns-title">Level Schedule</h2><span class="aq-sub">Relative to the profile; the last level is full level.</span></div>
		<div class="sel">
			<span class="lbl" id="ns-mode-l">Ladder</span>
			<div class="aq-seg" role="radiogroup" aria-labelledby="ns-mode-l">
				<button type="button" role="radio" aria-checked={s.mode === 'auto'} disabled={locked} onclick={() => (s.mode = 'auto')} data-testid="sine-mode-auto">Automatic</button>
				<button type="button" role="radio" aria-checked={s.mode === 'manual'} disabled={locked} onclick={() => (s.mode = 'manual')} data-testid="sine-mode-manual">Manual</button>
			</div>
			<p class="help">{s.mode === 'auto' ? 'The ladder moves on by itself after each level.' : 'The ladder moves only when you step the level during the run.'}</p>
		</div>

		<table class="aq-table levels" aria-label="Levels">
			<thead><tr><th>Level</th><th class="num">dB</th><th aria-label="Remove"></th></tr></thead>
			<tbody>
				{#each s.levels as _l, i (i)}
					<tr data-testid="sine-level-{i}">
						<td>{i === s.levels.length - 1 ? 'Full level' : `Level ${i + 1}`}</td>
						<td class="num"><NumberField id="ns-l-{i}" label="Level {i + 1}" compact unit="dB" step={1} max={0} value={s.levels[i]} onchange={(v) => (s.levels[i] = v ?? NaN)} errors={errs(`schedule.levels.${i}`)} disabled={locked} testid="sine-level-{i}-db" /></td>
						<td>
							<button class="aq-icon-btn" aria-label="Remove level {i + 1}" disabled={locked || s.levels.length <= 1} onclick={() => removeLevel(i)}>
								<svg viewBox="0 0 12 12" width="10" height="10" aria-hidden="true"><path d="M3 3l6 6M9 3l-6 6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
							</button>
						</td>
					</tr>
				{/each}
			</tbody>
		</table>
		<button class="btn sm ghost" disabled={locked} onclick={addLevel} data-testid="sine-level-add">Add Level</button>
		<Findings items={errs('schedule.levels').map((message) => ({ level: 'error' as const, message }))} />

		<div class="aq-fields spaced">
			<div class="sel">
				<label for="ns-start" class="lbl">Engage at</label>
				<select id="ns-start" class="field num" value={s.start_db} onchange={(e) => (s.start_db = Number(e.currentTarget.value))} disabled={locked} data-testid="sine-start-level">
					{#if !s.levels.includes(s.start_db)}<option value={s.start_db}>{db(s.start_db)}</option>{/if}
					{#each s.levels as l, i (i)}<option value={l}>{db(l)}</option>{/each}
				</select>
				<Findings items={errs('schedule.start_db').map((message) => ({ level: 'error' as const, message }))} />
			</div>
		</div>

		{#if s.mode === 'auto'}
			<div class="aq-group sep">
				<h3>How Long Each Level Holds</h3>
				<div class="aq-fields">
					{#each [['per_level', 'Each level but the last'], ['full_level', 'Full level']] as [key, label] (key)}
						{@const r = s[key as 'per_level' | 'full_level']}
						<div class="sel">
							<span class="lbl" id="ns-{key}-l">{label}</span>
							<div class="aq-seg sm" role="radiogroup" aria-labelledby="ns-{key}-l">
								<button type="button" role="radio" aria-checked={r.by === 'sweeps'} disabled={locked} title={sweep ? undefined : 'Sweeps count in a sweep test only.'} onclick={() => (r.by = 'sweeps')} data-testid="sine-{key}-sweeps">Sweeps</button>
								<button type="button" role="radio" aria-checked={r.by === 'seconds'} disabled={locked} onclick={() => (r.by = 'seconds')} data-testid="sine-{key}-seconds">Seconds</button>
							</div>
						</div>
						<NumberField id="ns-{key}-n" label="{label}, {r.by === 'sweeps' ? 'sweeps' : 'seconds'}" unit={r.by === 'sweeps' ? undefined : 's'} integer={r.by === 'sweeps'} step={r.by === 'sweeps' ? 1 : 10} min={0} bind:value={r.n} errors={errs(`schedule.${key}`)} disabled={locked} testid="sine-{key}-n" />
					{/each}
				</div>
				<p class="aq-small aq-muted">The run itself ends at the test's end. An end that would come before full level has held is refused.</p>
			</div>
		{/if}
		{#if dry}
			<p class="aq-small derived" data-testid="sine-schedule-time">{dry.test_s === null ? 'In manual the test time is set by hand.' : `The schedule takes ${formatNumber(dry.test_s, 4)} s of test time, holds excluded.`}</p>
		{/if}
	</section>

	<section class="aq-panel" aria-labelledby="ns-id" data-testid="sine-identification">
		<div class="aq-head"><h2 id="ns-id">Identification</h2><span class="aq-sub">Before any level, at a low drive.</span></div>
		<p class="aq-lead aq-small lead">The control service plays a white-noise burst under full level and measures how each control channel answers, and the delay between the drive and the response. The control law's first drive comes from that gain at the start frequency. Nothing is driven at test level until identification is done.</p>
		<div class="aq-fields">
			<NumberField id="ns-lv" label="Burst level under full" unit="dB" step={1} max={0} min={-60} bind:value={id.level_db} errors={errs('identification.level_db')} disabled={locked} testid="sine-id-level" help="From -60 to 0 dB. Usual: -20 dB." />
			<NumberField id="ns-gate" label="Coherence it must reach" step={0.05} min={0} max={1} bind:value={id.gate_coherence} errors={errs('identification.gate_coherence')} disabled={locked} testid="sine-id-gate" help="Over the band. Under it the run is held and never drives at test level. Usual: 0.5." />
		</div>
		<label class="aq-switch tog" for="ns-rev">
			<input id="ns-rev" type="checkbox" role="switch" checked={id.review} disabled={locked} onchange={(e) => (id.review = e.currentTarget.checked)} data-testid="sine-id-review" />
			<span class="track" aria-hidden="true"><span class="thumb"></span></span>
			<span>Hold for review after identification</span>
			<span class="word">{id.review ? 'On' : 'Off'}</span>
		</label>
		<p class="help">On: the run holds with the identified gain and its coherence on screen, and nothing drives until you proceed. Off: the tone engages as soon as the coherence check passes.</p>
		{#if dry}
			<p class="aq-small derived" data-testid="sine-id-length">The burst lasts about {formatNumber(dry.identification_s, 3)} s at this band's low edge.</p>
		{/if}
	</section>
</div>

<style>
	.cols {
		display: grid;
		grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
		gap: var(--sp-3);
		align-items: start;
	}

	.sel {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.lbl {
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}

	.help {
		margin: 2px 0 0;
		font-size: var(--text-xs);
		color: var(--text-3);
	}

	.levels {
		margin: var(--sp-3) 0 var(--sp-2);
		max-width: 360px;
	}

	.spaced {
		margin-top: var(--sp-3);
	}

	.tog {
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
	/* A group after fields or a table is set off as one after another group is. */
	.sep {
		margin-top: var(--sp-5);
		padding-top: var(--sp-4);
		border-top: 1px solid var(--line-1);
	}

	.lead {
		margin: 0 0 var(--sp-3);
	}
</style>
