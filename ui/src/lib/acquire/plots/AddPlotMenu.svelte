<script lang="ts">
	/**
	 * `+ Plot`: every plot type the test offers, grouped (Frequency, Time, Numbers, Other), each
	 * with its one line of help. The first entry takes focus; the arrows, Home, End and a letter
	 * move through the list, Enter picks, Escape closes. Each entry is a menu item named by its
	 * type's words, the help its description. It draws no frame of its own: it sits in the popover
	 * that opens it.
	 */
	import { onMount } from 'svelte';
	import { stepIndex, typeAhead } from './keys';
	import type { PlotType } from './types';

	interface Props {
		types: PlotType[];
		onpick: (type: PlotType) => void;
		onclose: () => void;
	}
	let { types, onpick, onclose }: Props = $props();

	const GROUPS: [PlotType['group'], string][] = [
		['frequency', 'Frequency'],
		['time', 'Time'],
		['numbers', 'Numbers'],
		['other', 'Other']
	];
	const groups = $derived(GROUPS.map(([id, words]) => ({ id, words, types: types.filter((t) => t.group === id) })).filter((g) => g.types.length > 0));
	/** The entries in the order they are listed. */
	const listed = $derived(groups.flatMap((g) => g.types));

	const uid = `apm-${Math.random().toString(36).slice(2, 8)}`;
	let menu: HTMLDivElement;

	function items(): HTMLButtonElement[] {
		return menu ? [...menu.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')] : [];
	}

	onMount(() => items()[0]?.focus());

	function onkeydown(e: KeyboardEvent) {
		const list = items();
		const at = list.indexOf(document.activeElement as HTMLButtonElement);
		if (e.key === 'Escape') {
			// not stopped here: a popover around the menu closes too and gives its button the keyboard
			e.preventDefault();
			onclose();
			return;
		}
		if (e.key === 'Tab') {
			onclose();
			return;
		}
		const next = stepIndex(e.key, at, list.length) ?? (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey ? typeAhead(listed.map((t) => t.words), at, e.key) : null);
		if (next === null) return;
		e.preventDefault();
		list[next]?.focus();
	}
</script>

<!-- svelte-ignore a11y_interactive_supports_focus -->
<div class="menu" role="menu" aria-label="Add a plot" tabindex="-1" bind:this={menu} {onkeydown} data-testid="add-plot-menu">
	{#each groups as g (g.id)}
		<div class="group" role="group" aria-labelledby="{uid}-{g.id}">
			<div class="head" id="{uid}-{g.id}">{g.words}</div>
			{#each g.types as t (t.id)}
				<button role="menuitem" class="item" tabindex="-1" aria-label={t.words} aria-describedby={t.help ? `${uid}-${t.id}-help` : undefined} data-testid="add-plot-{t.id}" onclick={() => onpick(t)}>
					<span class="words">{t.words}</span>
					{#if t.help}<span class="help" id="{uid}-{t.id}-help">{t.help}</span>{/if}
				</button>
			{/each}
		</div>
	{:else}
		<p class="none">This test has no plots to add.</p>
	{/each}
</div>

<style>
	.menu {
		width: min(300px, calc(100vw - 40px));
		max-height: min(70vh, 520px);
		overflow: auto;
	}

	.menu:focus {
		outline: none;
	}

	.group + .group {
		margin-top: var(--sp-1);
		padding-top: var(--sp-1);
		border-top: 1px solid var(--line-1);
	}

	.head {
		padding: 4px 8px 2px;
		font-size: var(--text-2xs);
		font-weight: var(--weight-strong);
		letter-spacing: 0.08em;
		text-transform: uppercase;
		color: var(--text-3);
	}

	.item {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: 1px;
		width: 100%;
		padding: 5px 8px;
		text-align: left;
		color: var(--text-1);
		background: transparent;
		border: 0;
		border-radius: var(--radius-sm);
		cursor: pointer;
	}

	.item:hover,
	.item:focus-visible {
		background: var(--bg-4);
		outline: none;
	}

	.item:focus-visible {
		box-shadow: inset 0 0 0 1px var(--focus);
	}

	.words {
		font-size: var(--text-sm);
		font-weight: var(--weight-medium);
	}

	.help {
		font-size: var(--text-xs);
		color: var(--text-3);
		line-height: 1.35;
	}

	.none {
		margin: 0;
		padding: var(--sp-2);
		font-size: var(--text-sm);
		color: var(--text-3);
	}
</style>
