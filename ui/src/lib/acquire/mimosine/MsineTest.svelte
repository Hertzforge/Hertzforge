<script lang="ts">
	/**
	 * The test: a sweep (logarithmic or linear, up or down, ended by a count of one-way sweeps, a
	 * time, or whichever comes first), a dwell at one frequency for a time or a number of cycles, or
	 * a stepped sine on a grid or a list of frequencies. Each kind keeps its settings when another is
	 * chosen. Then the rate asked of the station and the control tick.
	 */
	import { untrack } from 'svelte';
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import { TEST_KINDS } from './msineSetup';
	import type { MimoSineStore } from './msineStore.svelte';

	interface Props {
		ms: MimoSineStore;
	}
	let { ms }: Props = $props();

	const d = $derived(ms.setup);
	const t = $derived(d.test);
	const locked = $derived(ms.locked);
	const E = $derived(ms.fieldErrors);
	const errs = (k: string) => E[k] ?? [];
	const parse = (text: string) =>
		text
			.split(/[\s,;]+/)
			.filter((x) => x.trim())
			.map(Number);
	let listText = $state('');
	// The field follows the draft when the draft changes from elsewhere (a setup opened), not while typing.
	$effect(() => {
		const joined = t.stepped.frequencies_hz.join(', ');
		untrack(() => {
			if (parse(listText).join(', ') !== joined) listText = joined;
		});
	});

	function setList(text: string) {
		listText = text;
		t.stepped.frequencies_hz = parse(text);
	}
</script>

<section class="aq-panel" aria-labelledby="msine-test-title" data-testid="msine-test">
	<div class="aq-head"><h2 id="msine-test-title">Test</h2><span class="aq-sub">One tone at a time, on every drive at once.</span></div>
	<div class="kinds" role="radiogroup" aria-label="Kind of test">
		{#each TEST_KINDS as k (k.id)}
			<label class="kind"><input type="radio" name="msine-kind" value={k.id} bind:group={t.kind} disabled={locked} data-testid="msine-kind-{k.id}" /> {k.label}</label>
		{/each}
	</div>

	{#if t.kind === 'sweep'}
		<div class="aq-fields">
			<NumberField id="msine-lo" label="Band from" unit="Hz" bind:value={t.band_hz[0]} errors={errs('test.band_hz.0')} disabled={locked} testid="msine-band-lo" />
			<NumberField id="msine-hi" label="Band to" unit="Hz" bind:value={t.band_hz[1]} errors={errs('test.band_hz.1')} disabled={locked} testid="msine-band-hi" />
			<div class="tf">
				<label for="msine-law" class="lbl">Sweep</label>
				<select id="msine-law" class="field" bind:value={t.sweep.law} disabled={locked}><option value="log">Logarithmic</option><option value="linear">Linear</option></select>
			</div>
			{#if t.sweep.law === 'log'}
				<NumberField id="msine-rate" label="Rate" unit="oct/min" step={0.5} bind:value={t.sweep.oct_per_min} errors={errs('test.rate')} disabled={locked} testid="msine-rate" />
			{:else}
				<NumberField id="msine-rate" label="Rate" unit="Hz/s" step={1} bind:value={t.sweep.hz_per_s} errors={errs('test.rate')} disabled={locked} testid="msine-rate" />
			{/if}
			<NumberField id="msine-start" label="Start" unit="Hz" nullable="The band's low edge" bind:value={t.sweep.start_hz} errors={errs('test.start_hz')} disabled={locked} />
			<div class="tf">
				<label for="msine-dir" class="lbl">First leg</label>
				<select id="msine-dir" class="field" bind:value={t.sweep.direction} disabled={locked}><option value="up">Up</option><option value="down">Down</option></select>
			</div>
			<div class="tf">
				<label for="msine-end" class="lbl">Ends after</label>
				<select id="msine-end" class="field" bind:value={t.sweep.end_by} disabled={locked}><option value="sweeps">A count of one-way sweeps</option><option value="duration">A test time</option><option value="both">Whichever comes first</option></select>
			</div>
			{#if t.sweep.end_by !== 'duration'}<NumberField id="msine-sweeps" label="One-way sweeps" integer step={1} min={1} bind:value={t.sweep.sweeps} errors={errs('test.end.sweeps')} disabled={locked} />{/if}
			{#if t.sweep.end_by !== 'sweeps'}<NumberField id="msine-dur" label="Test time" unit="s" step={60} bind:value={t.sweep.duration_s} errors={errs('test.end.duration_s')} disabled={locked} />{/if}
		</div>
		<p class="aq-small aq-muted">Counted over the whole run from engage, at every level. A sweep that passes a light resonance in less than two of its own time constants is flagged before arming: there a slower sweep is the remedy.</p>
	{:else if t.kind === 'dwell'}
		<div class="aq-fields">
			<NumberField id="msine-dwell-f" label="Frequency" unit="Hz" bind:value={t.dwell.freq_hz} errors={errs('test.freq_hz')} disabled={locked} testid="msine-dwell-hz" />
			<div class="tf">
				<label for="msine-dwell-by" class="lbl">For</label>
				<select id="msine-dwell-by" class="field" bind:value={t.dwell.by} disabled={locked}><option value="seconds">A time</option><option value="cycles">A number of cycles</option></select>
			</div>
			{#if t.dwell.by === 'seconds'}
				<NumberField id="msine-dwell-s" label="Time at full level" unit="s" bind:value={t.dwell.seconds} errors={errs('test.end.seconds')} disabled={locked} />
			{:else}
				<NumberField id="msine-dwell-c" label="Cycles at full level" integer step={100} bind:value={t.dwell.cycles} errors={errs('test.end.cycles')} disabled={locked} />
			{/if}
		</div>
	{:else}
		<div class="kinds" role="radiogroup" aria-label="Steps">
			<label class="kind"><input type="radio" name="msine-on" value="grid" bind:group={t.stepped.on} disabled={locked} data-testid="msine-steps-grid" /> On a grid</label>
			<label class="kind"><input type="radio" name="msine-on" value="list" bind:group={t.stepped.on} disabled={locked} data-testid="msine-steps-list" /> On a list</label>
		</div>
		<div class="aq-fields">
			{#if t.stepped.on === 'grid'}
				<NumberField id="msine-slo" label="Band from" unit="Hz" bind:value={t.band_hz[0]} errors={errs('test.band_hz.0')} disabled={locked} />
				<NumberField id="msine-shi" label="Band to" unit="Hz" bind:value={t.band_hz[1]} errors={errs('test.band_hz.1')} disabled={locked} />
				<NumberField id="msine-spo" label="Steps an octave" step={1} min={0} bind:value={t.stepped.steps_per_octave} errors={errs('test.steps_per_octave')} disabled={locked} />
				<div class="tf">
					<label for="msine-spacing" class="lbl">Spacing</label>
					<select id="msine-spacing" class="field" bind:value={t.stepped.spacing} disabled={locked}><option value="log">Logarithmic</option><option value="linear">Linear</option></select>
				</div>
			{:else}
				<div class="tf wide">
					<label for="msine-list" class="lbl">Frequencies, Hz, played in the order given</label>
					<input id="msine-list" class="field num" value={listText} oninput={(e) => setList(e.currentTarget.value)} disabled={locked} data-testid="msine-list" />
					<Findings items={errs('test.frequencies_hz').map((message) => ({ level: 'error' as const, message }))} />
				</div>
			{/if}
			<div class="tf">
				<label for="msine-step-by" class="lbl">Dwell at each step</label>
				<select id="msine-step-by" class="field" bind:value={t.stepped.by} disabled={locked}>
					{#if t.stepped.on === 'grid'}<option value="computed">Computed per step</option>{/if}
					<option value="seconds">A time</option>
					<option value="cycles">A number of cycles</option>
				</select>
				<Findings items={errs('test.dwell').map((message) => ({ level: 'error' as const, message }))} />
			</div>
			{#if t.stepped.by === 'seconds'}<NumberField id="msine-step-s" label="Time a step" unit="s" bind:value={t.stepped.seconds} errors={errs('test.dwell.seconds')} disabled={locked} />{/if}
			{#if t.stepped.by === 'cycles'}<NumberField id="msine-step-c" label="Cycles a step" integer step={100} bind:value={t.stepped.cycles} errors={errs('test.dwell.cycles')} disabled={locked} />{/if}
		</div>
	{/if}
	<Findings items={[...errs('test'), ...errs('test.kind')].map((message) => ({ level: 'error' as const, message }))} />

	<div class="aq-group">
		<h3>Acquisition</h3>
		<div class="aq-fields">
			<NumberField id="msine-fs" label="Rate asked of the station" unit="Hz" nullable="The usual rate, 16384" step={1024} bind:value={d.acquisition.fs_hz} errors={errs('acquisition.fs_hz')} disabled={locked} />
			<NumberField id="msine-tick" label="Control tick" unit="ms" step={1} min={2} max={1000} bind:value={d.acquisition.tick_ms} errors={errs('acquisition.tick_ms')} help="Converted to whole frames at the rate the station adopts; 8 ms at least for a law file." disabled={locked} />
		</div>
	</div>
</section>

<style>
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

	.tf {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.wide {
		grid-column: 1 / -1;
	}

	.lbl {
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}
</style>
