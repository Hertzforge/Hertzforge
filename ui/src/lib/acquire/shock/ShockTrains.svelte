<script lang="ts">
	/**
	 * Pulses fired in a row on the manual ladder: a train (a count of pulses, at least the time
	 * between pulses apart) and a gunfire burst (rounds at a rate); each fires the drive in force
	 * with no law call inside it, and every pulse is judged. Then the capture of the run and the
	 * words kept with it.
	 */
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import type { ShockStore } from './shockStore.svelte';

	interface Props {
		shock: ShockStore;
	}
	let { shock }: Props = $props();

	const d = $derived(shock.setup);
	const locked = $derived(shock.locked);
	const E = $derived(shock.fieldErrors);
	const errs = (k: string) => E[k] ?? [];
	const manual = $derived(d.schedule.mode === 'manual');
</script>

<div class="cols">
	<section class="aq-panel" aria-labelledby="st-train">
		<div class="aq-head"><h2 id="st-train">Trains and Gunfire</h2><span class="aq-sub">Fired from the run, on the manual ladder.</span></div>
		{#if !manual}
			<p class="aq-notice aq-small">The schedule is automatic: trains and bursts are fired only on the manual ladder (Schedule).</p>
		{/if}
		<div class="aq-group">
			<h3>Train</h3>
			<p class="aq-lead">Pulses one after another at the present level, at least the time between pulses apart. The drive in force fires each; the law does not change it inside the train.</p>
			<div class="aq-fields">
				<NumberField id="st-n" label="Pulses" integer step={1} min={1} bind:value={d.train.count} errors={errs('train.count')} disabled={locked} testid="shock-train-count" />
				<NumberField id="st-s" label="Apart" unit="s" step={0.5} min={0} bind:value={d.train.spacing_s} errors={errs('train.spacing_s')} disabled={locked} testid="shock-train-spacing" help="At least the time between pulses." />
			</div>
		</div>
		<div class="aq-group">
			<h3>Gunfire Burst</h3>
			<label class="aq-switch" for="st-g">
				<input id="st-g" type="checkbox" role="switch" checked={d.gunfire.enabled} disabled={locked} onchange={(e) => (d.gunfire.enabled = e.currentTarget.checked)} data-testid="shock-gunfire-on" />
				<span class="track" aria-hidden="true"><span class="thumb"></span></span>
				<span>Offer a gunfire burst</span>
				<span class="word">{d.gunfire.enabled ? 'On' : 'Off'}</span>
			</label>
			{#if d.gunfire.enabled}
				<div class="aq-fields spaced">
					<NumberField id="st-rpm" label="Rate" unit="per min" step={50} min={1} bind:value={d.gunfire.rounds_per_min} errors={errs('gunfire.rounds_per_min')} disabled={locked} testid="shock-gunfire-rpm" />
					<NumberField id="st-r" label="Rounds" integer step={1} min={1} bind:value={d.gunfire.rounds} errors={errs('gunfire.rounds')} disabled={locked} testid="shock-gunfire-rounds" />
				</div>
				{#if Number.isFinite(d.gunfire.rounds_per_min) && d.gunfire.rounds_per_min > 0}
					<p class="aq-small aq-muted">Rounds {Math.round((60 / d.gunfire.rounds_per_min) * 1000)} ms apart, which must be at least the time between pulses.</p>
				{/if}
			{/if}
		</div>
	</section>

	<section class="aq-panel" aria-labelledby="st-cap">
		<div class="aq-head"><h2 id="st-cap">Capture and Run Details</h2></div>
		<div class="aq-group">
			<label class="aq-switch" for="st-auto">
				<input id="st-auto" type="checkbox" role="switch" checked={d.capture.auto_start} disabled={locked} onchange={(e) => (d.capture.auto_start = e.currentTarget.checked)} data-testid="shock-capture-auto" />
				<span class="track" aria-hidden="true"><span class="thumb"></span></span>
				<span>Start capturing when the run arms</span>
				<span class="word">{d.capture.auto_start ? 'On' : 'Off'}</span>
			</label>
			<div class="sel spaced">
				<label for="st-name" class="lbl">Capture name</label>
				<input id="st-name" class="field mono" bind:value={d.capture.run_name} disabled={locked} data-testid="shock-capture-name" />
				<Findings items={errs('capture.run_name').map((message) => ({ level: 'error' as const, message }))} />
			</div>
		</div>
		<div class="aq-group">
			<h3>Kept With the Run</h3>
			<table class="aq-table" data-testid="shock-metadata">
				<thead><tr><th>Field</th><th>Value</th><th aria-label="Remove"></th></tr></thead>
				<tbody>
					{#each d.metadata as m, i (i)}
						<tr>
							<td><input class="field" aria-label="Field {i + 1} name" bind:value={m.label} disabled={locked} oninput={() => (m.key = m.label.trim().toLowerCase().replace(/[^a-z0-9]+/g, '_').replace(/^_|_$/g, '') || `field_${i + 1}`)} /></td>
							<td><input class="field" aria-label="{m.label || `Field ${i + 1}`} value" bind:value={m.value} disabled={locked} /></td>
							<td>
								<button class="aq-icon-btn" aria-label="Remove field {i + 1}" disabled={locked} onclick={() => d.metadata.splice(i, 1)}>
									<svg viewBox="0 0 12 12" width="10" height="10" aria-hidden="true"><path d="M3 3l6 6M9 3l-6 6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
								</button>
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
			<button class="btn sm ghost spaced" disabled={locked} onclick={() => d.metadata.push({ key: `field_${d.metadata.length + 1}`, label: '', value: '' })}>Add Field</button>
		</div>
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
		max-width: 360px;
	}

	.lbl {
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}

	.mono {
		font-family: var(--font-mono);
	}

	.spaced {
		margin-top: var(--sp-3);
	}

	@media (max-width: 1240px) {
		.cols {
			grid-template-columns: minmax(0, 1fr);
		}
	}
</style>
