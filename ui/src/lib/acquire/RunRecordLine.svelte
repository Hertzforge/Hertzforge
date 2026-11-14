<script lang="ts">
	/**
	 * Where a run is recorded, for the Run card of any test type whose runs leave a run folder: from
	 * arm on, "Recording to" (after the end, "Recorded in") with the folder's name, the full path
	 * under it in small type, selectable, and a line when the time histories are not kept.
	 */
	import type { RunRecordRef } from '$lib/protocol/types';
	import { folderName } from './words';

	interface Props {
		record: RunRecordRef;
		/** The run has ended. */
		ended: boolean;
	}
	let { record, ended }: Props = $props();
</script>

<div class="rec" data-testid="record-line" data-streams={record.streams}>
	<svg class="mk" viewBox="0 0 14 14" width="14" height="14" aria-hidden="true"><path d="M1.5 3.2h4l1.3 1.4h5.7v6.9h-11z" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round" /></svg>
	<div class="what">
		<p class="recname"><span class="k">{ended ? 'Recorded in' : 'Recording to'}</span><strong title={record.folder} data-testid="record-folder">{folderName(record.folder)}</strong></p>
		<p class="path" title={record.folder} data-testid="record-path">{record.folder}</p>
		{#if record.streams === 'off'}<p class="msg" data-testid="record-streams-off">Time histories are not kept for this run.</p>{/if}
	</div>
</div>

<style>
	/* Where the run is recorded: the folder's name, and its full path under it. */
	.rec {
		display: flex;
		align-items: flex-start;
		gap: var(--sp-2);
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	.rec .mk {
		flex: none;
		margin-top: 1px;
		color: var(--text-3);
	}

	.what {
		min-width: 0;
	}

	.recname {
		display: flex;
		flex-direction: column;
	}

	.recname .k {
		color: var(--text-3);
	}

	.recname strong {
		font-family: var(--font-mono);
		font-weight: var(--weight-medium);
		color: var(--text-1);
		word-break: break-all;
	}

	.path {
		font-family: var(--font-mono);
		font-size: var(--text-2xs);
		color: var(--text-3);
		word-break: break-all;
		user-select: text;
	}

	.msg {
		margin-top: 2px;
		color: var(--text-2);
	}
</style>
