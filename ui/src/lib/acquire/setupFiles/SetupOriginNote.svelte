<script lang="ts">
	/**
	 * Beside the Arm verb: where the setup being armed came from (typed here; a saved setup, by
	 * name; a run, by its id), or that it was edited here after it was opened. Shown once a test's
	 * setup bar has been on screen in this window.
	 */
	import { knownOrigin } from './setupOrigin.svelte';

	interface Props {
		/** The test's store, as the run verbs are given it. */
		host: object;
	}
	let { host }: Props = $props();

	const origin = $derived(knownOrigin(host));
</script>

{#if origin}
	<p class="aq-small origin" data-testid="arm-origin" data-kind={origin.origin.kind} data-edited={origin.edited}>{origin.words}</p>
{/if}

<style>
	.origin {
		margin: 0;
	}
</style>
