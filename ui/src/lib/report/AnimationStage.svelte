<script lang="ts">
	/**
	 * A geometry's animation on a page of its own (`hertzforge export-animation`): the shell's own
	 * 3-D view filling the window, playing, with its own controls (orbit, zoom, play, scrub,
	 * amplitude), so a presentation can hold it in an iframe with no service running.
	 */
	import type { Shape3DView } from '$lib/protocol/types';
	import Shape3DPane from '$lib/viz/Shape3DPane.svelte';

	export interface AnimationSetup {
		/** Index into the view's modes the page opens at, when it has no motion. */
		mode?: number;
		/** The amplitude the page opens at, a multiple of the app's own. */
		scale?: number;
	}

	interface Props {
		view: Shape3DView;
		setup: AnimationSetup;
	}
	let { view, setup }: Props = $props();
	// svelte-ignore state_referenced_locally
	let mode = $state(setup.mode ?? 0);
</script>

<div class="stage" data-testid="animation-stage">
	<Shape3DPane {view} bind:mode startAmplitude={setup.scale ?? 1} />
</div>

<style>
	/* the window is the stage: locked to it only on this page (viewer.ts marks it), since the
	   report pages share this bundle and its global rules, and a report has to scroll */
	:global(html.stage-page),
	:global(html.stage-page body) {
		margin: 0;
		height: 100%;
		overflow: hidden;
	}
	.stage {
		position: fixed;
		inset: 0;
		display: flex;
		flex-direction: column;
		background: var(--bg-1);
	}
	.stage :global(.shape3d) {
		flex: 1;
		border: none;
		border-radius: 0;
	}
</style>
