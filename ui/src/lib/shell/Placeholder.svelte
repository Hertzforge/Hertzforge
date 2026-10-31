<script lang="ts">
	/** A workspace still to come: what moves here, said plainly. */
	import type { WorkspaceInfo } from '$lib/app/workspaces';
	import Icon from './Icon.svelte';

	interface Props {
		info: WorkspaceInfo;
	}
	let { info }: Props = $props();
</script>

<section class="placeholder" data-testid="placeholder-{info.id}" aria-labelledby="ph-title">
	<div class="card">
		<div class="icon"><Icon name={info.id} size={28} /></div>
		<p class="eyebrow">Planned Workspace</p>
		<h1 id="ph-title">{info.title}</h1>
		<p class="summary">{info.summary}</p>
		{#if info.moves}
			<p class="eyebrow moves-title">Moves Here</p>
			<ul class="moves">
				{#each info.moves as m (m)}<li>{m}</li>{/each}
			</ul>
		{/if}
		<p class="note">
			This workspace arrives in a later slice. Its tools will publish operations to the same catalog, so they
			appear in the command palette and build their panels the same way Analyze does.
		</p>
	</div>
</section>

<style>
	.placeholder {
		height: 100%;
		padding: var(--sp-8);
		overflow: auto;
	}

	.card {
		max-width: 560px;
		padding: var(--sp-6) var(--sp-6) var(--sp-5);
		background: var(--bg-1);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-lg);
	}

	.icon {
		display: inline-grid;
		place-items: center;
		width: 48px;
		height: 48px;
		margin-bottom: var(--sp-4);
		color: var(--blue-2);
		background: var(--blue-weak);
		border: 1px solid var(--blue-line);
		border-radius: var(--radius-lg);
	}

	h1 {
		margin: 4px 0 6px;
		font-size: var(--text-2xl);
		font-weight: var(--weight-strong);
		letter-spacing: -0.01em;
	}

	.summary {
		color: var(--text-2);
		font-size: var(--text-lg);
	}

	.moves-title {
		margin-top: var(--sp-5);
	}

	.moves {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
		margin: var(--sp-2) 0 0;
		padding: 0;
		list-style: none;
	}

	.moves li {
		padding: 3px 10px;
		font-size: var(--text-sm);
		color: var(--text-1);
		background: var(--bg-3);
		border: 1px solid var(--line-2);
		border-radius: 12px;
	}

	.note {
		margin-top: var(--sp-5);
		padding-top: var(--sp-4);
		border-top: 1px solid var(--line-1);
		font-size: var(--text-sm);
		color: var(--text-3);
		max-width: 60ch;
	}
</style>
