<script lang="ts">
	/**
	 * Where runs are kept: the data folder the control service writes each run's folder under, where
	 * it comes from, and whether a run's record can be written there. In the desktop app it can be
	 * changed (from the services' next start; never while a run is live; the runs kept stay where
	 * they are) and shown in the file manager; in a browser it is read from the service and shown.
	 */
	import { getApp } from '$lib/app/state.svelte';
	import { dataFolderSource } from '$lib/app/runFiles.svelte';

	interface Props {
		/** A run is live on this screen: the folder is not changed then. */
		locked?: boolean;
	}
	let { locked = false }: Props = $props();

	const app = getApp();
	const files = app.runFiles;
	const view = $derived(files.dataFolder);
	const changeWhy = $derived(locked ? 'The data folder is not changed while a run is live.' : null);

	// read once the service that answers is there (the analysis service, or the app itself)
	$effect(() => {
		if (app.config.popout) return;
		if (files.canShow || app.analysis.isOpen) void files.loadDataFolder();
	});
</script>

{#if !app.config.popout}
	<div class="datafolder" data-testid="data-folder">
		<p class="k"><span class="eyebrow">Runs are kept in</span></p>
		{#if view}
			<p class="path" title={view.path} data-testid="data-folder-path">{view.path}</p>
			<p class="note">{dataFolderSource(view)}</p>
			{#if view.writable === false}
				<p class="warn" role="alert" data-testid="data-folder-unwritable"><strong>Cannot be written.</strong> {view.reason ?? ''} A run is refused at arm until it can.</p>
			{:else if view.exists === false}
				<p class="note">Made when the first run arms.</p>
			{/if}
			{#if view.next}
				<p class="note" data-testid="data-folder-next">From the next start of Hertzforge: <span class="mono">{view.next}</span>. The runs kept stay where they are.</p>
			{/if}
		{/if}
		{#if files.dataError}<p class="warn" role="alert" data-testid="data-folder-error"><strong>Not changed.</strong> {files.dataError}</p>{/if}
		{#if files.settable || (files.canShow && view?.exists)}
			<div class="acts">
				{#if files.canShow && view?.exists}<button class="btn ghost sm" data-testid="data-folder-show" onclick={() => view && void files.showFolder(view.path)}>Show Folder</button>{/if}
				{#if files.settable}
					<button class="btn ghost sm" data-testid="data-folder-change" disabled={!!changeWhy || files.dataBusy} title={changeWhy ?? 'Choose the folder runs are kept in, from the next start'} onclick={() => void files.chooseDataFolder()}>Change Folder</button>
					{#if view?.chosen}<button class="btn ghost sm" data-testid="data-folder-default" disabled={!!changeWhy || files.dataBusy} title={view.default ? `Back to ${view.default}` : 'Back to the default'} onclick={() => void files.setDataFolder(null)}>Use Default</button>{/if}
				{/if}
			</div>
		{/if}
	</div>
{/if}

<style>
	.datafolder {
		margin-top: var(--sp-2);
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	.path,
	.mono {
		font-family: var(--font-mono);
		word-break: break-all;
		user-select: text;
	}

	.path {
		color: var(--text-1);
	}

	.note {
		color: var(--text-3);
	}

	.warn {
		color: var(--amber-1);
	}

	.warn strong {
		font-weight: var(--weight-strong);
	}

	.acts {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
		margin-top: 4px;
	}
</style>
