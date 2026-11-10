<script lang="ts">
	/**
	 * `+ Plot`: every plot type the test offers, grouped, each with its one line of help.
	 * (Placeholder until the display options land: a plain list.)
	 */
	import type { PlotType } from './types';

	interface Props {
		types: PlotType[];
		onpick: (type: PlotType) => void;
		onclose: () => void;
	}
	let { types, onpick }: Props = $props();
</script>

<div class="menu" role="menu" aria-label="Add a plot">
	{#each types as t (t.id)}
		<button role="menuitem" class="item" title={t.help} onclick={() => onpick(t)}>{t.words}</button>
	{/each}
</div>
