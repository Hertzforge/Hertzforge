<script lang="ts">
	/**
	 * A report drawn by the shell's own viewers (`report.views`): its marking above and below,
	 * its title, and each block in turn, text as the core wrote it from the block's markdown and
	 * each figure's views through `ViewRenderer`, under its number and caption, as every module
	 * draws them. It knows nothing about the service: it is handed the page and says which block
	 * was clicked. With `editing`, an unbound slot shows as a card saying what it was, a click
	 * selects its block (the report itself with a click outside the blocks), and the selected
	 * block is outlined; read, nothing is selectable and unbound slots are left out.
	 */
	import type { ReportPageBlock, ReportPageData } from '$lib/protocol/types';
	import ViewRenderer from '$lib/viz/ViewRenderer.svelte';

	interface Props {
		page: ReportPageData;
		editing?: boolean;
		/** The report block selected, or null for the report itself. */
		selected?: number | null;
		onselect?: (block: number | null) => void;
	}

	let { page, editing = false, selected = null, onselect }: Props = $props();

	const shown = $derived(editing ? page.blocks : page.blocks.filter((b) => b.kind !== 'unbound'));

	/** The height a figure's view is given: a 3-D scene and a long bar chart want more. */
	function height(block: ReportPageBlock): number {
		const view = block.views?.[0];
		if (!view) return 240;
		if (view.view === 'shape3d' || view.view === 'waterfall') return 380;
		if (view.view === 'table' || view.view === 'bars' || view.view === 'reading') return 0;
		return 300;
	}

	function choose(e: MouseEvent, block: number | null) {
		if (!editing) return;
		e.stopPropagation();
		onselect?.(block);
	}

	function keyed(e: KeyboardEvent, block: number | null) {
		if (editing && (e.key === 'Enter' || e.key === ' ')) {
			e.preventDefault();
			onselect?.(block);
		}
	}
</script>

<!-- a click between the blocks selects the report itself; the blocks take the keyboard -->
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
<article class="page" class:editing data-testid="report-page" onclick={(e) => choose(e, null)}>
	{#if page.marking}<p class="marking" data-testid="report-marking-top">{page.marking}</p>{/if}
	<h1 class="title">{page.title}</h1>
	{#each shown as block, k (k)}
		{@const picked = editing && block.block !== null && block.block === selected}
		<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
		<section
			class="block {block.kind}"
			class:picked
			data-testid="report-block"
			data-block={block.block}
			role={editing ? 'button' : undefined}
			tabindex={editing ? 0 : undefined}
			aria-pressed={editing ? picked : undefined}
			aria-label={editing ? `Block ${(block.block ?? 0) + 1}${block.label ? `, ${block.label}` : ''}` : undefined}
			onclick={(e) => choose(e, block.block)}
			onkeydown={(e) => keyed(e, block.block)}
		>
			{#if block.kind === 'text'}
				<!-- the core's escaped subset of markdown: headings, emphasis, code and lists -->
				<div class="text">{@html block.html ?? ''}</div>
			{:else if block.kind === 'figure'}
				<figure>
					{#each block.views ?? [] as view, i (i)}
						<div class="view" style:height={height(block) ? `${height(block)}px` : undefined}>
							<ViewRenderer {view} showTitle={false} minHeight={height(block) || 160} />
						</div>
					{/each}
					{#if block.label || block.caption}
						<figcaption><b>{block.label}</b>{block.label && block.caption ? '. ' : ''}{block.caption}</figcaption>
					{/if}
					{#if block.note}<p class="note">{block.note}</p>{/if}
				</figure>
			{:else}
				<div class="unbound" data-testid="report-unbound">
					{block.empty ? `This ${block.was ?? 'block'} has nothing to draw from what it is bound to.` : `This ${block.was ?? 'block'} is not bound to anything in the project yet; choose its source in the pane.`}
				</div>
			{/if}
		</section>
	{/each}
	{#if page.marking}<p class="marking" data-testid="report-marking-bottom">{page.marking}</p>{/if}
</article>

<style>
	.page {
		max-width: 960px;
		margin: 0 auto;
		padding: var(--sp-4) var(--sp-5) var(--sp-6);
		color: var(--text-1);
		font-size: var(--text-sm);
		line-height: 1.5;
	}

	.marking {
		margin: 0 0 var(--sp-3);
		text-align: center;
		font-weight: 700;
		letter-spacing: 0.06em;
		text-transform: uppercase;
		font-size: var(--text-xs);
	}

	.title {
		margin: 0 0 var(--sp-4);
		font-size: 1.6em;
		font-weight: 650;
	}

	.block {
		margin: 0 0 var(--sp-4);
		border-radius: var(--radius-md);
		outline: 2px solid transparent;
		outline-offset: 4px;
	}

	.editing .block {
		cursor: pointer;
	}

	.editing .block:hover {
		outline-color: var(--line-2);
	}

	.block.picked,
	.editing .block.picked:hover {
		outline-color: var(--accent);
	}

	.text :global(h2),
	.text :global(h3),
	.text :global(h4) {
		margin: var(--sp-4) 0 var(--sp-2);
		font-weight: 650;
	}

	.text :global(p) {
		margin: 0 0 var(--sp-2);
	}

	figure {
		margin: 0;
	}

	.view {
		min-width: 0;
		margin-bottom: var(--sp-2);
	}

	figcaption {
		color: var(--text-2);
		font-size: var(--text-xs);
	}

	.note {
		margin: 2px 0 0;
		color: var(--text-3);
		font-size: var(--text-2xs, 11px);
		font-style: italic;
	}

	.unbound {
		padding: var(--sp-3);
		border: 1px dashed var(--line-2);
		border-radius: var(--radius-md);
		color: var(--text-3);
		font-size: var(--text-xs);
	}
</style>
