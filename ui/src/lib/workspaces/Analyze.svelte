<script lang="ts">
	/**
	 * Analyze: pick a random-run dataset, pick a tool that accepts it, set its parameters, Run.
	 * Each run adds a result card; spectrum panes that share a `link` share a cursor.
	 */
	import type { AppState } from '$lib/app/state.svelte';
	import ParamForm from '$lib/forms/ParamForm.svelte';
	import Icon from '$lib/shell/Icon.svelte';
	import ViewRenderer from '$lib/viz/ViewRenderer.svelte';

	interface Props {
		app: AppState;
	}
	let { app }: Props = $props();

	const a = $derived(app.analyze);
	const isMac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);

	function onKey(e: KeyboardEvent) {
		if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
			e.preventDefault();
			void a.run();
		}
	}

	function summaryChips(summary: Record<string, unknown>): { label: string; value: string }[] {
		const labels: Record<string, [string, string]> = {
			duration_s: ['Duration', 's'],
			level_db: ['Level', 'dB'],
			band_hz: ['Band', 'Hz'],
			lines: ['Lines', ''],
			spec_rms_g: ['Spec rms', 'g']
		};
		return Object.entries(summary).map(([k, v]) => {
			const [label, unit] = labels[k] ?? [k.replace(/_/g, ' '), ''];
			return { label, value: `${v}${unit ? ` ${unit}` : ''}` };
		});
	}

	function time(d: Date) {
		return d.toLocaleTimeString('en-US', { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false });
	}
</script>

<svelte:window onkeydown={onKey} />

<div class="analyze">
	<aside class="side" aria-label="Dataset and tools">
		<section class="block">
			<h2 class="eyebrow">Dataset</h2>
			{#if !app.catalog}
				<p class="muted">
					{#if app.catalogError}{app.catalogError}{:else if app.analysis.isOpen}Loading the catalog.{:else}Waiting for the analysis service at <span class="mono">{app.analysis.url}</span>.{/if}
				</p>
			{:else if a.datasets.length === 0}
				<p class="muted">The analysis service lists no random-run datasets.</p>
			{:else}
				<div class="choices" role="radiogroup" aria-label="Dataset" data-testid="dataset-picker">
					{#each a.datasets as ds (ds.id)}
						<button
							class="choice"
							data-testid="dataset-{ds.id}"
							role="radio"
							aria-checked={a.datasetId === ds.id}
							class:on={a.datasetId === ds.id}
							onclick={() => a.selectDataset(ds.id)}
						>
							<span class="ctitle">{ds.title}</span>
							{#if ds.description}<span class="cdesc">{ds.description}</span>{/if}
						</button>
					{/each}
				</div>
			{/if}
			{#if a.opening}<p class="muted small">Opening.</p>{/if}
			{#if a.openError}<p class="error" role="alert">{a.openError}</p>{/if}
			{#if a.opened}
				<dl class="chips">
					{#each summaryChips(a.opened.summary) as c (c.label)}
						<div><dt>{c.label}</dt><dd class="num">{c.value}</dd></div>
					{/each}
					<div><dt>Channels</dt><dd class="num">{a.opened.channels.length}</dd></div>
				</dl>
			{/if}
		</section>

		{#if a.opened}
			<section class="block">
				<h2 class="eyebrow">Tools <span class="for">for {a.opened.kind.replace('_', ' ')} data</span></h2>
				<div class="tools" role="radiogroup" aria-label="Tool" data-testid="tool-list">
					{#each a.tools as op (op.id)}
						<button class="tool" data-testid="tool-{op.id}" role="radio" aria-checked={a.opId === op.id} class:on={a.opId === op.id} onclick={() => a.selectOp(op.id)}>
							<Icon name="tool" size={15} />
							<span class="ttitle">{op.title}</span>
							<span class="tout">{op.outputs.join(', ')}</span>
						</button>
					{/each}
				</div>
			</section>
		{/if}

		{#if a.op && a.opened}
			<section class="block panel" aria-labelledby="op-title">
				<div class="phead">
					<h2 id="op-title">{a.op.title}</h2>
					<button class="btn ghost sm" onclick={() => a.resetParams()} title="Reset every field to its default">Defaults</button>
				</div>
				{#if a.op.summary}<p class="psum">{a.op.summary}</p>{/if}
				<ParamForm
					params={a.op.params}
					bind:values={a.values[a.op.id]}
					channels={a.opened.channels}
					showAllErrors={a.triedRun}
					idPrefix={a.op.id.replace('.', '-')}
				/>
				<div class="runrow">
					<button class="btn primary run" data-testid="run-op" onclick={() => a.run()} disabled={a.running || !app.analysis.isOpen}>
						{#if a.running}
							<svg class="spin" viewBox="0 0 12 12" width="12" height="12" aria-hidden="true"><path d="M6 1.5a4.5 4.5 0 104.5 4.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" /></svg>
							Running
						{:else}
							<Icon name="play" size={13} /> Run
						{/if}
					</button>
					<span class="keys"><kbd>{isMac ? 'Cmd' : 'Ctrl'}</kbd><kbd>Enter</kbd></span>
				</div>
				{#if a.runError}<p class="error" role="alert">{a.runError}</p>{/if}
			</section>
		{/if}
	</aside>

	<main class="results" aria-label="Results">
		<div class="rhead">
			<h2>Results</h2>
			<span class="count num">{a.results.length}</span>
			<span class="hint">
				<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M5 7l2-2M4.2 8.8a1.8 1.8 0 01-2.5-2.5l1.4-1.4M7.8 3.2a1.8 1.8 0 012.5 2.5L8.9 7.1" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" /></svg>
				Panes on the same frequency axis share a cursor
			</span>
			{#if a.results.length}
				<button class="btn ghost sm" data-testid="results-clear" onclick={() => a.clearResults()}><Icon name="trash" size={14} /> Clear</button>
			{/if}
		</div>

		{#if a.results.length === 0}
			<div class="empty">
				<div class="frame" aria-hidden="true">
					<svg viewBox="0 0 220 90" width="220" height="90">
						<path d="M10 80V10M10 80h200" stroke="var(--line-3)" fill="none" />
						<path d="M14 66c20-2 26-34 46-34h70c20 0 40 20 76 26" stroke="var(--series-reference)" stroke-dasharray="7 4" fill="none" opacity="0.6" />
						<path d="M14 70c18-4 26-30 46-36 8 2 12-4 20 0 10 6 16-10 26-2 8 6 16-4 24 0 14 4 30 18 70 26" stroke="var(--series-measured)" fill="none" stroke-width="1.6" />
					</svg>
				</div>
				<h3>No Results Yet</h3>
				<p>
					{#if !a.opened}Pick a dataset on the left. The tools that accept it appear below it.{:else if !a.op}Pick a tool.{:else}Set the parameters and press Run. Each run adds a pane here, and running a second tool puts them side by side.{/if}
				</p>
			</div>
		{:else}
			<div class="grid">
				{#each a.results as r (r.id)}
					<article class="card" data-testid="result-card" data-op={r.opId} aria-label="{r.opTitle} result">
						<header>
							<div class="ctext">
								<h3>{r.opTitle}</h3>
								<p class="meta">{r.datasetTitle}</p>
								<p class="meta params">{r.paramSummary}</p>
							</div>
							<span class="at num">{time(r.at)}</span>
							<button class="btn ghost icon sm" data-testid="result-remove" onclick={() => a.removeResult(r.id)} aria-label="Remove this result" title="Remove">
								<Icon name="close" size={14} />
							</button>
						</header>
						<div class="views">
							{#each r.views as v, i (i)}
								<div class="view {v.view}">
									<ViewRenderer view={v} layout="strip" showTitle={v.view === 'spectrum'} minHeight={240} />
								</div>
							{/each}
						</div>
					</article>
				{/each}
			</div>
		{/if}
	</main>
</div>

<style>
	.analyze {
		display: grid;
		grid-template-columns: 316px minmax(0, 1fr);
		height: 100%;
		min-height: 0;
	}

	.side {
		display: flex;
		flex-direction: column;
		gap: 0;
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

	.for {
		margin-left: 4px;
		font-weight: var(--weight-regular);
		text-transform: none;
		letter-spacing: 0;
		color: var(--text-4);
	}

	.muted {
		color: var(--text-3);
		font-size: var(--text-sm);
	}

	.small {
		margin-top: var(--sp-2);
		font-size: var(--text-xs);
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

	.choices,
	.tools {
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

	.chips {
		display: grid;
		grid-template-columns: repeat(3, 1fr);
		gap: 1px;
		margin: var(--sp-3) 0 0;
		background: var(--line-1);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-md);
		overflow: hidden;
	}

	.chips div {
		padding: 5px 8px;
		background: var(--bg-2);
	}

	.chips dt {
		font-size: var(--text-2xs);
		color: var(--text-3);
	}

	.chips dd {
		margin: 0;
		font-size: var(--text-xs);
		color: var(--text-1);
		white-space: nowrap;
	}

	.tool {
		display: grid;
		grid-template-columns: auto 1fr auto;
		align-items: center;
		gap: var(--sp-2);
		padding: 7px 10px;
		text-align: left;
		color: var(--text-3);
		background: transparent;
		border: 1px solid transparent;
		border-radius: var(--radius-md);
	}

	.tool:hover {
		background: var(--bg-3);
	}

	.tool.on {
		color: var(--blue-2);
		background: var(--blue-weak);
		border-color: var(--blue-line);
	}

	.ttitle {
		font-size: var(--text-sm);
		color: var(--text-1);
	}

	.tout {
		font-size: var(--text-2xs);
		color: var(--text-4);
	}

	.panel {
		border-bottom: 0;
	}

	.phead {
		display: flex;
		align-items: center;
		justify-content: space-between;
	}

	.phead h2 {
		font-size: var(--text-lg);
		font-weight: var(--weight-strong);
	}

	.psum {
		margin: 4px 0 var(--sp-3);
		font-size: var(--text-xs);
		color: var(--text-3);
	}

	.runrow {
		display: flex;
		align-items: center;
		gap: var(--sp-3);
		margin-top: var(--sp-2);
	}

	.run {
		min-width: 104px;
	}

	.keys {
		display: inline-flex;
		gap: 3px;
	}

	.results {
		display: flex;
		flex-direction: column;
		min-height: 0;
		min-width: 0;
		overflow-y: auto;
		padding: var(--sp-4) var(--sp-5) var(--sp-6);
	}

	.rhead {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		margin-bottom: var(--sp-3);
	}

	.rhead h2 {
		font-size: var(--text-lg);
		font-weight: var(--weight-strong);
	}

	.count {
		min-width: 20px;
		padding: 0 6px;
		font-size: var(--text-xs);
		text-align: center;
		color: var(--text-2);
		background: var(--bg-3);
		border-radius: 9px;
	}

	.hint {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		margin-left: var(--sp-3);
		margin-right: auto;
		font-size: var(--text-xs);
		color: var(--text-3);
	}

	.empty {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		max-width: 460px;
		margin: 8vh 0 0 4vw;
	}

	.frame {
		margin-bottom: var(--sp-4);
		padding: var(--sp-3);
		background: var(--bg-1);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-lg);
	}

	.empty h3 {
		font-size: var(--text-xl);
		font-weight: var(--weight-strong);
	}

	.empty p {
		margin-top: 6px;
		color: var(--text-2);
	}

	.grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(min(100%, 460px), 1fr));
		gap: var(--sp-4);
		align-items: start;
	}

	.card {
		display: flex;
		flex-direction: column;
		min-width: 0;
		background: var(--bg-1);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-lg);
		animation: enter var(--dur-med) var(--ease);
	}

	.card header {
		display: flex;
		align-items: flex-start;
		gap: var(--sp-2);
		padding: var(--sp-3) var(--sp-3) var(--sp-2) var(--sp-4);
		border-bottom: 1px solid var(--line-1);
	}

	.ctext {
		flex: 1;
		min-width: 0;
	}

	.card h3 {
		font-size: var(--text-md);
		font-weight: var(--weight-strong);
	}

	.meta {
		font-size: var(--text-xs);
		color: var(--text-3);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.params {
		color: var(--text-2);
	}

	.at {
		padding-top: 2px;
		font-size: var(--text-2xs);
		color: var(--text-4);
	}

	.views {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		padding: var(--sp-3) var(--sp-3) var(--sp-3);
	}

	.view.spectrum {
		height: 340px;
	}

	.spin {
		animation: spin 0.9s linear infinite;
	}

	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}

	@keyframes enter {
		from {
			opacity: 0;
			transform: translateY(4px);
		}
	}
</style>
