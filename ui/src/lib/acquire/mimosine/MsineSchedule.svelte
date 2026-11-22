<script lang="ts">
	/**
	 * The level schedule and the identification before it. The levels rise to full level (the last);
	 * in the automatic mode each level below full is held for a count of one-way sweeps (a sweep
	 * only) or seconds, and full level for its own; in the manual mode the operator steps them. One
	 * schedule for every control. The identification plays independent random on every drive at
	 * once, at a level under the references, and holds for review unless told not to.
	 */
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import { db } from '../words';
	import type { MimoSineStore } from './msineStore.svelte';

	interface Props {
		ms: MimoSineStore;
	}
	let { ms }: Props = $props();

	const d = $derived(ms.setup);
	const s = $derived(d.schedule);
	const id = $derived(d.identification);
	const locked = $derived(ms.locked);
	const E = $derived(ms.fieldErrors);
	const errs = (k: string) => E[k] ?? [];
	const sweep = $derived(d.test.kind === 'sweep');

	function addLevel() {
		if (locked) return;
		const last = s.levels.at(-1) ?? 0;
		s.levels.push(Math.min(12, last + 3));
	}
</script>

<div class="stack">
	<section class="aq-panel" aria-labelledby="msine-sch-title" data-testid="msine-schedule">
		<div class="aq-head"><h2 id="msine-sch-title">Level Schedule</h2><span class="aq-sub">dB under the references; the last level is full level.</span></div>
		<div class="kinds" role="radiogroup" aria-label="Ladder mode">
			<label class="kind"><input type="radio" name="msine-mode" value="auto" bind:group={s.mode} disabled={locked} /> Automatic</label>
			<label class="kind"><input type="radio" name="msine-mode" value="manual" bind:group={s.mode} disabled={locked} data-testid="msine-manual" /> By hand</label>
		</div>
		<div class="levels" data-testid="msine-levels">
			{#each s.levels as _, i (i)}
				<div class="lv">
					<NumberField compact id="msine-lv-{i}" label="Level {i + 1}" unit="dB" step={1} min={-60} max={12} bind:value={s.levels[i]} errors={errs(`schedule.levels.${i}`)} disabled={locked} />
					<button class="aq-icon-btn" aria-label="Remove level {i + 1}" disabled={locked || s.levels.length <= 1} onclick={() => s.levels.splice(i, 1)}>
						<svg viewBox="0 0 12 12" width="10" height="10" aria-hidden="true"><path d="M3 3l6 6M9 3l-6 6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
					</button>
				</div>
			{/each}
			<button class="btn sm ghost" disabled={locked} onclick={addLevel}>Add Level</button>
		</div>
		<Findings items={errs('schedule.levels').map((message) => ({ level: 'error' as const, message }))} />
		<div class="aq-fields">
			<div class="tf">
				<label for="msine-start-db" class="lbl">Start at</label>
				<select id="msine-start-db" class="field" bind:value={s.start_db} disabled={locked}>
					{#each s.levels as l, i (i)}<option value={l}>{db(l)}</option>{/each}
				</select>
				<Findings items={errs('schedule.start_db').map((message) => ({ level: 'error' as const, message }))} />
			</div>
			{#if s.mode === 'auto'}
				{#each [{ key: 'per_level', label: 'Each level below full' }, { key: 'full_level', label: 'Full level' }] as r (r.key)}
					{@const rung = s[r.key as 'per_level' | 'full_level']}
					<div class="tf">
						<label for="msine-{r.key}-by" class="lbl">{r.label}, held for</label>
						<select id="msine-{r.key}-by" class="field" bind:value={rung.by} disabled={locked}>
							{#if sweep}<option value="sweeps">One-way sweeps</option>{/if}
							<option value="seconds">Seconds</option>
						</select>
					</div>
					<NumberField id="msine-{r.key}-n" label="{rung.by === 'sweeps' ? 'Sweeps' : 'Seconds'} {r.key === 'per_level' ? 'at each level' : 'at full level'}" integer={rung.by === 'sweeps'} step={1} min={0} bind:value={rung.n} errors={errs(`schedule.${r.key}`)} disabled={locked} />
				{/each}
			{/if}
		</div>
	</section>

	<section class="aq-panel" aria-labelledby="msine-id-title" data-testid="msine-identification">
		<div class="aq-head"><h2 id="msine-id-title">Identification</h2><span class="aq-sub">Independent random on every drive at once, before the tone, at low level.</span></div>
		<div class="aq-fields">
			<NumberField id="msine-id-level" label="Level under the references" unit="dB" step={1} min={-60} max={0} bind:value={id.level_db} errors={errs('identification.level_db')} disabled={locked} />
			<NumberField id="msine-id-resolution" label="Line spacing" unit="Hz" step={0.05} min={0.05} max={4} bind:value={id.resolution_hz} errors={errs('identification.resolution_hz')} help="Each record lasts about its inverse; a lightly damped mode needs several lines across its bandwidth." disabled={locked} />
			<NumberField id="msine-id-frames" label="Records averaged" integer nullable="24 or six per drive" step={8} min={1} max={4096} bind:value={id.frames} errors={errs('identification.frames')} help="Each record plays twice: once to settle, once to measure." disabled={locked} />
			<NumberField id="msine-id-trust" label="Trust a line from coherence" step={0.05} min={0} max={1} bind:value={id.trust_coherence} errors={errs('identification.trust_coherence')} help="A line where a control's multiple coherence is under it is not trusted." disabled={locked} />
			<NumberField id="msine-id-gate" label="Every control's coherence at least" step={0.05} min={0} max={1} bind:value={id.gate_coherence} errors={errs('identification.gate_coherence')} help="The band-mean coherence every control must reach; under it the run ends held." disabled={locked} />
		</div>
		<label class="aq-switch" for="msine-review">
			<input id="msine-review" type="checkbox" role="switch" bind:checked={id.review} disabled={locked} data-testid="msine-review" />
			<span class="track" aria-hidden="true"><span class="thumb"></span></span>
			<span>Hold for review after identification</span>
			<span class="word">{id.review ? 'On' : 'Off'}</span>
		</label>
	</section>
</div>

<style>
	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
	}

	.kinds {
		display: flex;
		gap: var(--sp-4);
		margin-bottom: var(--sp-3);
	}

	.kind {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		font-size: var(--text-sm);
	}

	.levels {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2);
		margin-bottom: var(--sp-3);
	}

	.lv {
		display: flex;
		align-items: center;
		gap: 2px;
	}

	.tf {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.lbl {
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}
</style>
