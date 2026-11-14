<script lang="ts">
	/**
	 * What a Mixed Mode run records beyond its report, the run folder's name under the local data
	 * folder, and the words kept with the run.
	 */
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import MetadataFields from '../sine/MetadataFields.svelte';
	import { CAPTURE_RULES, type MixedDraft } from './mixedSetup';

	/** What the card reads and edits: Mixed Mode's store. */
	interface CaptureHost {
		setup: Pick<MixedDraft, 'capture' | 'metadata'>;
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

<section class="aq-panel narrow" aria-labelledby="nk-cap" data-testid="mixed-capture">
	<div class="aq-head"><h2 id="nk-cap">Capture and Run Details</h2></div>
	<div class="aq-group">
		<div class="sel">
			<span class="lbl" id="nk-rule-l">Record</span>
			<div class="aq-seg" role="radiogroup" aria-labelledby="nk-rule-l">
				{#each CAPTURE_RULES as r (r.id)}
					<button type="button" role="radio" aria-checked={cap.rule === r.id} disabled={locked} onclick={() => (cap.rule = r.id)} data-testid="mixed-capture-{r.id}">{r.label}</button>
				{/each}
			</div>
			<p class="help">{CAPTURE_RULES.find((r) => r.id === cap.rule)?.help}</p>
		</div>
		<div class="aq-fields spaced">
			{#if cap.rule === 'first_seconds'}
				<NumberField id="nk-first" label="Seconds at each level" unit="s" step={1} min={0} bind:value={cap.first_s} errors={errs('capture.first_s')} disabled={locked} testid="mixed-capture-first" />
			{/if}
			<NumberField id="nk-settle" label="Settled at a level after" unit="s" step={0.5} min={0} bind:value={cap.level_settle_s} errors={errs('capture.level_settle_s')} disabled={locked} help="Usual: 1 s." />
			<NumberField id="nk-save" label="Save every" unit="s" nullable="Only at the end" fallback={60} step={10} min={0} bind:value={cap.periodic_save_s} errors={errs('capture.periodic_save_s')} disabled={locked} />
		</div>
		<div class="sel spaced">
			<label for="nk-name" class="lbl">Run folder</label>
			<input id="nk-name" class="field mono" bind:value={cap.run_name} disabled={locked} data-testid="mixed-capture-name" />
			<p class="help">A plain name, under the program's local data folder.</p>
			<Findings items={errs('capture.run_name').map((message) => ({ level: 'error' as const, message }))} />
		</div>
	</div>
	<MetadataFields metadata={d.metadata} {locked} />
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
