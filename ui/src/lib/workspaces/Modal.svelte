<script lang="ts">
	/**
	 * Modal: pick a model; `modal.modes` runs on it automatically. The mode table sits beside the
	 * shape view; arrow keys in the table step through modes.
	 */
	import type { AppState } from '$lib/app/state.svelte';
	import { formatFixed } from '$lib/viz/format';
	import ViewRenderer from '$lib/viz/ViewRenderer.svelte';

	interface Props {
		app: AppState;
	}
	let { app }: Props = $props();

	const m = $derived(app.modal);
	let table = $state<HTMLElement | null>(null);

	// The first model opens on arrival, so the view is never empty when there is something to show.
	$effect(() => {
		const first = m.datasets[0];
		if (first && !m.datasetId) void m.select(first.id);
	});

	function onTableKey(e: KeyboardEvent) {
		const n = m.view?.modes.length ?? 0;
		if (!n) return;
		let next = m.mode;
		if (e.key === 'ArrowDown') next = Math.min(n - 1, m.mode + 1);
		else if (e.key === 'ArrowUp') next = Math.max(0, m.mode - 1);
		else if (e.key === 'Home') next = 0;
		else if (e.key === 'End') next = n - 1;
		else return;
		e.preventDefault();
		m.mode = next;
		table?.querySelector<HTMLElement>(`[data-i="${next}"]`)?.scrollIntoView({ block: 'nearest' });
	}
</script>

<div class="modal">
	<aside class="side" aria-label="Model and modes">
		<section class="block">
			<h2 class="eyebrow">Model</h2>
			{#if !app.catalog}
				<p class="muted">
					{#if app.catalogError}{app.catalogError}{:else}Waiting for the analysis service at <span class="mono">{app.analysis.url}</span>.{/if}
				</p>
			{:else if m.datasets.length === 0}
				<p class="muted">The analysis service lists no modal models.</p>
			{:else}
				<div class="choices" role="radiogroup" aria-label="Model" data-testid="model-picker">
					{#each m.datasets as ds (ds.id)}
						<button class="choice" data-testid="dataset-{ds.id}" role="radio" aria-checked={m.datasetId === ds.id} class:on={m.datasetId === ds.id} onclick={() => m.select(ds.id)}>
							<span class="ctitle">{ds.title}</span>
							{#if ds.description}<span class="cdesc">{ds.description}</span>{/if}
						</button>
					{/each}
				</div>
			{/if}
			{#if m.error}<p class="error" role="alert">{m.error}</p>{/if}
		</section>

		<section class="block modes">
			<h2 class="eyebrow">Modes</h2>
			{#if m.loading}
				<p class="muted">Running Mode Shapes.</p>
			{:else if m.view}
				<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
				<div
					class="table"
					role="listbox"
					data-testid="mode-list"
					tabindex="0"
					aria-label="Modes"
					aria-activedescendant="mode-{m.mode}"
					bind:this={table}
					onkeydown={onTableKey}
				>
					<div class="thead" aria-hidden="true">
						<span>Mode</span><span>Frequency (Hz)</span><span>Damping (%)</span>
					</div>
					{#each m.view.modes as md, i (md.index)}
						<!-- svelte-ignore a11y_click_events_have_key_events -->
						<div
							class="tr"
							id="mode-{i}"
							data-i={i}
							data-testid="mode-row-{md.index}"
							role="option"
							tabindex="-1"
							aria-selected={m.mode === i}
							class:on={m.mode === i}
							onclick={() => (m.mode = i)}
						>
							<span class="idx num">{md.index}</span>
							<span class="num">{formatFixed(md.frequency_hz, 5)}</span>
							<span class="num">{md.damping == null ? '-' : formatFixed(md.damping * 100, 3)}</span>
						</div>
					{/each}
				</div>
				<p class="hint">Up and Down step through modes. In the view: Space plays or pauses, W wireframe, G ghost, R resets.</p>
			{/if}
		</section>
	</aside>

	<main class="stage" aria-label="Mode shape">
		{#if m.view}
			<ViewRenderer view={m.view} bind:mode={m.mode} />
		{:else}
			<div class="placeholder">
				{#if m.loading}Building the model.{:else if m.error}{m.error}{:else}Pick a model.{/if}
			</div>
		{/if}
	</main>
</div>

<style>
	.modal {
		display: grid;
		grid-template-columns: 316px minmax(0, 1fr);
		height: 100%;
		min-height: 0;
	}

	.side {
		display: flex;
		flex-direction: column;
		min-height: 0;
		overflow-y: auto;
		background: var(--bg-1);
		border-right: 1px solid var(--line-1);
	}

	.block {
		padding: var(--sp-4);
		border-bottom: 1px solid var(--line-1);
	}

	.block h2.eyebrow {
		margin-bottom: var(--sp-2);
	}

	.modes {
		border-bottom: 0;
	}

	.muted {
		color: var(--text-3);
		font-size: var(--text-sm);
	}

	.error {
		margin-top: var(--sp-2);
		padding: 6px 8px;
		font-size: var(--text-xs);
		color: var(--amber-2);
		background: var(--amber-weak);
		border-left: 2px solid var(--amber-2);
		border-radius: var(--radius-sm);
	}

	.choices {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.choice {
		display: flex;
		flex-direction: column;
		gap: 2px;
		padding: 8px 10px;
		text-align: left;
		background: transparent;
		border: 1px solid transparent;
		border-radius: var(--radius-md);
	}

	.choice:hover {
		background: var(--bg-3);
	}

	.choice.on {
		background: var(--blue-weak);
		border-color: var(--blue-line);
	}

	.ctitle {
		font-size: var(--text-sm);
		font-weight: var(--weight-medium);
		color: var(--text-1);
	}

	.cdesc {
		font-size: var(--text-xs);
		color: var(--text-3);
	}

	.table {
		border: 1px solid var(--line-1);
		border-radius: var(--radius-md);
		overflow: hidden;
	}

	.table:focus-visible {
		outline-offset: 1px;
	}

	.thead,
	.tr {
		display: grid;
		grid-template-columns: 48px 1fr 1fr;
		gap: var(--sp-2);
		padding: 0 var(--sp-3);
		align-items: center;
	}

	.thead {
		height: 28px;
		font-size: var(--text-2xs);
		color: var(--text-3);
		background: var(--bg-2);
		border-bottom: 1px solid var(--line-1);
	}

	.thead span:not(:first-child),
	.tr span:not(:first-child) {
		text-align: right;
	}

	.tr {
		height: 30px;
		font-size: var(--text-sm);
		color: var(--text-2);
		cursor: pointer;
		border-bottom: 1px solid var(--line-1);
	}

	.tr:last-child {
		border-bottom: 0;
	}

	.tr:hover {
		background: var(--bg-3);
	}

	.tr.on {
		color: var(--text-1);
		background: var(--blue-weak);
		box-shadow: inset 3px 0 0 var(--blue-2);
	}

	.idx {
		color: var(--text-3);
	}

	.tr.on .idx {
		color: var(--blue-1);
	}

	.hint {
		margin-top: var(--sp-3);
		font-size: var(--text-2xs);
		color: var(--text-3);
	}

	.stage {
		min-width: 0;
		min-height: 0;
		padding: var(--sp-4);
	}

	.placeholder {
		display: grid;
		place-items: center;
		height: 100%;
		color: var(--text-3);
		background: var(--bg-plot);
		border: 1px dashed var(--line-2);
		border-radius: var(--radius-md);
	}
</style>
