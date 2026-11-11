<script lang="ts">
	/**
	 * The exported report as a reader opens it: the page read-only, with a switch between the
	 * light and dark themes (the reader's own system theme to begin with, unless the file was
	 * written in one) that is remembered where the browser allows it.
	 */
	import { onMount } from 'svelte';
	import type { ReportPageData } from '$lib/protocol/types';
	import { theme } from '$lib/theme/theme.svelte';
	import ReportPage from './ReportPage.svelte';

	interface Props {
		page: ReportPageData | null;
		error?: string | null;
	}

	let { page, error = null }: Props = $props();

	/** How long the figures are given to draw before the print dialog: spectra and 3-D views draw a frame or two after mounting. */
	const SETTLE_MS = 1200;

	onMount(() => {
		const written = document.documentElement.getAttribute('data-theme');
		theme.init(written === 'light' || written === 'dark' ? written : null);
		if (page) document.title = page.title;
		// a page written to be printed (`report.print`, an issue) asks for the dialog itself
		if (page && document.querySelector('meta[name="hertzforge-print"]')) {
			void document.fonts.ready.then(() => setTimeout(() => window.print(), SETTLE_MS));
		}
	});

	function flip() {
		theme.set(theme.resolved === 'dark' ? 'light' : 'dark');
	}
</script>

<svelte:head>
	{#if page?.css}
		<!-- the core's own stylesheet for the printed page: its margins and running heads -->
		{@html `<style>${page.css.replace(/<\/style/gi, '<\\/style')}</style>`}
	{/if}
</svelte:head>

<div class="viewer">
	<button class="btn ghost sm flip" type="button" data-testid="report-theme" onclick={flip}>
		{theme.resolved === 'dark' ? 'Light mode' : 'Dark mode'}
	</button>
	{#if page}
		<ReportPage {page} />
	{:else}
		<p class="error" role="alert">{error ?? 'This page holds no report.'}</p>
	{/if}
</div>

<style>
	/* the shell locks the window to itself; a report is a document that scrolls */
	:global(html),
	:global(body) {
		height: auto;
		min-height: 100%;
		overflow: auto;
		background: var(--bg-0);
	}

	.viewer {
		position: relative;
		min-height: 100vh;
	}

	.flip {
		position: fixed;
		top: var(--sp-2);
		right: var(--sp-3);
		z-index: 2;
	}

	.error {
		padding: var(--sp-5);
		color: var(--text-2);
	}

	@media print {
		.flip {
			display: none;
		}
	}
</style>
