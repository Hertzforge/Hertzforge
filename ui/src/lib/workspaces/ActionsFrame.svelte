<script lang="ts">
	/**
	 * The Actions frame every module shares: what the selection can take, its readings and acts
	 * (Visual Dynamics' bar offers only the acts a selection allows) filtered to the module `only`
	 * and the acts offered everywhere, the acts that take no object when nothing is selected, and
	 * the panel of the chosen operation with its parameters and Run. The Report workspace shows it
	 * for the report; the study host for the tree's selection.
	 */
	import type { AppState } from '$lib/app/state.svelte';
	import { offeredIn } from '$lib/app/workspaces';
	import type { Workspace } from '$lib/protocol/types';
	import ParamForm from '$lib/forms/ParamForm.svelte';
	import Icon from '$lib/shell/Icon.svelte';

	interface Props {
		app: AppState;
		/** The module whose operations are offered, with those offered everywhere. */
		only: Workspace;
	}
	let { app, only }: Props = $props();

	const a = $derived(app.analyze);
	const tools = $derived(a.toolsFor(only));
	const inModule = (e: { workspace: Workspace; everywhere?: boolean }) => offeredIn(e, only);

	// the project's FRFs a fit can start on, and the coherences that can weight it
	const objects = $derived(app.project.summary?.objects ?? []);
	const coherences = $derived(objects.filter((o) => o.kind === 'coherence' || o.kind === 'multiple_coherence').map((o) => o.name));
	let weight = $state('');
	$effect(() => {
		if (weight && !coherences.includes(weight)) weight = '';
	});

	const isMac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
</script>

{#if a.onProject}
		{@const picked = app.project.selection.objects}
		{@const kinds = picked.map((n) => app.project.summary?.objects.find((o) => o.name === n)?.kind)}
		{@const fitPair = picked.length === 2 && kinds.includes('frf') && kinds.includes('shape_set')}
		{@const fitAlone = only === 'modal' && picked.length === 1 && kinds[0] === 'frf'}
		<section class="block" data-testid="actions-frame">
			<h2 class="eyebrow">
				{#if picked.length}Actions <span class="for">for {picked.length === 1 ? picked[0] : `${picked.length} objects`}</span>{:else}Actions <span class="for">for the project</span>{/if}
			</h2>
			{#if !a.actions}
				<p class="muted small">Asking what the selection can take.</p>
			{:else}
				{@const readings = a.actions.readings.filter(inModule)}
				{@const acts = a.actions.acts.filter(inModule)}
				{@const whole = a.actions.project.filter(inModule)}
				{#if fitPair || fitAlone}
					<div class="tools" role="group" aria-label="Fit" data-testid="fit-start">
						{#if fitPair}
							<button class="tool" data-testid="action-edit-fit" onclick={() => { app.go('modal'); void app.fit.start(picked[kinds.indexOf('frf')], picked[kinds.indexOf('shape_set')], weight || undefined); }}>
								<Icon name="tool" size={15} />
								<span class="ttitle">Edit Fit</span>
								<span class="tout">modal</span>
							</button>
						{:else}
							<button class="tool" data-testid="fit-begin" onclick={() => { void app.fit.start(picked[0], undefined, weight || undefined); }} disabled={app.fit.busy !== null}>
								<Icon name="tool" size={15} />
								<span class="ttitle">Fit Modes</span>
								<span class="tout">by hand</span>
							</button>
						{/if}
						{#if coherences.length}
							<select class="field sm" aria-label="Weight the fit by" data-testid="fit-weight" bind:value={weight} title="Weight each response at each line by its coherence, γ²/(1-γ²)">
								<option value="">No weighting</option>
								{#each coherences as name (name)}<option value={name}>Weight by {name}</option>{/each}
							</select>
						{/if}
						{#if app.fit.error && !app.fit.state}<p class="error" role="alert">{app.fit.error}</p>{/if}
					</div>
				{/if}
				{#if readings.length}
					<h3 class="group">Readings</h3>
					<div class="tools" role="radiogroup" aria-label="Readings" data-testid="action-readings">
						{#each readings as e (e.id)}
							<button class="tool" data-testid="action-{e.id}" role="radio" aria-checked={a.opId === e.id} class:on={a.opId === e.id} onclick={() => a.pickAction(e)}>
								<Icon name="tool" size={15} />
								<span class="ttitle">{e.title}</span>
							</button>
						{/each}
					</div>
				{/if}
				{#if acts.length}
					<h3 class="group">Acts</h3>
					<div class="tools" role="radiogroup" aria-label="Acts" data-testid="action-acts">
						{#each acts as e (e.id)}
							<button class="tool" data-testid="action-{e.id}" role="radio" aria-checked={a.opId === e.id} class:on={a.opId === e.id} onclick={() => a.pickAction(e)}>
								<Icon name="action" size={15} />
								<span class="ttitle">{e.title}</span>
							</button>
						{/each}
					</div>
				{/if}
				{#if whole.length}
					<div class="tools" role="radiogroup" aria-label="Project acts" data-testid="action-project">
						{#each whole as e (e.id)}
							<button class="tool" data-testid="action-{e.id}" role="radio" aria-checked={a.opId === e.id} class:on={a.opId === e.id} onclick={() => a.pickAction(e)}>
								<Icon name="action" size={15} />
								<span class="ttitle">{e.title}</span>
							</button>
						{/each}
					</div>
				{/if}
				{#if !readings.length && !acts.length && !whole.length}
					<p class="muted small">{picked.length ? 'Nothing takes this selection together. Select less, or something else.' : 'Select an object in the tree to see what it can do.'}</p>
				{/if}
			{/if}
		</section>
{/if}

	{#if a.op && a.onProject}
		<section class="block panel" aria-labelledby="op-title">
			<div class="phead">
				<h2 id="op-title">{a.op.title}</h2>
				<button class="btn ghost sm" onclick={() => a.resetParams()} title="Reset every field to its default">Defaults</button>
			</div>
			{#if a.op.summary}<p class="psum">{a.op.summary}</p>{/if}
			<ParamForm
				params={a.params.filter((p) => !p.in_view)}
				bind:values={a.values[a.op.id]}
				showAllErrors={a.triedRun}
				idPrefix={a.op.id.replace('.', '-')}
			/>
			<div class="runrow">
				<button class="btn primary run" data-testid="run-op" onclick={() => a.run()} disabled={a.running || !app.analysis.isOpen}>
					{#if a.running}
						<svg class="spin" viewBox="0 0 12 12" width="12" height="12" aria-hidden="true"><path d="M6 1.5a4.5 4.5 0 104.5 4.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" /></svg>
						Running
					{:else}
						<Icon name="play" size={13} /> Run
					{/if}
				</button>
				<span class="keys"><kbd>{isMac ? 'Cmd' : 'Ctrl'}</kbd><kbd>Enter</kbd></span>
			</div>
			{#if a.runError}<p class="error" role="alert">{a.runError}</p>{/if}
		</section>
	{/if}

<style>
	.block {
		padding: var(--sp-4);
		border-bottom: 1px solid var(--line-1);
	}
	.block h2.eyebrow {
		margin-bottom: var(--sp-2);
	}
	.for {
		margin-left: 4px;
		font-weight: var(--weight-regular);
		text-transform: none;
		letter-spacing: 0;
		color: var(--text-4);
	}
	.muted {
		color: var(--text-3);
		font-size: var(--text-sm);
	}
	.small {
		margin-top: var(--sp-2);
		font-size: var(--text-xs);
	}
	.error {
		margin-top: var(--sp-2);
		padding: 6px 8px;
		font-size: var(--text-xs);
		color: var(--amber-2);
		background: var(--amber-weak);
		border-left: 2px solid var(--amber-2);
		border-radius: var(--radius-sm);
	}
	.group {
		margin: var(--sp-2) 0 2px;
		font-size: var(--text-2xs);
		font-weight: var(--weight-medium);
		letter-spacing: 0.04em;
		text-transform: uppercase;
		color: var(--text-4);
	}
	.tools {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}
	.tool {
		display: grid;
		grid-template-columns: auto 1fr auto;
		align-items: center;
		gap: var(--sp-2);
		padding: 7px 10px;
		text-align: left;
		color: var(--text-3);
		background: transparent;
		border: 1px solid transparent;
		border-radius: var(--radius-md);
	}
	.tool:hover {
		background: var(--bg-3);
	}
	.tool.on {
		color: var(--blue-2);
		background: var(--blue-weak);
		border-color: var(--blue-line);
	}
	.ttitle {
		font-size: var(--text-sm);
		color: var(--text-1);
	}
	.tout {
		font-size: var(--text-2xs);
		color: var(--text-4);
	}
	.panel {
		border-bottom: 0;
	}
	.phead {
		display: flex;
		align-items: center;
		justify-content: space-between;
	}
	.phead h2 {
		font-size: var(--text-lg);
		font-weight: var(--weight-strong);
	}
	.psum {
		margin: 4px 0 var(--sp-3);
		font-size: var(--text-xs);
		color: var(--text-3);
	}
	.runrow {
		display: flex;
		align-items: center;
		gap: var(--sp-3);
		margin-top: var(--sp-2);
	}
	.run {
		min-width: 104px;
	}
	.keys {
		display: inline-flex;
		gap: 3px;
	}
	.spin {
		animation: spin 0.9s linear infinite;
	}
	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}
	.field.sm {
		padding: 2px 6px;
		font-size: var(--text-xs);
	}
</style>
