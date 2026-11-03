<script lang="ts">
	/**
	 * The connection's own project: Open, Import and Save, what is under way, and the objects it
	 * holds. In the desktop app the buttons open the native dialogs; in a browser they ask for a
	 * path, since a browser never gives the shell a file's path (files.ts).
	 */
	import type { AppState } from '$lib/app/state.svelte';
	import { formatCount } from '$lib/viz/format';
	import { chooseFiles, chooseSavePath, hasNativeFiles } from './files';

	interface Props {
		app: AppState;
	}
	let { app }: Props = $props();

	const p = $derived(app.project);
	/** The path field a browser shows in place of a dialog, and what it is for. */
	let asking = $state<'open' | 'import' | 'save' | null>(null);
	let typed = $state('');
	let field = $state<HTMLInputElement | null>(null);

	const PROMPTS = { open: 'Project file to open', import: 'File to import', save: 'Save the project as' } as const;

	async function act(what: 'open' | 'import' | 'save') {
		if (what === 'save' && p.summary?.file && !p.summary.file.endsWith('.vdyn')) {
			await p.save();
			return;
		}
		if (hasNativeFiles()) {
			if (what === 'save') {
				const path = await chooseSavePath(p.summary?.title ?? 'Project');
				if (path) await p.save(path);
				return;
			}
			const paths = await chooseFiles(what);
			if (paths) await p.openPaths(paths);
			return;
		}
		asking = asking === what ? null : what;
		typed = '';
		queueMicrotask(() => field?.focus());
	}

	async function submit(e: SubmitEvent) {
		e.preventDefault();
		const path = typed.trim();
		if (!path || !asking) return;
		const ok = asking === 'save' ? await p.save(path) : asking === 'open' ? await p.open(path) : await p.importFile(path);
		if (ok) asking = null;
	}

	const fraction = $derived(p.progress && p.progress.total > 0 ? p.progress.done / p.progress.total : null);
</script>

<section class="project" aria-labelledby="project-title" data-testid="project-panel">
	<div class="head">
		<h2 id="project-title" class="eyebrow">Project</h2>
		<div class="actions">
			<button class="btn ghost sm" data-testid="project-open" onclick={() => act('open')} disabled={!app.analysis.isOpen || p.busy !== null}>Open</button>
			<button class="btn ghost sm" data-testid="project-import" onclick={() => act('import')} disabled={!app.analysis.isOpen || p.busy !== null}>Import</button>
			{#if p.summary}
				<button class="btn ghost sm" data-testid="project-save" onclick={() => act('save')} disabled={p.busy !== null}>Save</button>
			{/if}
		</div>
	</div>

	{#if asking}
		<form class="ask" onsubmit={submit} data-testid="project-path-form">
			<label for="project-path">{PROMPTS[asking]}</label>
			<div class="row">
				<input
					id="project-path"
					bind:this={field}
					bind:value={typed}
					class="field mono"
					data-testid="project-path"
					placeholder={asking === 'save' ? '/path/to/project.hforge' : '/path/to/file'}
					spellcheck="false"
					autocomplete="off"
					onkeydown={(e) => e.key === 'Escape' && (asking = null)}
				/>
				<button class="btn primary sm" type="submit" disabled={!typed.trim() || p.busy !== null}>{asking === 'save' ? 'Save' : asking === 'open' ? 'Open' : 'Import'}</button>
			</div>
			<p class="hint">In the desktop app this is the file dialog, and files can be dropped on the window.</p>
		</form>
	{/if}

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
		<button class="choice" class:on={app.analyze.onProject} aria-pressed={app.analyze.onProject} data-testid="project-select" onclick={() => app.analyze.selectProject()}>
			<span class="ctitle">
				{p.summary.title}
				{#if p.summary.modified}<span class="dot" title="Changes not saved" aria-label="Changes not saved"></span>{/if}
			</span>
			<span class="cdesc mono" title={p.summary.file ?? undefined}>{p.summary.file ?? 'Not saved yet'}</span>
		</button>
		{#if p.summary.objects.length}
			<ul class="objects" aria-label="Objects" data-testid="project-objects">
				{#each p.summary.objects as o (o.name)}
					<li>
						<span class="oname">{o.name}</span>
						{#if o.units_defined === false}<span class="badge" title="Imported without its units">units?</span>{/if}
						<span class="okind">{o.kind.replace(/_/g, ' ')}{o.records !== undefined ? `, ${formatCount(o.records)}` : ''}</span>
					</li>
				{/each}
			</ul>
		{:else}
			<p class="muted">Empty. Import a file, or drop one on the window in the desktop app.</p>
		{/if}
	{:else if !asking && !p.busy}
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

	.ask {
		display: flex;
		flex-direction: column;
		gap: 4px;
		margin-bottom: var(--sp-2);
	}

	.ask label {
		font-size: var(--text-xs);
		color: var(--text-3);
	}

	.row {
		display: flex;
		gap: 4px;
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

	.badge {
		padding: 0 4px;
		font-size: 10px;
		color: var(--amber-2);
		border: 1px solid var(--amber-line);
		border-radius: var(--radius-sm);
	}
</style>
