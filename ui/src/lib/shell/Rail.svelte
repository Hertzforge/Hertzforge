<script lang="ts">
	/** The workspace rail. Alt plus 1 to 7 jumps; arrow keys move between items. */
	import { WORKSPACES } from '$lib/app/workspaces';
	import type { Workspace } from '$lib/protocol/types';
	import Icon from './Icon.svelte';

	interface Props {
		current: Workspace;
		onselect: (ws: Workspace) => void;
	}

	let { current, onselect }: Props = $props();
	let nav: HTMLElement;

	function onKey(e: KeyboardEvent) {
		if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
		const buttons = [...nav.querySelectorAll<HTMLButtonElement>('button')];
		const i = buttons.indexOf(document.activeElement as HTMLButtonElement);
		if (i < 0) return;
		e.preventDefault();
		buttons[(i + (e.key === 'ArrowDown' ? 1 : buttons.length - 1)) % buttons.length].focus();
	}
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<nav class="rail" aria-label="Workspaces" bind:this={nav} onkeydown={onKey}>
	{#each WORKSPACES as w, i (w.id)}
		<button
			class="item"
			data-testid="rail-{w.id}"
			class:active={w.id === current}
			class:planned={!w.live}
			aria-current={w.id === current ? 'page' : undefined}
			title="{w.title} (Alt+{i + 1}){w.live ? '' : ', planned'}"
			onclick={() => onselect(w.id)}
		>
			<span class="ico"><Icon name={w.id} size={20} /></span>
			<span class="lbl">{w.rail}</span>
		</button>
	{/each}
</nav>

<style>
	.rail {
		display: flex;
		flex-direction: column;
		gap: 2px;
		width: var(--rail-width);
		padding: var(--sp-2) 6px;
		background: var(--bg-1);
		border-right: 1px solid var(--line-1);
		overflow-y: auto;
	}

	.item {
		position: relative;
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 4px;
		padding: 9px 2px 8px;
		color: var(--text-3);
		background: transparent;
		border: 0;
		border-radius: var(--radius-md);
		transition:
			color var(--dur-fast) var(--ease),
			background var(--dur-fast) var(--ease);
	}

	.item:hover {
		color: var(--text-1);
		background: var(--bg-3);
	}

	.item.active {
		color: var(--text-1);
		background: var(--bg-3);
	}

	.item.active::before {
		content: '';
		position: absolute;
		left: -6px;
		top: 10px;
		bottom: 10px;
		width: 3px;
		background: var(--blue-2);
		border-radius: 0 2px 2px 0;
	}

	.item.active .ico {
		color: var(--blue-2);
	}

	.ico {
		display: inline-flex;
	}

	.lbl {
		max-width: 72px;
		font-size: 10.5px;
		font-weight: var(--weight-medium);
		line-height: 1.2;
		text-align: center;
	}

	.planned .lbl {
		color: var(--text-4);
	}

	.planned:hover .lbl,
	.planned.active .lbl {
		color: var(--text-2);
	}
</style>
