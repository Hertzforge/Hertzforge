<script lang="ts">
	/**
	 * Modal: the study of FRFs and shape sets, in the shared host, with the fitting screen in place
	 * of the action table and the reading while a fit is open, the project tree beside it (Brandon,
	 * 2026-11-14). A fit begins from the action table (an FRF alone, or an FRF with the shape set
	 * to edit). It is kept in the project as it goes, so selecting anything in the tree leaves it.
	 */
	import { untrack } from 'svelte';
	import type { AppState } from '$lib/app/state.svelte';
	import FitPanel from '$lib/modal/FitPanel.svelte';
	import Analyze from './Analyze.svelte';

	interface Props {
		app: AppState;
	}
	let { app }: Props = $props();

	// a selection made while fitting leaves the fit, which the project already holds
	let fittingFrom: unknown = null;
	$effect(() => {
		const selection = app.project.selection;
		const fitting = !!app.fit.state;
		untrack(() => {
			if (!fitting) fittingFrom = null;
			else if (fittingFrom === null) fittingFrom = selection;
			else if (selection !== fittingFrom) void app.fit.end();
		});
	});
</script>

{#if app.fit.state}
	<Analyze {app} only="modal">
		{#snippet replacing()}
			<div class="fitting" aria-label="Fitting modes">
				<FitPanel fit={app.fit} />
			</div>
		{/snippet}
	</Analyze>
{:else}
	<Analyze {app} only="modal" />
{/if}

<style>
	/* the fit screen takes the whole height of the frame, so its plot can */
	.fitting {
		display: flex;
		flex: 1 1 0;
		flex-direction: column;
		min-width: 0;
		min-height: 0;
	}

	.fitting > :global(*) {
		flex: 1 1 0;
	}
</style>
