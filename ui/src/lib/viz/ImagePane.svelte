<script lang="ts">
	/**
	 * A photo of the test, drawn from the file's own bytes at its own aspect under its name, as
	 * Visual Dynamics stacks a Photos object's pictures. The bytes become a blob URL for the page's
	 * life and are let go when the pane goes.
	 */
	import type { ImageView } from '$lib/protocol/types';

	interface Props {
		view: ImageView;
		showTitle?: boolean;
	}
	let { view, showTitle = true }: Props = $props();

	let url = $state('');
	$effect(() => {
		const blob = new Blob([view.bytes], { type: `image/${view.format}` });
		const made = URL.createObjectURL(blob);
		url = made;
		return () => URL.revokeObjectURL(made);
	});
</script>

<figure class="photo" data-testid="image-view">
	{#if url}<img src={url} alt={view.title} />{/if}
	{#if showTitle}<figcaption>{view.title}</figcaption>{/if}
</figure>

<style>
	.photo {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		margin: 0;
		padding: var(--sp-3);
		background: var(--bg-plot);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-md);
	}

	img {
		display: block;
		max-width: 100%;
		max-height: 70vh;
		object-fit: contain;
		align-self: center;
		border-radius: var(--radius-sm);
	}

	figcaption {
		font-size: var(--text-sm);
		color: var(--text-2);
		text-align: center;
	}
</style>
