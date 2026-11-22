<script lang="ts">
	/**
	 * The run's own words: who runs it, on what, the document and clause it is run to, notes, and
	 * whether the time histories are recorded. All optional, all part of the configuration the
	 * control service takes; none of it changes how the run drives.
	 */
	import Findings from '../Findings.svelte';
	import type { MimoSineStore } from './msineStore.svelte';

	interface Props {
		ms: MimoSineStore;
	}
	let { ms }: Props = $props();

	const r = $derived(ms.setup.record);
	const locked = $derived(ms.locked);
	const docErrors = $derived((ms.fieldErrors['specification.document'] ?? []).map((message) => ({ level: 'error' as const, message })));
</script>

<section class="aq-panel" aria-labelledby="msine-rec-title" data-testid="msine-record">
	<div class="aq-head">
		<h2 id="msine-rec-title">Run Record</h2>
		<span class="aq-sub">Optional. Sent with the setup.</span>
	</div>
	<div class="aq-fields">
		<div class="tf">
			<label for="msine-title" class="lbl">Title</label>
			<input id="msine-title" class="field" bind:value={ms.setup.title} placeholder="From the references and the test" disabled={locked} autocomplete="off" data-testid="msine-title" />
		</div>
		<div class="tf">
			<label for="msine-operator" class="lbl">Operator</label>
			<input id="msine-operator" class="field" bind:value={r.operator} disabled={locked} autocomplete="off" />
		</div>
		<div class="tf">
			<label for="msine-article" class="lbl">Test article</label>
			<input id="msine-article" class="field" bind:value={r.article} disabled={locked} autocomplete="off" />
		</div>
		<div class="tf">
			<label for="msine-document" class="lbl">Specification document</label>
			<input id="msine-document" class="field" bind:value={r.document} disabled={locked} autocomplete="off" aria-invalid={docErrors.length > 0} />
			<Findings items={docErrors} />
		</div>
		<div class="tf">
			<label for="msine-clause" class="lbl">Clause</label>
			<input id="msine-clause" class="field" bind:value={r.clause} disabled={locked} autocomplete="off" />
		</div>
		<div class="tf wide">
			<label for="msine-notes" class="lbl">Notes</label>
			<textarea id="msine-notes" class="field" rows="2" bind:value={r.notes} disabled={locked}></textarea>
		</div>
	</div>
	<label class="aq-switch" for="msine-streams">
		<input id="msine-streams" type="checkbox" role="switch" bind:checked={r.streams} disabled={locked} data-testid="msine-streams" />
		<span class="track" aria-hidden="true"><span class="thumb"></span></span>
		<span>Record time histories</span>
		<span class="word">{r.streams ? 'On' : 'Off'}</span>
	</label>
</section>

<style>
	.tf {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.wide {
		grid-column: 1 / -1;
	}

	.lbl {
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}
</style>
