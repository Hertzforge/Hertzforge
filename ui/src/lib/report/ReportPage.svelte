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
	import type { ReportPageBlock, ReportPageData, View } from '$lib/protocol/types';
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

	/** The height a figure's view is given: a 3-D scene wants more, and a matrix a row's worth per row. */
	function height(block: ReportPageBlock): number {
		const view = block.views?.[0];
		if (!view) return 240;
		if (view.view === 'shape3d') return 380;
		// a stage of every channel in depth, as wide as the page: room to read it
		if (view.view === 'waterfall') return 520;
		if (view.view === 'table' || view.view === 'bars' || view.view === 'reading') return 0;
		// a coherence map or a MAC: tall enough that every row is labeled (the pane thins its
		// labels below 13 px a row), with room under it for the tilted ticks and the legend
		if (view.view === 'matrix') return Math.max(300, 16 * view.rows.ticks.length + 160);
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

<!-- the words for a plot's shaded zones and marks, under it: in a grid's cell and under a figure of one channel alike -->
{#snippet zoneKey(view: View)}
	{#if view.view === 'spectrum' && view.key?.length}
		<p class="key">
			{#each view.key as [code, words] (code)}<span class="keyed"><i class="swatch {code}"></i>{words}</span>{/each}
		</p>
	{/if}
{/snippet}

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
					{#if block.grid}
						<!-- a table of plots: a row a node, a column a direction -->
						<div class="plotgrid" data-testid="report-grid" style:--columns={Math.max(1, block.grid.columns.length)}>
							<div class="corner"></div>
							{#each block.grid.columns as column, c (c)}<div class="colhead">{column}</div>{/each}
							{#each block.grid.rows as row, r (r)}
								<div class="rowhead">{row.label}</div>
								{#each row.cells as cell, c (c)}
									<div class="cell" data-testid="report-grid-cell">
										{#each cell as placed (placed.view)}
											{@const view = block.views?.[placed.view]}
											{#if view}
												<div class="view" style:height="260px">
													<ViewRenderer {view} showTitle={false} minHeight={200} />
												</div>
												{@render zoneKey(view)}
												<p class="celltitle"><b>{placed.caption}</b>{placed.note ? ` (${placed.note})` : ''}</p>
												{#each placed.facts ?? [] as [label, text] (label)}
													<p class="fact"><b>{label}:</b> {text}</p>
												{/each}
											{/if}
										{/each}
									</div>
								{/each}
							{/each}
						</div>
					{:else}
					{#each block.views ?? [] as view, i (i)}
						<div class="view" style:height={height(block) ? `${height(block)}px` : undefined}>
							<ViewRenderer {view} showTitle={false} minHeight={height(block) || 160} whole />
						</div>
						{@render zoneKey(view)}
					{/each}
					{/if}
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
	.plotgrid {
		display: grid;
		grid-template-columns: max-content repeat(var(--columns), minmax(0, 1fr));
		gap: var(--sp-2) var(--sp-3);
		align-items: start;
	}

	.colhead {
		font-weight: 650;
		text-align: center;
	}

	.rowhead {
		padding-top: var(--sp-2);
		font-weight: 650;
		white-space: nowrap;
	}

	.cell {
		min-width: 0;
	}

	.key,
	.celltitle,
	.fact {
		margin: 2px 0 0;
		font-size: var(--text-xs);
	}

	.key {
		display: flex;
		flex-wrap: wrap;
		gap: 2px 12px;
		color: var(--text-2);
	}

	.keyed {
		display: inline-flex;
		align-items: center;
		gap: 4px;
	}

	.swatch {
		display: inline-block;
		width: 12px;
		height: 10px;
		border-radius: 2px;
	}

	.swatch.warning {
		background: color-mix(in srgb, #d9a441 40%, transparent);
	}

	.swatch.above {
		background: color-mix(in srgb, #d6453d 40%, transparent);
	}

	.swatch.below {
		background: color-mix(in srgb, #3d7fd6 40%, transparent);
	}

	.swatch.exceed_over {
		background: #d6453d;
	}

	.swatch.exceed_under {
		background: #3d7fd6;
	}

	/* the window's whole width, so a large screen gives the plots their room */
	.page {
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

	@media print {
		.page {
			padding: 0;
		}

		.block {
			break-inside: avoid;
			outline: none;
		}

		.title {
			break-after: avoid;
		}
	}

	.unbound {
		padding: var(--sp-3);
		border: 1px dashed var(--line-2);
		border-radius: var(--radius-md);
		color: var(--text-3);
		font-size: var(--text-xs);
	}
</style>
