<script lang="ts">
	/**
	 * One of the tree's icons at 16 px (or `size`): an object kind's, a record's quantity or
	 * fraction, or a sub-item's in its parent's color. The markup comes from icons.ts, which only
	 * ever builds it from its own constants and colors, never from text a service or a file sent.
	 */
	import { drawChild, drawObject, drawRecord } from './icons';

	interface Props {
		/** An object kind (or banded variant), a record's icon key, or a sub-item kind. */
		name: string;
		/** 'object' (the default), 'record' or 'child'. */
		of?: 'object' | 'record' | 'child';
		/** For a record, the owning object's icon key; for a child, the parent's kind. */
		parent?: string;
		/** A child of an empty category, drawn gray. */
		empty?: boolean;
		/** Units not yet declared: the amber badge. */
		badge?: boolean;
		size?: number;
		title?: string;
	}
	let { name, of = 'object', parent = '', empty = false, badge = false, size = 16, title }: Props = $props();

	const markup = $derived(
		of === 'record' ? drawRecord(name, parent, badge) : of === 'child' ? drawChild(name, parent, empty, badge) : drawObject(name, badge)
	);
</script>

<svg viewBox="0 0 64 64" width={size} height={size} aria-hidden={title ? undefined : 'true'} role={title ? 'img' : undefined} class="glyph" data-icon={name}>
	{#if title}<title>{title}</title>{/if}
	{@html markup}
</svg>

<style>
	.glyph {
		flex: none;
		display: inline-block;
		vertical-align: -3px;
	}
</style>
