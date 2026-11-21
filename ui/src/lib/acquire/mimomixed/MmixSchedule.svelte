<script lang="ts">
	/**
	 * The level schedule (the shared editor: levels in dB rising to full level, automatic or manual),
	 * then identification, its gates and the analysis frame. Identification plays independent random
	 * on every drive at once, from its starting level up to its level below the reference in at most
	 * its steps, then gates on the band coherence and the model's condition; the run always holds for
	 * review after it, and past a failed gate only an override with the operator's words goes on.
	 */
	import NumberField from '../NumberField.svelte';
	import ScheduleEditor from '../ScheduleEditor.svelte';
	import type { MimoMixedStore } from './mmixStore.svelte';

	interface Props {
		mm: MimoMixedStore;
	}
	let { mm }: Props = $props();

	const d = $derived(mm.setup);
	const locked = $derived(mm.locked);
	const E = $derived(mm.fieldErrors);
	const df = $derived(d.estimation.fs_hz / d.estimation.n_pts);
	const identS = $derived((d.identification.frames * d.estimation.n_pts) / d.estimation.fs_hz);
</script>

<div class="stack">
	<ScheduleEditor acq={mm} />

	<section class="aq-panel" aria-labelledby="mmix-ident" data-testid="mmix-identification">
		<div class="aq-head"><h2 id="mmix-ident">Identification</h2><span class="aq-sub">{Number.isFinite(identS) ? `About ${identS.toFixed(1)} s of independent random on every drive.` : ''}</span></div>
		<div class="aq-fields">
			<NumberField id="mmix-id-level" label="Level below the reference" unit="dB" min={0} step={1} bind:value={d.identification.below_reference_db} errors={E['identification.below_reference_db']} disabled={locked} testid="mmix-id-level" help="The level identification climbs to, under full level." />
			<NumberField id="mmix-id-start" label="Starting drive" unit="V rms" min={0} step={0.005} bind:value={d.identification.start_vrms} errors={E['identification.start_vrms']} disabled={locked} testid="mmix-id-start" />
			<NumberField id="mmix-id-tol" label="Level within" unit="dB" min={0} step={0.5} bind:value={d.identification.tolerance_db} errors={E['identification.tolerance_db']} disabled={locked} testid="mmix-id-tol" />
			<NumberField id="mmix-id-steps" label="Steps, at most" integer min={1} bind:value={d.identification.max_steps} errors={E['identification.max_steps']} disabled={locked} testid="mmix-id-steps" />
			<NumberField id="mmix-id-frames" label="Frames" integer min={8} bind:value={d.identification.frames} errors={E['identification.frames']} disabled={locked} testid="mmix-id-frames" />
			<NumberField id="mmix-id-noise" label="Noise frames" integer min={1} bind:value={d.identification.noise_frames} errors={E['identification.noise_frames']} disabled={locked} testid="mmix-id-noise" help="Frames read with nothing driven, for the noise floor." />
		</div>
	</section>

	<section class="aq-panel" aria-labelledby="mmix-gates" data-testid="mmix-gates">
		<div class="aq-head"><h2 id="mmix-gates">Identification Gates</h2><span class="aq-sub">The run always holds for review after identification; a failed gate holds it until an override with your words.</span></div>
		<div class="aq-fields">
			<NumberField id="mmix-id-coh" label="Coherence gate" min={0} max={1} step={0.05} bind:value={d.gates.coherence_threshold} errors={E['gates.coherence_threshold']} disabled={locked} testid="mmix-id-coherence" help="Every control's band coherence must reach it." />
			<NumberField id="mmix-id-cond" label="Condition limit" min={1} step={100} bind:value={d.gates.condition_threshold} errors={E['gates.condition_threshold']} disabled={locked} testid="mmix-id-condition" />
		</div>
	</section>

	<section class="aq-panel" aria-labelledby="mmix-frame" data-testid="mmix-estimation">
		<div class="aq-head"><h2 id="mmix-frame">Analysis Frame</h2><span class="aq-sub">{Number.isFinite(df) ? `Lines every ${df.toFixed(3)} Hz; a loop every ${(d.estimation.n_pts / d.estimation.fs_hz).toFixed(2)} s.` : ''}</span></div>
		<div class="aq-fields">
			<NumberField id="mmix-fs" label="Sample rate" unit="Hz" step={1024} min={1024} bind:value={d.estimation.fs_hz} errors={E['estimation.fs_hz']} disabled={locked} testid="mmix-fs" />
			<NumberField id="mmix-npts" label="Frame" unit="points" integer step={512} min={64} bind:value={d.estimation.n_pts} errors={E['estimation.n_pts']} disabled={locked} testid="mmix-npts" />
			<NumberField id="mmix-fpl" label="Frames per loop" integer min={1} bind:value={d.estimation.frames_per_loop} errors={E['estimation.frames_per_loop']} disabled={locked} testid="mmix-fpl" />
			<NumberField id="mmix-mem" label="Loops averaged" integer min={1} bind:value={d.estimation.memory_loops} errors={E['estimation.memory_loops']} disabled={locked} testid="mmix-mem" />
		</div>
	</section>
</div>

<style>
	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
	}
</style>
