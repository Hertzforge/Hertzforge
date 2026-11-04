<script lang="ts">
	/**
	 * Modal: the study of FRFs and shape sets, in the shared host, with the fitting screen in its
	 * place while a fit is open. A fit begins from the Actions frame (an FRF alone, or an FRF with
	 * the shape set to edit) and ends from its own bar.
	 */
	import type { AppState } from '$lib/app/state.svelte';
	import FitPanel from '$lib/modal/FitPanel.svelte';
	import Analyze from './Analyze.svelte';

	interface Props {
		app: AppState;
	}
	let { app }: Props = $props();
</script>

{#if app.fit.state}
	<main class="fitting" aria-label="Fitting modes">
		<FitPanel fit={app.fit} />
	</main>
{:else}
	<Analyze {app} only="modal" />
{/if}

<style>
	.fitting {
		height: 100%;
		min-width: 0;
		min-height: 0;
		padding: var(--sp-4);
	}
</style>
