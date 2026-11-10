<script lang="ts">
	/**
	 * What the run keeps and where: the run's name, the run folder's name under the local data
	 * folder, the free space the run leaves on the data volume, and the words kept with the run. The
	 * service writes each recording's samples and facts into the run folder, and the run's
	 * configuration, log, marks and end beside them.
	 */
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import type { RecorderStore } from './recorderStore.svelte';

	interface Props {
		recorder: RecorderStore;
	}
	let { recorder }: Props = $props();

	const d = $derived(recorder.setup);
	const locked = $derived(recorder.locked);
	const E = $derived(recorder.fieldErrors);
	const errs = (k: string) => E[k] ?? [];
</script>

<section class="aq-panel narrow" aria-labelledby="rc-cap" data-testid="recorder-capture">
	<div class="aq-head"><h2 id="rc-cap">Run Records</h2></div>
	<p class="aq-lead aq-small lead">Each recording is a folder of its own in the run folder: its samples (every recorded row, at the rate the station ran at) and its facts. Beside them the run keeps its configuration, its log, its marks and how it ended. Monitoring saves nothing.</p>
	<div class="aq-group">
		<div class="sel">
			<label for="rc-title" class="lbl">Title</label>
			<input id="rc-title" class="field" bind:value={d.title} disabled={locked} data-testid="recorder-title" />
		</div>
		<div class="sel spaced">
			<label for="rc-name" class="lbl">Run folder</label>
			<input id="rc-name" class="field mono" bind:value={d.capture.run_name} disabled={locked} data-testid="recorder-capture-name" />
			<p class="help">A plain name, under the program's local data folder. A name already taken gets a number after it.</p>
			<Findings items={errs('capture.run_name').map((message) => ({ level: 'error' as const, message }))} />
		</div>
		<div class="aq-fields spaced">
			<NumberField id="rc-floor" label="Keep free on the data volume" unit="GiB" step={1} min={1} bind:value={d.capture.keep_free_gb} errors={errs('capture.keep_free_gb')} disabled={locked} testid="recorder-keep-free" help="At least 1. With a timed length, the floor is also at least twice one recording. A run that would cross it is ended and says so." />
		</div>
	</div>
	<div class="aq-group">
		<h3>Kept With the Run</h3>
		<table class="aq-table" data-testid="recorder-metadata">
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
