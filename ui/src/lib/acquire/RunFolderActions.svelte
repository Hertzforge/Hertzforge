<script lang="ts">
	/**
	 * A named run's folder as a file a person manages: Show Folder (in the desktop app, which can
	 * open the file manager), Copy Folder Path, and Delete Run where the run has ended, with the line
	 * that says how each went. The run files' state is the app's (`RunFiles`); the service deletes.
	 */
	import { deleteLine, type RunFiles } from '$lib/app/runFiles.svelte';

	interface Props {
		folder: string;
		files: RunFiles;
		/** Offer Copy Folder Path here (off where the line beside it already has it). */
		copy?: boolean;
		/** Offer Delete Run: the run has ended. */
		deletable?: boolean;
	}
	let { folder, files, copy = true, deletable = false }: Props = $props();

	const status = $derived(files.at(folder));
	const gone = $derived(status === 'done');
	const line = $derived(deleteLine(status, files.refusal));
	const shownError = $derived(files.shown?.folder === folder ? files.shown.error : null);
	const copied = $derived(files.copied?.folder === folder ? files.copied.ok : null);
</script>

{#if !gone}
	<span class="folderacts" data-testid="run-folder-actions">
		{#if files.canShow}
			<button class="btn sm ghost" data-testid="act-show-folder" title="Show the run folder in the file manager: {folder}" onclick={() => void files.showFolder(folder)}>Show Folder</button>
		{/if}
		{#if copy}
			<button class="btn sm ghost" data-testid="act-copy-folder" title={folder} onclick={() => void files.copyPath(folder)}>Copy Folder Path</button>
		{/if}
		{#if deletable}
			<button class="btn sm danger" data-testid="act-delete-run" aria-busy={status === 'working' || status === 'asking'} disabled={status === 'working' || status === 'asking'} title="Delete the run folder and its project file, after asking" onclick={() => void files.deleteRun(folder)}>Delete Run</button>
		{/if}
	</span>
{/if}
{#if line}
	<p class="folderline {status}" role={status === 'refused' ? 'alert' : 'status'} data-testid="delete-run-status" data-status={status}>
		{#if status === 'refused'}<strong>Not deleted.</strong>{/if}
		<span>{status === 'refused' ? (files.refusal ?? '') : line}</span>
	</p>
{/if}
{#if shownError}<p class="folderline refused" role="alert" data-testid="show-folder-status"><strong>Not shown.</strong> <span>{shownError}</span></p>{/if}
{#if copied !== null}<p class="folderline" role="status" data-testid="copy-folder-status">{copied ? "Copied the folder's path." : 'The clipboard cannot be written here. The path is shown above, to select and copy.'}</p>{/if}

<style>
	.folderacts {
		display: inline-flex;
		flex-wrap: wrap;
		gap: 6px;
	}

	.folderline {
		display: flex;
		flex-wrap: wrap;
		gap: 4px;
		margin-top: 4px;
		flex-basis: 100%;
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	.folderline.refused {
		color: var(--amber-1);
	}

	.folderline strong {
		font-weight: var(--weight-strong);
	}
</style>
