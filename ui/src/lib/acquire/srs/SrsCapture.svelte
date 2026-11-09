<script lang="ts">
	/** The capture of the run and the words kept with it. */
	import Findings from '../Findings.svelte';
	import type { SrsStore } from './srsStore.svelte';

	interface Props {
		srs: SrsStore;
	}
	let { srs }: Props = $props();

	const d = $derived(srs.setup);
	const locked = $derived(srs.locked);
	const E = $derived(srs.fieldErrors);
	const errs = (k: string) => E[k] ?? [];
</script>

<section class="aq-panel narrow" aria-labelledby="rk-cap" data-testid="srs-capture">
	<div class="aq-head"><h2 id="rk-cap">Capture and Run Details</h2></div>
	<div class="aq-group">
		<label class="aq-switch" for="rk-auto">
			<input id="rk-auto" type="checkbox" role="switch" checked={d.capture.auto_start} disabled={locked} onchange={(e) => (d.capture.auto_start = e.currentTarget.checked)} data-testid="srs-capture-auto" />
			<span class="track" aria-hidden="true"><span class="thumb"></span></span>
			<span>Start capturing when the run arms</span>
			<span class="word">{d.capture.auto_start ? 'On' : 'Off'}</span>
		</label>
		<div class="sel spaced">
			<label for="rk-name" class="lbl">Capture name</label>
			<input id="rk-name" class="field mono" bind:value={d.capture.run_name} disabled={locked} data-testid="srs-capture-name" />
			<Findings items={errs('capture.run_name').map((message) => ({ level: 'error' as const, message }))} />
		</div>
	</div>
	<div class="aq-group">
		<h3>Kept With the Run</h3>
		<table class="aq-table" data-testid="srs-metadata">
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

<style>
	.narrow {
		max-width: 860px;
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

	.mono {
		font-family: var(--font-mono);
	}

	.spaced {
		margin-top: var(--sp-3);
	}
</style>
