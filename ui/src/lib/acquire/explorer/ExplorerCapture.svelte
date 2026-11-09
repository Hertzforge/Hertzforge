<script lang="ts">
	/**
	 * What the run keeps: the run folder's name under the local data folder, and the words kept with
	 * the run. The service writes the configuration, every steering verb it took, the survey line at
	 * the end and how the run ended; it records no samples.
	 */
	import Findings from '../Findings.svelte';
	import type { ExplorerStore } from './explorerStore.svelte';

	interface Props {
		explorer: ExplorerStore;
	}
	let { explorer }: Props = $props();

	const d = $derived(explorer.setup);
	const locked = $derived(explorer.locked);
	const E = $derived(explorer.fieldErrors);
	const errs = (k: string) => E[k] ?? [];
</script>

<section class="aq-panel narrow" aria-labelledby="xc-cap" data-testid="explorer-capture">
	<div class="aq-head"><h2 id="xc-cap">Run Records</h2></div>
	<p class="aq-lead aq-small lead">The run's folder holds its configuration, every steering verb the run took with the frame it took effect at, the survey line at the end and how the run ended. No samples are recorded.</p>
	<div class="aq-group">
		<div class="sel">
			<label for="xc-name" class="lbl">Run folder</label>
			<input id="xc-name" class="field mono" bind:value={d.capture.run_name} disabled={locked} data-testid="explorer-capture-name" />
			<p class="help">A plain name, under the program's local data folder.</p>
			<Findings items={errs('capture.run_name').map((message) => ({ level: 'error' as const, message }))} />
		</div>
	</div>
	<div class="aq-group">
		<h3>Kept With the Run</h3>
		<table class="aq-table" data-testid="explorer-metadata">
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
		<Findings items={errs('metadata').map((message) => ({ level: 'error' as const, message }))} />
	</div>
</section>

<style>
	.narrow {
		max-width: 860px;
	}

	.lead {
		margin: 0 0 var(--sp-2);
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

	.mono {
		font-family: var(--font-mono);
	}

	.spaced {
		margin-top: var(--sp-3);
	}
</style>
