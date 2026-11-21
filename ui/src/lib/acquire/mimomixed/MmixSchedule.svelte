<script lang="ts">
	/**
	 * The level schedule (the shared editor: levels in dB rising to full level, automatic or manual),
	 * then identification and the analysis frame, as MIMO Random's. Identification plays independent
	 * random on every drive at once at its own level, then gates on the worst multiple coherence and
	 * on the model's condition; past a failed gate the run proceeds only with the operator's words.
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
			<NumberField id="mmix-id-level" label="Level" unit="dB" step={1} bind:value={d.identification.level_db} errors={E['identification.level_db']} disabled={locked} testid="mmix-id-level" help="Below full level, against the floor's reference." />
			<NumberField id="mmix-id-frames" label="Frames" integer min={8} bind:value={d.identification.frames} errors={E['identification.frames']} disabled={locked} testid="mmix-id-frames" />
			<NumberField id="mmix-id-coh" label="Coherence gate" min={0} max={1} step={0.05} bind:value={d.identification.coherence_floor} errors={E['identification.coherence_floor']} disabled={locked} testid="mmix-id-coherence" help="The worst multiple coherence across the band must reach it." />
			<NumberField id="mmix-id-cond" label="Condition limit" min={1} step={100} bind:value={d.identification.condition_limit} errors={E['identification.condition_limit']} disabled={locked} testid="mmix-id-condition" />
			<label class="aq-check"><input type="checkbox" bind:checked={d.identification.review} disabled={locked} data-testid="mmix-id-review" /> Hold for review after identification</label>
		</div>
	</section>

	<section class="aq-panel" aria-labelledby="mmix-frame" data-testid="mmix-estimation">
		<div class="aq-head"><h2 id="mmix-frame">Analysis Frame</h2><span class="aq-sub">{Number.isFinite(df) ? `Lines every ${df.toFixed(3)} Hz; a loop every ${(d.estimation.n_pts / d.estimation.fs_hz).toFixed(2)} s.` : ''}</span></div>
		<div class="aq-fields">
			<NumberField id="mmix-fs" label="Sample rate" unit="Hz" step={1024} min={1024} bind:value={d.estimation.fs_hz} errors={E['estimation.fs_hz']} disabled={locked} testid="mmix-fs" />
			<NumberField id="mmix-npts" label="Frame" unit="points" integer step={512} min={256} bind:value={d.estimation.n_pts} errors={E['estimation.n_pts']} disabled={locked} testid="mmix-npts" />
			<NumberField id="mmix-fpl" label="Frames per loop" integer min={1} bind:value={d.estimation.frames_per_loop} errors={E['estimation.frames_per_loop']} disabled={locked} testid="mmix-fpl" />
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
