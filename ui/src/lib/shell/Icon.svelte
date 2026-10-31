<script lang="ts" module>
	export type IconName =
		| 'acquire'
		| 'analyze'
		| 'modal'
		| 'model'
		| 'specify'
		| 'simulate'
		| 'report'
		| 'search'
		| 'play'
		| 'stop'
		| 'close'
		| 'tool'
		| 'dataset'
		| 'action'
		| 'sun'
		| 'moon'
		| 'system'
		| 'trash'
		| 'reset'
		| 'chevron';
</script>

<script lang="ts">
	/** Inline line icons on a 20 px grid, drawn with the current color. */
	interface Props {
		name: IconName;
		size?: number;
	}



	let { name, size = 20 }: Props = $props();
</script>

<svg
	viewBox="0 0 20 20"
	width={size}
	height={size}
	fill="none"
	stroke="currentColor"
	stroke-width="1.45"
	stroke-linecap="round"
	stroke-linejoin="round"
	aria-hidden="true"
>
	{#if name === 'acquire'}
		<rect x="2.5" y="4" width="15" height="12" rx="2" />
		<path d="M4.5 10.5c1.2 0 1.2-3.5 2.4-3.5s1.2 6 2.4 6 1.2-4.5 2.4-4.5 1.2 2.5 2.4 2.5h1.4" />
	{:else if name === 'analyze'}
		<path d="M3 3v14h14" />
		<path d="M5.5 13.5l2.5-5 2.5 3.5 2.2-6 2.3 4" />
	{:else if name === 'modal'}
		<path d="M2.5 10c2.5-5 5-5 7.5 0s5 5 7.5 0" />
		<path d="M2.5 10c2.5 5 5 5 7.5 0s5-5 7.5 0" opacity="0.45" />
		<circle cx="10" cy="10" r="1.1" fill="currentColor" stroke="none" />
	{:else if name === 'model'}
		<path d="M3 15.5L10 3.5l7 12z" />
		<path d="M6.5 9.5h7M10 15.5L6.5 9.5M10 15.5l3.5-6" />
	{:else if name === 'specify'}
		<path d="M5 2.5h7l3 3v12H5z" />
		<path d="M12 2.5v3h3" />
		<path d="M7.5 13.5l2-3 1.6 1.6 1.9-3.6" />
	{:else if name === 'simulate'}
		<circle cx="10" cy="10" r="7.5" />
		<path d="M8.2 6.8v6.4l5-3.2z" />
	{:else if name === 'report'}
		<path d="M5 2.5h10v15H5z" />
		<path d="M7.5 6.5h5M7.5 9.5h5M7.5 12.5h3" />
	{:else if name === 'search'}
		<circle cx="8.5" cy="8.5" r="5" />
		<path d="M12.3 12.3l4.2 4.2" />
	{:else if name === 'play'}
		<path d="M6 4.2v11.6l9.2-5.8z" fill="currentColor" stroke="none" />
	{:else if name === 'stop'}
		<rect x="5" y="5" width="10" height="10" rx="1.5" fill="currentColor" stroke="none" />
	{:else if name === 'close'}
		<path d="M5 5l10 10M15 5L5 15" />
	{:else if name === 'tool'}
		<path d="M3.5 16.5l6-6" />
		<path d="M12.5 3a4 4 0 00-3 6.5l1 1A4 4 0 0017 7.5l-2.5.5-1-1 .5-2.5z" />
	{:else if name === 'dataset'}
		<ellipse cx="10" cy="5" rx="6" ry="2.3" />
		<path d="M4 5v10c0 1.3 2.7 2.3 6 2.3s6-1 6-2.3V5" />
		<path d="M4 10c0 1.3 2.7 2.3 6 2.3s6-1 6-2.3" />
	{:else if name === 'action'}
		<path d="M11 2.5L4.5 11H10l-1 6.5L15.5 9H10z" />
	{:else if name === 'sun'}
		<circle cx="10" cy="10" r="3.2" />
		<path d="M10 2v2M10 16v2M2 10h2M16 10h2M4.3 4.3l1.4 1.4M14.3 14.3l1.4 1.4M4.3 15.7l1.4-1.4M14.3 5.7l1.4-1.4" />
	{:else if name === 'moon'}
		<path d="M15.5 12.5A6.5 6.5 0 017.5 4.5a6.5 6.5 0 108 8z" />
	{:else if name === 'system'}
		<rect x="2.5" y="3.5" width="15" height="10" rx="1.5" />
		<path d="M7 17h6M10 13.5V17" />
		<path d="M10 3.5v10" opacity="0.5" />
	{:else if name === 'trash'}
		<path d="M4 6h12M8 6V4h4v2M5.5 6l.8 10.5h7.4L14.5 6" />
	{:else if name === 'reset'}
		<path d="M4 10a6 6 0 106-6H7M7 1.5V4.5h3" />
	{:else if name === 'chevron'}
		<path d="M7.5 5l5 5-5 5" />
	{/if}
</svg>
