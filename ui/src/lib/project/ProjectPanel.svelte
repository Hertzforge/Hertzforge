<script lang="ts">
	/**
	 * The connection's own project: what is under way and the objects it holds. Files are the File
	 * menu's, or the top bar's where there is none (FileBar); an empty panel offers them too.
	 */
	import type { AppState } from '$lib/app/state.svelte';
	import Icon from '$lib/shell/Icon.svelte';
	import ProjectTree from './ProjectTree.svelte';

	interface Props {
		app: AppState;
	}
	let { app }: Props = $props();

	const p = $derived(app.project);
	/** An unresolved reference whose drop waits on a second click. */
	let confirming = $state<string | null>(null);
	const fraction = $derived(p.progress && p.progress.total > 0 ? p.progress.done / p.progress.total : null);
</script>

<section class="project" aria-labelledby="project-title" data-testid="project-panel">
	<div class="head">
		<h2 id="project-title" class="eyebrow">Project</h2>
		{#if p.summary}
			<span class="counts">{p.summary.objects.length} objects</span>
		{:else}
			<div class="actions">
				<button class="btn ghost sm" data-testid="panel-open" onclick={() => p.fileAct('open')} disabled={!app.analysis.isOpen || p.busy !== null}>Open</button>
				<button class="btn ghost sm" data-testid="panel-import" onclick={() => p.fileAct('import')} disabled={!app.analysis.isOpen || p.busy !== null}>Import</button>
			</div>
		{/if}
	</div>

	{#if p.busy}
		<div class="busy" role="status" data-testid="project-busy">
			<span>{p.busy}</span>
			{#if fraction !== null}
				<span class="bar" aria-hidden="true"><span style="width: {(fraction * 100).toFixed(1)}%"></span></span>
				<span class="num">{Math.round(fraction * 100)}%</span>
			{/if}
		</div>
	{/if}
	{#if p.error}<p class="error" role="alert" data-testid="project-error">{p.error}</p>{/if}

	{#if p.summary}
		<div class="choice on" data-testid="project-select">
			<span class="ctitle">
				{p.summary.title}
				{#if p.summary.modified}<span class="dot" title="Changes not saved" aria-label="Changes not saved"></span>{/if}
			</span>
			<span class="cdesc mono" title={p.summary.file ?? undefined}>{p.summary.file ?? 'Not saved yet'}</span>
		</div>
		{#if p.summary.objects.length || p.summary.unresolved?.length}
			{#if p.summary.objects.length}
				<ProjectTree {app} />
			{/if}
			{#if p.summary.unresolved?.length}
				<ul class="objects" aria-label="Unresolved references" data-testid="project-unresolved">
					{#each p.summary.unresolved as u (u.name)}
						<li class="unresolved" data-testid="unresolved-{u.name}" title="Held in {u.path}">
							{#if confirming === u.name}
								<span class="oname">Drop {u.name}?</span>
								<span class="row-actions shown">
									<button class="btn ghost sm danger" data-testid="remove-confirm" onclick={() => { confirming = null; void p.remove([u.name]); }}>Drop</button>
									<button class="btn ghost sm" onclick={() => (confirming = null)}>Keep</button>
								</span>
							{:else}
								<span class="oname">{u.name}</span>
								<span class="badge">{u.reason}</span>
								<span class="okind">{u.path.split(/[\\/]/).pop()}</span>
								<span class="row-actions">
									<button class="icon" data-testid="remove-{u.name}" title="Drop the reference to {u.name}" aria-label="Drop the reference to {u.name}" onclick={() => (confirming = u.name)} disabled={p.busy !== null}>
										<Icon name="trash" size={12} />
									</button>
								</span>
							{/if}
						</li>
					{/each}
				</ul>
			{/if}
			{#if p.summary.unresolved?.length}
				<p class="hint" data-testid="unresolved-hint">
					Held in files that were not found or have changed. Saving keeps them; put the file back, or beside the project, and open it again.
				</p>
			{/if}
		{:else}
			<p class="muted">Empty. Import a file, or drop one on the window in the desktop app.</p>
		{/if}
	{:else if !p.asking && !p.busy}
		<p class="muted">Open a project or import your own files. In the desktop app, drop them on the window.</p>
	{/if}
</section>

<style>
	.project {
		padding: var(--sp-4);
		border-bottom: 1px solid var(--line-1);
	}

	.head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		margin-bottom: var(--sp-2);
	}

	.actions {
		display: flex;
		gap: 2px;
	}




	.hint {
		font-size: var(--text-xs);
		color: var(--text-4);
	}

	.busy {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		margin-bottom: var(--sp-2);
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	.bar {
		flex: 1;
		height: 4px;
		background: var(--bg-3);
		border-radius: 2px;
		overflow: hidden;
	}

	.bar span {
		display: block;
		height: 100%;
		background: var(--accent);
	}

	.error {
		margin-bottom: var(--sp-2);
		padding: 6px 8px;
		font-size: var(--text-xs);
		color: var(--amber-2);
		background: var(--amber-weak);
		border-left: 2px solid var(--amber-2);
		border-radius: var(--radius-sm);
	}

	.muted {
		color: var(--text-3);
		font-size: var(--text-sm);
	}

	.choice {
		display: flex;
		flex-direction: column;
		gap: 2px;
		width: 100%;
		padding: 8px 10px;
		text-align: left;
		background: transparent;
		border: 1px solid transparent;
		border-radius: var(--radius-md);
	}

	.choice:hover {
		background: var(--bg-3);
	}

	.choice.on {
		background: var(--blue-weak);
		border-color: var(--blue-line);
	}

	.ctitle {
		display: flex;
		align-items: center;
		gap: 6px;
		font-size: var(--text-sm);
		font-weight: var(--weight-medium);
		color: var(--text-1);
	}

	.cdesc {
		overflow: hidden;
		font-size: var(--text-xs);
		color: var(--text-3);
		text-overflow: ellipsis;
		white-space: nowrap;
		direction: rtl;
		text-align: left;
	}

	.dot {
		width: 6px;
		height: 6px;
		background: var(--amber-2);
		border-radius: 50%;
	}






	.objects {
		margin-top: var(--sp-2);
		padding-left: 10px;
		list-style: none;
		font-size: var(--text-xs);
	}

	.objects li {
		display: flex;
		align-items: baseline;
		gap: 6px;
		padding: 2px 0;
	}

	.oname {
		color: var(--text-1);
	}

	.okind {
		margin-left: auto;
		color: var(--text-4);
		white-space: nowrap;
	}

	.unresolved .oname {
		color: var(--text-3);
	}

	.row-actions {
		display: flex;
		gap: 6px;
		align-items: center;
		opacity: 0;
	}

	.objects li:hover .row-actions,
	.objects li:focus-within .row-actions,
	.row-actions.shown {
		opacity: 1;
	}

	.row-actions .icon {
		padding: 0;
		font-size: 10px;
		color: var(--text-3);
		background: none;
		border: none;
		cursor: pointer;
	}

	.row-actions .icon:hover {
		color: var(--text-1);
	}

	.badge {
		cursor: pointer;
		background: none;
		padding: 0 4px;
		font-size: 10px;
		color: var(--amber-2);
		border: 1px solid var(--amber-line);
		border-radius: var(--radius-sm);
	}
	.counts {
		color: var(--text-3);
		font-size: var(--text-xs);
	}
</style>
