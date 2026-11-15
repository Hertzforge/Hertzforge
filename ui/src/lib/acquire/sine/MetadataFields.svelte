<script lang="ts">
	/**
	 * The fields kept with a run (a run number, a serial number), each a name and a value, added and
	 * removed by the operator. Sine's run record and Mixed Mode's capture both show it.
	 */
	interface Props {
		metadata: { key: string; label: string; value: string }[];
		locked: boolean;
	}
	let { metadata, locked }: Props = $props();
</script>

<div class="aq-group">
	<h3>Kept With the Run</h3>
	<table class="aq-table" data-testid="run-metadata">
		<thead><tr><th>Field</th><th>Value</th><th aria-label="Remove"></th></tr></thead>
		<tbody>
			{#each metadata as m, i (i)}
				<tr>
					<td><input class="field" aria-label="Field {i + 1} name" bind:value={m.label} disabled={locked} oninput={() => (m.key = m.label.trim().toLowerCase().replace(/[^a-z0-9]+/g, '_').replace(/^_|_$/g, '') || `field_${i + 1}`)} /></td>
					<td><input class="field" aria-label="{m.label || `Field ${i + 1}`} value" bind:value={m.value} disabled={locked} /></td>
					<td>
						<button class="aq-icon-btn" aria-label="Remove field {i + 1}" disabled={locked} onclick={() => metadata.splice(i, 1)}>
							<svg viewBox="0 0 12 12" width="10" height="10" aria-hidden="true"><path d="M3 3l6 6M9 3l-6 6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
						</button>
					</td>
				</tr>
			{/each}
		</tbody>
	</table>
	<button class="btn sm ghost spaced" disabled={locked} onclick={() => metadata.push({ key: `field_${metadata.length + 1}`, label: '', value: '' })}>Add Field</button>
</div>

<style>
	.spaced {
		margin-top: var(--sp-3);
	}
</style>
