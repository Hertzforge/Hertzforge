<script lang="ts">
	/**
	 * Sine's run record: the shared Run Record panel (who runs the test, on what, to which document
	 * and clause, notes, and whether the run folder keeps the time histories), then the fields kept
	 * with the run's report. Every run is recorded; none of this changes how the run drives.
	 */
	import RunRecordEditor from '../RunRecordEditor.svelte';
	import type { RecordHost } from '../setup';
	import MetadataFields from './MetadataFields.svelte';
	import type { SineDraft } from './sineSetup';

	interface Props {
		sine: RecordHost & { setup: Pick<SineDraft, 'record' | 'metadata'> };
	}
	let { sine }: Props = $props();
</script>

<div class="stack" data-testid="sine-record">
	<RunRecordEditor acq={sine} kept="The setup, the channel table, the log, the levels, the identification and the control trace are still kept." />
	<section class="aq-panel" aria-label="Fields kept with the run">
		<MetadataFields metadata={sine.setup.metadata} locked={sine.locked} />
	</section>
</div>

<style>
	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		max-width: 860px;
	}
</style>
