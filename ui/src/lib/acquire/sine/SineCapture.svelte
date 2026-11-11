<script lang="ts">
	/**
	 * What the run records beyond its report, the run folder's name under the local data folder,
	 * and the words kept with the run.
	 */
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import { CAPTURE_RULES, type SineDraft } from './sineSetup';

	/** What the card reads and edits: Sine's store, or another test type's with the same capture. */
	interface CaptureHost {
		setup: Pick<SineDraft, 'capture' | 'metadata'>;
		locked: boolean;
		fieldErrors: Record<string, string[]>;
	}

	interface Props {
		sine: CaptureHost;
	}
	let { sine }: Props = $props();

	const d = $derived(sine.setup);
	const cap = $derived(d.capture);
	const locked = $derived(sine.locked);
	const E = $derived(sine.fieldErrors);
	const errs = (k: string) => E[k] ?? [];
</script>

<section class="aq-panel narrow" aria-labelledby="nk-cap" data-testid="sine-capture">
	<div class="aq-head"><h2 id="nk-cap">Capture and Run Details</h2></div>
	<div class="aq-group">
		<div class="sel">
			<span class="lbl" id="nk-rule-l">Record</span>
			<div class="aq-seg" role="radiogroup" aria-labelledby="nk-rule-l">
				{#each CAPTURE_RULES as r (r.id)}
					<button type="button" role="radio" aria-checked={cap.rule === r.id} disabled={locked} onclick={() => (cap.rule = r.id)} data-testid="sine-capture-{r.id}">{r.label}</button>
				{/each}
			</div>
			<p class="help">{CAPTURE_RULES.find((r) => r.id === cap.rule)?.help}</p>
		</div>
		<div class="aq-fields spaced">
			{#if cap.rule === 'first_seconds'}
				<NumberField id="nk-first" label="Seconds at each level" unit="s" step={1} min={0} bind:value={cap.first_s} errors={errs('capture.first_s')} disabled={locked} testid="sine-capture-first" />
			{/if}
			<NumberField id="nk-settle" label="Settled at a level after" unit="s" step={0.5} min={0} bind:value={cap.level_settle_s} errors={errs('capture.level_settle_s')} disabled={locked} help="Usual: 1 s." />
			<NumberField id="nk-save" label="Save every" unit="s" nullable="Only at the end" fallback={60} step={10} min={0} bind:value={cap.periodic_save_s} errors={errs('capture.periodic_save_s')} disabled={locked} />
		</div>
		<div class="sel spaced">
			<label for="nk-name" class="lbl">Run folder</label>
			<input id="nk-name" class="field mono" bind:value={cap.run_name} disabled={locked} data-testid="sine-capture-name" />
			<p class="help">A plain name, under the program's local data folder.</p>
			<Findings items={errs('capture.run_name').map((message) => ({ level: 'error' as const, message }))} />
		</div>
	</div>
	<div class="aq-group">
		<h3>Kept With the Run</h3>
		<table class="aq-table" data-testid="sine-metadata">
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

	.mono {
		font-family: var(--font-mono);
	}

	.spaced {
		margin-top: var(--sp-3);
	}
</style>
