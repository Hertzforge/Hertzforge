<script lang="ts">
	/**
	 * Each control's reference: its amplitude at full level against frequency with its alarm and
	 * abort bands (the profile, edited as Sine's), and its phase against frequency relative to the
	 * tone. Or, for the whole run, amplitudes only: the law then chooses the phases, and a law that
	 * cannot is not offered. One control's reference can be copied to every other.
	 */
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import SineProfileTable from '../sine/SineProfileTable.svelte';
	import type { MimoSineStore } from './msineStore.svelte';

	interface Props {
		ms: MimoSineStore;
	}
	let { ms }: Props = $props();

	const d = $derived(ms.setup);
	const locked = $derived(ms.locked);
	const E = $derived(ms.fieldErrors);
	const W = $derived(ms.fieldWarnings);
	const errs = (k: string) => E[k] ?? [];
	const unitOf = (ch: number) => ms.rows.find((r) => r.index === ch)?.units || 'g';

	function copyToAll(i: number) {
		if (locked) return;
		const from = $state.snapshot(d.controls[i]);
		d.controls.forEach((c, k) => {
			if (k === i) return;
			c.profile = structuredClone(from.profile);
			c.phase = structuredClone(from.phase);
		});
	}

	function addPhase(i: number) {
		const p = d.controls[i].phase;
		const last = p.at(-1);
		p.push({ freq_hz: (last?.freq_hz ?? 20) * 2, phase_deg: last?.phase_deg ?? 0 });
	}
</script>

<div class="stack">
	<section class="aq-panel" aria-labelledby="msine-refs-title" data-testid="msine-references">
		<div class="aq-head">
			<h2 id="msine-refs-title">References</h2>
			<span class="aq-sub">At full level, one per control. The schedule's levels scale every reference together.</span>
		</div>
		<label class="aq-switch" for="msine-phase-on">
			<input id="msine-phase-on" type="checkbox" role="switch" bind:checked={d.phase_specified} disabled={locked} data-testid="msine-phase-specified" />
			<span class="track" aria-hidden="true"><span class="thumb"></span></span>
			<span>Reference phases</span>
			<span class="word">{d.phase_specified ? 'Given per control' : 'Amplitudes only: the law chooses the phases'}</span>
		</label>
		<Findings items={errs('phase_specified').map((message) => ({ level: 'error' as const, message }))} />
		{#if !d.controls.length}
			<p class="aq-small aq-muted">Each control's reference appears here once the channel table has control rows.</p>
		{/if}
	</section>

	{#each d.controls as c, i (c.channel)}
		<section class="aq-panel" aria-labelledby="msine-ref-{c.channel}" data-testid="msine-reference-{c.channel}">
			<div class="aq-head">
				<h2 id="msine-ref-{c.channel}">Control {c.channel}</h2>
				<span class="aq-sub">{ms.rows.find((r) => r.index === c.channel)?.description ?? ''}</span>
				<span class="aq-spacer"></span>
				{#if d.controls.length > 1}<button class="btn sm ghost" disabled={locked} onclick={() => copyToAll(i)} data-testid="msine-copy-{c.channel}">Copy to Every Control</button>{/if}
			</div>
			<div class="aq-fields">
				<div class="tf">
					<label for="msine-ref-label-{i}" class="lbl">Name</label>
					<input id="msine-ref-label-{i}" class="field" bind:value={c.profile.label} disabled={locked} autocomplete="off" />
				</div>
				<NumberField id="msine-alarm-lo-{i}" label="Alarm below" unit="dB" step={0.5} min={0} bind:value={c.profile.alarm_db[0]} errors={errs(`controls.${i}.profile.alarm_db`)} disabled={locked} />
				<NumberField id="msine-alarm-hi-{i}" label="Alarm above" unit="dB" step={0.5} min={0} bind:value={c.profile.alarm_db[1]} disabled={locked} />
				<NumberField id="msine-abort-lo-{i}" label="Abort below" unit="dB" step={0.5} min={0} bind:value={c.profile.abort_db[0]} errors={errs(`controls.${i}.profile.abort_db`)} disabled={locked} />
				<NumberField id="msine-abort-hi-{i}" label="Abort above" unit="dB" step={0.5} min={0} bind:value={c.profile.abort_db[1]} disabled={locked} />
			</div>
			<SineProfileTable rows={c.profile.rows} prefix="controls.{i}.profile.rows" errors={E} warnings={W} {locked} testid="msine-profile-{c.channel}" label="Control {c.channel} amplitude" />
			{#if d.phase_specified}
				<div class="aq-group">
					<h3>Phase</h3>
					<p class="aq-lead aq-small">Degrees relative to the tone, above -180 and at most 180, joined linearly against the logarithm of frequency and held beyond the first and last rows. The control's response is measured in {unitOf(c.channel)}.</p>
					<table class="aq-table" data-testid="msine-phase-{c.channel}">
						<thead><tr><th class="num">Frequency</th><th class="num">Phase</th><th></th></tr></thead>
						<tbody>
							{#each c.phase as p, k (k)}
								<tr>
									<td><NumberField compact id="msine-ph-f-{i}-{k}" label="Row {k + 1} frequency" unit="Hz" bind:value={p.freq_hz} disabled={locked} /></td>
									<td><NumberField compact id="msine-ph-p-{i}-{k}" label="Row {k + 1} phase" unit="deg" step={5} min={-180} max={180} bind:value={p.phase_deg} disabled={locked} /></td>
									<td><button class="aq-icon-btn" aria-label="Remove row {k + 1}" disabled={locked || c.phase.length <= 1} onclick={() => c.phase.splice(k, 1)}>
										<svg viewBox="0 0 12 12" width="10" height="10" aria-hidden="true"><path d="M3 3l6 6M9 3l-6 6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
									</button></td>
								</tr>
							{/each}
						</tbody>
					</table>
					<button class="btn sm ghost" disabled={locked} onclick={() => addPhase(i)}>Add Row</button>
					<Findings items={errs(`controls.${i}.phase`).map((message) => ({ level: 'error' as const, message }))} testid="msine-phase-findings-{c.channel}" />
				</div>
			{/if}
		</section>
	{/each}
</div>

<style>
	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
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
