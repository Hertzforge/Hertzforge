<script lang="ts">
	/**
	 * What the run's record says about the test: who runs it, on what, the document and clause it
	 * is run to, and notes, all optional and kept in the run folder as typed; and whether the
	 * folder keeps the time histories, with what an hour of them takes on disk. Part of the
	 * configuration the control service accepts; none of it changes how the run drives.
	 */
	import Findings from './Findings.svelte';
	import { recordBytesPerHour, sizeWords, type RecordHost } from './setup';

	interface Props {
		acq: RecordHost;
		/** What the record keeps beside the time histories, in words, for the line under the switch. */
		kept?: string;
	}
	let { acq, kept = 'The setup, the channel table, the log and the spectra are still kept.' }: Props = $props();

	const r = $derived(acq.setup.record);
	const locked = $derived(acq.locked);
	const E = $derived(acq.fieldErrors);
	const channels = $derived(acq.recordChannels);
	const bytes = $derived(recordBytesPerHour(channels, acq.recordRateHz));
	const docErrors = $derived((E['specification.document'] ?? []).map((message) => ({ level: 'error' as const, message })));
</script>

<section class="aq-panel record" aria-labelledby="record-title" data-testid="record-panel">
	<div class="aq-head">
		<h2 id="record-title">Run Record</h2>
		<span class="aq-sub">Optional. Kept with the run's data.</span>
	</div>

	<div class="aq-fields words">
		<div class="tf">
			<label for="rec-operator" class="lbl">Operator</label>
			<input id="rec-operator" class="field" bind:value={r.operator} disabled={locked} autocomplete="off" data-testid="rec-operator" />
		</div>
		<div class="tf">
			<label for="rec-article" class="lbl">Test article</label>
			<input id="rec-article" class="field" bind:value={r.article} disabled={locked} autocomplete="off" data-testid="rec-article" />
		</div>
		<div class="tf">
			<label for="rec-document" class="lbl">Specification document</label>
			<input id="rec-document" class="field" bind:value={r.document} disabled={locked} autocomplete="off" aria-invalid={docErrors.length > 0} data-testid="rec-document" />
			<Findings items={docErrors} />
		</div>
		<div class="tf">
			<label for="rec-clause" class="lbl">Clause</label>
			<input id="rec-clause" class="field" bind:value={r.clause} disabled={locked} autocomplete="off" data-testid="rec-clause" />
		</div>
		<div class="tf wide">
			<label for="rec-notes" class="lbl">Notes</label>
			<textarea id="rec-notes" class="field" rows="2" bind:value={r.notes} disabled={locked} data-testid="rec-notes"></textarea>
		</div>
	</div>

	<div class="aq-group streams">
		<h3>Recording</h3>
		<label class="aq-switch" for="rec-streams">
			<input id="rec-streams" type="checkbox" role="switch" bind:checked={r.streams} disabled={locked} data-testid="rec-streams" />
			<span class="track" aria-hidden="true"><span class="thumb"></span></span>
			<span>Record time histories</span>
			<span class="word">{r.streams ? 'On' : 'Off'}</span>
		</label>
		<p class="help" data-testid="rec-size">
			{#if r.streams}
				{#if bytes !== null}
					About <strong class="num">{sizeWords(bytes)}</strong> per hour: {channels} {channels === 1 ? 'channel' : 'channels'} (the enabled inputs and drives) at the sample rate, four bytes a sample.
				{:else}
					The size per hour shows once the sample rate is a number.
				{/if}
			{:else}
				Every enabled input and drive goes unrecorded. {kept}
			{/if}
		</p>
	</div>
</section>

<style>
	.words {
		grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
	}

	.tf {
		display: flex;
		flex-direction: column;
		gap: 4px;
		min-width: 0;
	}

	.tf.wide {
		grid-column: 1 / -1;
	}

	.lbl {
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}

	textarea {
		height: auto;
		padding: 6px 8px;
		resize: vertical;
	}

	.record .aq-group {
		margin-top: var(--sp-4);
		padding-top: var(--sp-4);
		border-top: 1px solid var(--line-1);
	}

	.help {
		margin-top: 4px;
		font-size: var(--text-2xs);
		color: var(--text-3);
		line-height: 1.4;
	}

	.help strong {
		color: var(--text-1);
	}
</style>
