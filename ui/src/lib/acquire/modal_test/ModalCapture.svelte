<script lang="ts">
	/**
	 * What the test keeps and where: the shared Run Record panel (who runs the test, on what, to
	 * which document and clause, and notes, and whether the time histories are kept), the test's
	 * title, the run folder's name under the local data folder, and the words kept with the run. The
	 * service writes the run folder; what it holds is the guide's to say (docs/modal-test.md).
	 */
	import Findings from '../Findings.svelte';
	import RunRecordEditor from '../RunRecordEditor.svelte';
	import type { ModalTestStore } from './modalTestStore.svelte';

	interface Props {
		modal: ModalTestStore;
	}
	let { modal }: Props = $props();

	const d = $derived(modal.setup);
	const locked = $derived(modal.locked);
	const E = $derived(modal.fieldErrors);
	const errs = (k: string) => E[k] ?? [];
</script>

<div class="stack" data-testid="mt-record">
<RunRecordEditor acq={modal} kept="The setup with its check, the channel table with every DOF, the FRFs and the outcome are still kept." />
<section class="aq-panel narrow" aria-labelledby="mt-pc-cap" data-testid="mt-capture">
	<div class="aq-head"><h2 id="mt-pc-cap">Run Records</h2></div>
	<p class="aq-lead aq-small lead">The run folder holds the inputs as the station read them (in shaker mode the drives as played too), a line for every hit or frame taken and each change of its status, the FRFs and coherence of every point, the setup with its check, the channel table with every DOF, and how the test ended.</p>
	<div class="aq-group">
		<div class="sel">
			<label for="mt-pc-title" class="lbl">Title</label>
			<input id="mt-pc-title" class="field" bind:value={d.title} disabled={locked} data-testid="mt-title" />
		</div>
		<div class="sel spaced">
			<label for="mt-pc-name" class="lbl">Run folder</label>
			<input id="mt-pc-name" class="field mono" bind:value={d.capture.run_name} disabled={locked} data-testid="mt-capture-name" />
			<p class="help">A plain name, under the program's local data folder. A name already taken gets a number after it.</p>
			<Findings items={errs('capture.run_name').map((message) => ({ level: 'error' as const, message }))} />
		</div>
	</div>
	<div class="aq-group">
		<h3>Kept With the Run</h3>
		<table class="aq-table" data-testid="mt-metadata">
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
</div>

<style>
	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		max-width: 860px;
	}

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
