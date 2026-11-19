<script lang="ts">
	/**
	 * What protects the shaker and the article: the drive ceiling in volts peak (no default: it is the
	 * amplifier's input, so the setup asks for it), a per-sample clip, rms limits on the drive and the
	 * response, the envelope around the target that raises an alarm and aborts, the divergence check,
	 * and the feasibility ceilings the prepared section's kinematics are held to at arm.
	 */
	import NumberField from '../NumberField.svelte';
	import type { TwrStore } from './twrStore.svelte';

	interface Props {
		twr: TwrStore;
	}
	let { twr }: Props = $props();

	const d = $derived(twr.setup);
	const lim = $derived(d.limits);
	const f = $derived(d.feasibility);
	const locked = $derived(twr.locked);
	const E = $derived(twr.fieldErrors);
	const errs = (k: string) => E[k] ?? [];
	const unit = $derived(twr.unit);
	const kin = $derived(twr.prepared?.derived.kinematics ?? null);
</script>

<div class="cols">
	<div class="stack">
		<section class="aq-panel" aria-labelledby="tl-drive" data-testid="twr-drive-limits">
			<div class="aq-head"><h2 id="tl-drive">Drive</h2><span class="aq-sub">In volts at the amplifier's input.</span></div>
			<p class="aq-lead aq-small">The drive ceiling has no default: give the value for your amplifier. A drive whose peak is over it is refused and the run aborts; it is never clipped to fit. It holds for identification and for every play.</p>
			<div class="aq-fields">
				<NumberField id="tl-ceil" label="Drive ceiling" unit="V pk" step={0.1} min={0} max={10} bind:value={lim.drive_vpk_ceiling} errors={errs('limits.drive_vpk_ceiling')} disabled={locked} testid="twr-drive-ceiling" help="At most 10 V. In automatic, a full-level drive predicted over it ends the run before any play." />
				<NumberField id="tl-clip" label="Per-sample clip" unit="V" nullable="No clip" fallback={Number.isFinite(lim.drive_vpk_ceiling) ? lim.drive_vpk_ceiling * 1.6 : 1} step={0.1} min={0} bind:value={lim.drive_clip_vpk} errors={errs('limits.drive_clip_vpk')} disabled={locked} testid="twr-drive-clip" help="Holds each sample under it; the law sees the drive as clipped." />
				<NumberField id="tl-drms" label="Drive rms limit" unit="V rms" nullable="None" fallback={1} step={0.1} min={0} bind:value={lim.drive_vrms_limit} errors={errs('limits.drive_vrms_limit')} disabled={locked} testid="twr-drive-rms" help="A drive over it is refused and the run aborts." />
				<NumberField id="tl-rrms" label="Response rms limit" unit="{unit} rms" nullable="None" fallback={1} step={0.1} min={0} bind:value={lim.response_rms_limit} errors={errs('limits.response_rms_limit')} disabled={locked} testid="twr-response-rms" help="A response over it aborts the run." />
			</div>
		</section>

		<section class="aq-panel" aria-labelledby="tl-env" data-testid="twr-envelope">
			<div class="aq-head"><h2 id="tl-env">Envelope and Divergence</h2><span class="aq-sub">Around the target at the level, a share of its peak.</span></div>
			<p class="aq-lead aq-small">The response is checked against the bands block by block as it arrives, and again after the play. Outside the alarm band raises the alarm; outside the abort band for the samples in a row below, the play is cancelled with the station's own stop and the run aborts.</p>
			<div class="aq-fields">
				<NumberField id="tl-alarm" label="Alarm band" step={0.05} min={0} max={1} bind:value={lim.envelope_alarm_frac} errors={errs('limits.envelope_alarm_frac')} disabled={locked} testid="twr-envelope-alarm" help="Usual: 0.25." />
				<NumberField id="tl-abort" label="Abort band" step={0.05} min={0} max={1} bind:value={lim.envelope_abort_frac} errors={errs('limits.envelope_abort_frac')} disabled={locked} testid="twr-envelope-abort" help="At least the alarm band. Usual: 0.5." />
				<NumberField id="tl-deb" label="Samples in a row outside it" integer step={1} min={1} max={10000} bind:value={lim.envelope_debounce_samples} errors={errs('limits.envelope_debounce_samples')} disabled={locked} testid="twr-envelope-debounce" help="Usual: 8." />
				<NumberField id="tl-divn" label="Plays with a rising error" integer step={1} min={1} bind:value={lim.divergence_plays} errors={errs('limits.divergence_plays')} disabled={locked} testid="twr-divergence-plays" help="Correction plays in a row at one level, above the gate, that end the run. Usual: 3." />
				<NumberField id="tl-divr" label="Rise each, share of the last error" step={0.05} min={0} bind:value={lim.divergence_rise_frac} errors={errs('limits.divergence_rise_frac')} disabled={locked} testid="twr-divergence-rise" help="Usual: 0.1." />
			</div>
		</section>
	</div>

	<section class="aq-panel" aria-labelledby="tl-feas" data-testid="twr-feasibility">
		<div class="aq-head"><h2 id="tl-feas">Feasibility</h2><span class="aq-sub">The prepared section at full level, checked at arm.</span></div>
		<p class="aq-lead aq-small">A ceiling the section's kinematics pass blocks arming until the operator acknowledges it in words, which the run log keeps. Leave a ceiling off to not check it.</p>
		<div class="aq-fields">
			<NumberField id="tl-acc" label="Peak acceleration" unit="{unit} pk" nullable="Not checked" fallback={10} step={1} min={0} bind:value={f.peak_accel} errors={errs('feasibility.peak_accel')} disabled={locked} testid="twr-feas-accel" />
			<NumberField id="tl-vel" label="Peak velocity" unit="in/s pk" nullable="Not checked" fallback={50} step={1} min={0} bind:value={f.peak_velocity_ips} errors={errs('feasibility.peak_velocity_ips')} disabled={locked} testid="twr-feas-velocity" />
			<NumberField id="tl-disp" label="Displacement" unit="in pk-pk" nullable="Not checked" fallback={1} step={0.1} min={0} bind:value={f.disp_pkpk_in} errors={errs('feasibility.disp_pkpk_in')} disabled={locked} testid="twr-feas-disp" />
			<NumberField id="tl-hp" label="Kinematics high-pass" unit="Hz" step={1} min={0} bind:value={f.kinematics_hp_hz} errors={errs('feasibility.kinematics_hp_hz')} disabled={locked} testid="twr-feas-hp" help="The kinematics are integrated after it. Usual: 5 Hz." />
		</div>
		{#if kin}
			<p class="aq-small aq-muted" data-testid="twr-feas-now">The prepared section now: {kin.peak_accel} {unit} pk, {kin.peak_velocity_ips} in/s pk, {kin.disp_pkpk_in} in pk-pk.</p>
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

	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
	}

	@media (max-width: 1240px) {
		.cols {
			grid-template-columns: minmax(0, 1fr);
		}
	}
</style>
