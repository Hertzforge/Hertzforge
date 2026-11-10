<script lang="ts">
	/**
	 * A plot panel's options menu, in sections: Data, Axes, Display, Cursors and Marks, Hold, Export.
	 * (Placeholder until the display options land: the plot type only.)
	 */
	import type { PlotPanelSpec, PlotType } from './types';

	interface Props {
		spec: PlotPanelSpec;
		types: PlotType[];
		/** The test's channels, for the channel choice. */
		channels: { index: number; name: string }[];
		onchange: (spec: PlotPanelSpec) => void;
		onexport?: (what: 'copy' | 'image' | 'csv') => void;
		onclose: () => void;
	}
	let { spec, types, onchange, onclose }: Props = $props();
</script>

<div class="opts" role="dialog" aria-label="Plot options">
	<label>Shows
		<select value={spec.quantity} onchange={(e) => onchange({ ...spec, quantity: e.currentTarget.value })}>
			{#each types as t (t.id)}<option value={t.id}>{t.words}</option>{/each}
		</select>
	</label>
	<button class="btn sm" onclick={onclose}>Done</button>
</div>
