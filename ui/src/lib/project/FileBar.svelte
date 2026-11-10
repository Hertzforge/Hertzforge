<script lang="ts">
	/**
	 * New, Open, Import and Save in the top bar, where there is no File menu. In a browser they ask for a
	 * path here, since a browser never gives the shell a file's path (files.ts).
	 */
	import type { AppState } from '$lib/app/state.svelte';

	interface Props {
		app: AppState;
	}
	let { app }: Props = $props();

	const p = $derived(app.project);
	const PROMPTS = { open: 'Project file to open', import: 'File to import', save: 'Save the project as' } as const;
	const VERBS = { open: 'Open', import: 'Import', save: 'Save' } as const;
	let typed = $state('');

	function focused(node: HTMLInputElement) {
		node.focus();
	}

	async function submit(e: SubmitEvent) {
		e.preventDefault();
		const path = typed.trim();
		if (!path || !p.asking) return;
		const ok = p.asking === 'save' ? await p.save(path) : p.asking === 'open' ? await p.open(path) : await p.importFile(path);
		if (ok) {
			p.asking = null;
			typed = '';
		}
	}
</script>

<div class="files">
	<button class="btn ghost sm" data-testid="project-new" onclick={() => p.fileAct('new')} disabled={!app.analysis.isOpen || p.busy !== null}>New</button>
	<button class="btn ghost sm" data-testid="project-open" onclick={() => p.fileAct('open')} disabled={!app.analysis.isOpen || p.busy !== null}>Open</button>
	<button class="btn ghost sm" data-testid="project-import" onclick={() => p.fileAct('import')} disabled={!app.analysis.isOpen || p.busy !== null}>Import</button>
	{#if p.summary}<button class="btn ghost sm" data-testid="project-save" onclick={() => p.fileAct('save')} disabled={p.busy !== null}>Save</button>{/if}
	{#if p.asking}
		<form class="ask" onsubmit={submit} data-testid="project-path-form">
			<label for="project-path">{PROMPTS[p.asking]}</label>
			<div class="row">
				<input id="project-path" use:focused bind:value={typed} class="field mono" data-testid="project-path" placeholder={p.asking === 'save' ? '/path/to/project.hforge' : '/path/to/file'} spellcheck="false" autocomplete="off" onkeydown={(e) => e.key === 'Escape' && (p.asking = null)} />
				<button class="btn primary sm" type="submit" disabled={!typed.trim() || p.busy !== null}>{VERBS[p.asking]}</button>
			</div>
		</form>
	{/if}
</div>

<style>
	.files {
		position: relative;
		display: flex;
		gap: 2px;
	}
	.ask {
		position: absolute;
		top: calc(100% + 6px);
		right: 0;
		z-index: 20;
		width: 420px;
		padding: var(--sp-3);
		background: var(--bg-2);
		border: 1px solid var(--line-2);
		border-radius: var(--radius-md);
		box-shadow: var(--shadow-pop);
	}
	label {
		display: block;
		margin-bottom: var(--sp-1);
		color: var(--text-2);
		font-size: var(--text-xs);
	}
	.row {
		display: flex;
		gap: var(--sp-2);
	}
	.row .field {
		flex: 1;
	}
</style>
