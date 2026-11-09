<script lang="ts">
	/**
	 * Where the run's records go and the words kept with them: whether capture starts with the run,
	 * the run folder's name under the local data folder, and the fields kept with the run.
	 */
	import Findings from '../Findings.svelte';
	import type { TwrStore } from './twrStore.svelte';

	interface Props {
		twr: TwrStore;
	}
	let { twr }: Props = $props();

	const d = $derived(twr.setup);
	const cap = $derived(d.capture);
	const locked = $derived(twr.locked);
	const E = $derived(twr.fieldErrors);
	const errs = (k: string) => E[k] ?? [];
</script>

<section class="aq-panel narrow" aria-labelledby="tk-cap" data-testid="twr-capture">
	<div class="aq-head"><h2 id="tk-cap">Capture and Run Details</h2></div>
	<div class="aq-group">
		<label class="aq-switch" for="tk-auto">
			<input id="tk-auto" type="checkbox" role="switch" checked={cap.auto_start} disabled={locked} onchange={(e) => (cap.auto_start = e.currentTarget.checked)} data-testid="twr-capture-auto" />
			<span class="track" aria-hidden="true"><span class="thumb"></span></span>
			<span>Capture from the start of the run</span>
			<span class="word">{cap.auto_start ? 'On' : 'Off'}</span>
		</label>
		<p class="help">Each play's target, response and drive go to the run folder either way.</p>
		<div class="sel spaced">
			<label for="tk-name" class="lbl">Run folder</label>
			<input id="tk-name" class="field mono" bind:value={cap.run_name} disabled={locked} data-testid="twr-capture-name" />
			<p class="help">A plain name, under the program's local data folder.</p>
			<Findings items={errs('capture.run_name').map((message) => ({ level: 'error' as const, message }))} />
		</div>
	</div>
	<div class="aq-group">
		<h3>Kept With the Run</h3>
		<table class="aq-table" data-testid="twr-metadata">
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

	.help {
		margin: 2px 0 0;
		font-size: var(--text-xs);
		color: var(--text-3);
	}

	.spaced {
		margin-top: var(--sp-3);
	}
</style>
