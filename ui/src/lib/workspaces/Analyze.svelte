<script lang="ts">
	/**
	 * The study host every analysis module shares: the project tree, what the selection shows, the
	 * Actions frame, the tool panel and the result cards. A module (`only`) differs in which
	 * operations its Actions frame and tool list offer, its own and those offered everywhere, and in
	 * which demonstration datasets it opens. Each run adds a result card; spectrum panes that share
	 * a `link` share a cursor.
	 */
	import type { AnalyzeResult, AppState } from '$lib/app/state.svelte';
	import type { Workspace } from '$lib/protocol/types';
	import { offeredIn } from '$lib/app/workspaces';
	import ParamForm from '$lib/forms/ParamForm.svelte';
	import ProjectPanel from '$lib/project/ProjectPanel.svelte';
	import Icon from '$lib/shell/Icon.svelte';
	import ViewRenderer from '$lib/viz/ViewRenderer.svelte';
	import { cursors } from '$lib/viz/cursor';

	/** A channel table's views, as VD's drop-down names them. */
	const TABLE_VIEWS: Record<string, string> = { channels: 'Channels', response: 'Response transformation', reference: 'Reference transformation' };

	/** Where a shape set's values are read, as VD names the choices. */
	const VALUES_CHOICES: Record<string, string> = {
		all: 'All DOFs',
		control: 'Control channels',
		reference: 'References',
		response: 'Responses',
		monitor: 'Monitors'
	};

	interface Props {
		app: AppState;
		/** The module shown: its operations, and those every module offers. */
		only: Workspace;
	}
	let { app, only }: Props = $props();

	const a = $derived(app.analyze);
	const tools = $derived(a.toolsFor(only));
	const datasets = $derived(a.datasetsFor(only));
	const inModule = (e: { workspace: Workspace; everywhere?: boolean }) => offeredIn(e, only);

	// a module works on the project when one is open and nothing else is, and on one of its own tools
	$effect(() => {
		if (app.project.summary && !a.opened && !a.opening) a.selectProject();
		if (a.onProject && tools.length && !tools.some((o) => o.id === a.opId)) a.selectOp(tools[0].id);
	});

	// the project's FRFs a fit can start on, and the coherences that can weight it
	const objects = $derived(app.project.summary?.objects ?? []);
	const coherences = $derived(objects.filter((o) => o.kind === 'coherence' || o.kind === 'multiple_coherence').map((o) => o.name));
	let weight = $state('');
	$effect(() => {
		if (weight && !coherences.includes(weight)) weight = '';
	});

	// a settings form opens on what its source object holds, and follows it when the source changes
	$effect(() => a.followSource());

	// a stage that reads at a line follows the frequency cursor of the plot beside it
	$effect(() => {
		const follows = a.shown?.views.some((v) => v.view === 'shape3d' && v.cursor === 'frequency');
		if (!follows) return;
		return cursors.subscribe('frequency', (state) => a.moveLine(state.x));
	});

	/**
	 * The cell chosen in each result's MAC, by result id. A matrix that `selects` the result's
	 * shape view shows that row's mode with that column's overlaid; the first cell to begin.
	 */
	let pairs = $state<Record<number, { row: number; column: number }>>({});
	const pairOf = (r: AnalyzeResult) => pairs[r.id] ?? { row: 0, column: 0 };
	const linked = (r: AnalyzeResult) => r.views.some((v) => v.view === 'matrix' && v.selects === 'shape3d');
	/**
	 * The one result filling the results area, by id, the others kept as they were behind it. A
	 * result removed lets go of it, and a new one does too, so it is not run out of sight.
	 */
	let maximized = $state<number | null>(null);
	const big = $derived(a.results.some((r) => r.id === maximized) ? maximized : null);
	let newest = 0;
	$effect(() => {
		const top = Math.max(0, ...a.results.map((r) => r.id));
		if (top > newest) maximized = null;
		newest = top;
	});

	const isMac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);

	function onKey(e: KeyboardEvent) {
		if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
			e.preventDefault();
			void a.run();
		} else if (e.key === 'Escape' && big !== null && !(e.target as HTMLElement | null)?.closest?.('input, textarea, select')) {
			maximized = null;
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
		<ProjectPanel {app} />
		{#if !app.catalog}
			<section class="block">
				<h2 class="eyebrow">Dataset</h2>
				<p class="muted">
					{#if app.catalogError}{app.catalogError}{:else if app.analysis.isOpen}Loading the catalog.{:else}Waiting for the analysis service at <span class="mono">{app.analysis.url}</span>.{/if}
				</p>
			</section>
		{:else if datasets.length}
			<section class="block">
				<h2 class="eyebrow">Dataset</h2>
				<div class="choices" role="radiogroup" aria-label="Dataset" data-testid="dataset-picker">
					{#each datasets as ds (ds.id)}
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
				{#if a.opening}<p class="muted small">Opening.</p>{/if}
				{#if a.openError}<p class="error" role="alert">{a.openError}</p>{/if}
				{#if a.opened && !a.onProject}
					<dl class="chips">
						{#each summaryChips(a.opened.summary) as c (c.label)}
							<div><dt>{c.label}</dt><dd class="num">{c.value}</dd></div>
						{/each}
						<div><dt>Channels</dt><dd class="num">{a.opened.channels.length}</dd></div>
					</dl>
				{/if}
			</section>
		{/if}

		{#if a.opened && a.onProject}
			{@const picked = app.project.selection.objects}
			{@const kinds = picked.map((n) => app.project.summary?.objects.find((o) => o.name === n)?.kind)}
			{@const fitPair = picked.length === 2 && kinds.includes('frf') && kinds.includes('shape_set')}
			{@const fitAlone = only === 'modal' && picked.length === 1 && kinds[0] === 'frf'}
			<section class="block" data-testid="actions-frame">
				<h2 class="eyebrow">
					{#if picked.length}Actions <span class="for">for {picked.length === 1 ? picked[0] : `${picked.length} objects`}</span>{:else}Actions <span class="for">for the project</span>{/if}
				</h2>
				{#if !a.actions}
					<p class="muted small">Asking what the selection can take.</p>
				{:else}
					{@const readings = a.actions.readings.filter(inModule)}
					{@const acts = a.actions.acts.filter(inModule)}
					{@const whole = a.actions.project.filter(inModule)}
					{#if fitPair || fitAlone}
						<div class="tools" role="group" aria-label="Fit" data-testid="fit-start">
							{#if fitPair}
								<button class="tool" data-testid="action-edit-fit" onclick={() => { app.go('modal'); void app.fit.start(picked[kinds.indexOf('frf')], picked[kinds.indexOf('shape_set')], weight || undefined); }}>
									<Icon name="tool" size={15} />
									<span class="ttitle">Edit Fit</span>
									<span class="tout">modal</span>
								</button>
							{:else}
								<button class="tool" data-testid="fit-begin" onclick={() => { void app.fit.start(picked[0], undefined, weight || undefined); }} disabled={app.fit.busy !== null}>
									<Icon name="tool" size={15} />
									<span class="ttitle">Fit Modes</span>
									<span class="tout">by hand</span>
								</button>
							{/if}
							{#if coherences.length}
								<select class="field sm" aria-label="Weight the fit by" data-testid="fit-weight" bind:value={weight} title="Weight each response at each line by its coherence, γ²/(1-γ²)">
									<option value="">No weighting</option>
									{#each coherences as name (name)}<option value={name}>Weight by {name}</option>{/each}
								</select>
							{/if}
							{#if app.fit.error && !app.fit.state}<p class="error" role="alert">{app.fit.error}</p>{/if}
						</div>
					{/if}
					{#if readings.length}
						<h3 class="group">Readings</h3>
						<div class="tools" role="radiogroup" aria-label="Readings" data-testid="action-readings">
							{#each readings as e (e.id)}
								<button class="tool" data-testid="action-{e.id}" role="radio" aria-checked={a.opId === e.id} class:on={a.opId === e.id} onclick={() => a.pickAction(e)}>
									<Icon name="tool" size={15} />
									<span class="ttitle">{e.title}</span>
								</button>
							{/each}
						</div>
					{/if}
					{#if acts.length}
						<h3 class="group">Acts</h3>
						<div class="tools" role="radiogroup" aria-label="Acts" data-testid="action-acts">
							{#each acts as e (e.id)}
								<button class="tool" data-testid="action-{e.id}" role="radio" aria-checked={a.opId === e.id} class:on={a.opId === e.id} onclick={() => a.pickAction(e)}>
									<Icon name="action" size={15} />
									<span class="ttitle">{e.title}</span>
								</button>
							{/each}
						</div>
					{/if}
					{#if whole.length}
						<div class="tools" role="radiogroup" aria-label="Project acts" data-testid="action-project">
							{#each whole as e (e.id)}
								<button class="tool" data-testid="action-{e.id}" role="radio" aria-checked={a.opId === e.id} class:on={a.opId === e.id} onclick={() => a.pickAction(e)}>
									<Icon name="action" size={15} />
									<span class="ttitle">{e.title}</span>
								</button>
							{/each}
						</div>
					{/if}
					{#if !readings.length && !acts.length && !whole.length}
						<p class="muted small">{picked.length ? 'Nothing takes this selection together. Select less, or something else.' : 'Select an object in the tree to see what it can do.'}</p>
					{/if}
				{/if}
			</section>
		{:else if a.opened}
			<section class="block">
				<h2 class="eyebrow">Tools <span class="for">for {a.opened.kind.replace('_', ' ')} data</span></h2>
				<div class="tools" role="radiogroup" aria-label="Tool" data-testid="tool-list">
					{#each tools as op (op.id)}
						<button class="tool" data-testid="tool-{op.id}" role="radio" aria-checked={a.opId === op.id} class:on={a.opId === op.id} onclick={() => a.selectOp(op.id)}>
							<Icon name="tool" size={15} />
							<span class="ttitle">{op.title}</span>
							<span class="tout">{op.outputs.join(', ')}</span>
						</button>
					{/each}
				</div>
			</section>
		{/if}

		{#if a.op && a.opened && (a.onProject || tools.includes(a.op))}
			<section class="block panel" aria-labelledby="op-title">
				<div class="phead">
					<h2 id="op-title">{a.op.title}</h2>
					<button class="btn ghost sm" onclick={() => a.resetParams()} title="Reset every field to its default">Defaults</button>
				</div>
				{#if a.op.summary}<p class="psum">{a.op.summary}</p>{/if}
				<ParamForm
					params={a.params.filter((p) => !p.in_view)}
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

		{#if a.reading}
			<section class="selected" class:alone={a.results.length === 0} data-testid="reading-view" aria-label="The reading">
				<header>
					<h3>{a.reading.title}</h3>
					<p class="meta">{a.shown?.title ?? ''}</p>
					<button class="btn ghost sm" data-testid="reading-back" onclick={() => a.endReading()}>Back</button>
				</header>
				<div class="svviews" class:pair={a.reading.views.length === 2}>
					{#each a.reading.views as v, i (i)}
						<div class="view {v.view}">
							<ViewRenderer view={v} layout="strip" showTitle={v.view === 'spectrum'} minHeight={240} scope="reading" onparams={(patch) => a.rerunReading(patch)} />
						</div>
					{/each}
				</div>
			</section>
		{:else if a.shown || a.showError}
			<section class="selected" class:alone={a.results.length === 0} data-testid="selection-view" aria-label="What the selection shows">
				<header>
					<h3>{a.shown?.title ?? 'Selection'}</h3>
					{#if a.shown?.note}<p class="meta" data-testid="selection-note">{a.shown.note}</p>{/if}
					{#if a.showError}<p class="meta error" role="alert">{a.showError}</p>{/if}
					{#if a.shown?.offers}
						{@const offers = a.shown.offers}
						<div class="bar" role="toolbar" aria-label="How the data is read">
							{#if offers.flat}
								<div class="seg" role="radiogroup" aria-label="Stage or plot">
									<button role="radio" aria-checked={!a.prefs.flat} class:on={!a.prefs.flat} data-testid="read-3d" title="The 3-D stage, every record at its own depth" onclick={() => a.prefs.flat && a.setPref('flat', false)}>3D</button>
									<button role="radio" aria-checked={a.prefs.flat} class:on={a.prefs.flat} data-testid="read-flat" title="The flat plot, every record as a curve" onclick={() => !a.prefs.flat && a.setPref('flat', true)}>Flat</button>
								</div>
							{/if}
							{#if offers.frequency}
								<button class="btn ghost sm" class:on={a.prefs.log_frequency} aria-pressed={a.prefs.log_frequency} data-testid="read-logf" title="Decades on every frequency axis" onclick={() => a.setPref('log_frequency', !a.prefs.log_frequency)}>Log f</button>
							{/if}
							{#if offers.complex}
								<select class="field sm" aria-label="Component" data-testid="read-component" value={a.prefs.component} onchange={(e) => a.setPref('component', (e.currentTarget as HTMLSelectElement).value as typeof a.prefs.component)}>
									<option value="magnitude">Magnitude</option>
									<option value="real">Real part</option>
									<option value="imag">Imaginary part</option>
									<option value="phase">Phase</option>
								</select>
							{/if}
							{#if a.shown?.views.some((v) => v.view === 'matrix')}
								<div class="seg" role="radiogroup" aria-label="Bars or grid">
									<button role="radio" aria-checked={a.prefs.bars} class:on={a.prefs.bars} data-testid="read-bars" title="The MAC as 3-D bars, height and color the value" onclick={() => !a.prefs.bars && a.setPref('bars', true)}>Bars</button>
									<button role="radio" aria-checked={!a.prefs.bars} class:on={!a.prefs.bars} data-testid="read-grid" title="The MAC as a flat grid" onclick={() => a.prefs.bars && a.setPref('bars', false)}>Grid</button>
								</div>
							{/if}
							{#if a.shown?.views.some((v) => v.view === 'shape3d' && v.overlays?.length)}
								<button class="btn ghost sm" class:on={a.prefs.overlay} aria-pressed={a.prefs.overlay} data-testid="read-overlay" title="Animate the picked pair of modes over each other, the second phase-aligned to the first" onclick={() => a.setPref('overlay', !a.prefs.overlay)}>Overlay</button>
							{/if}
							{#if offers.projection}
								<button class="btn ghost sm" class:on={a.prefs.projection} aria-pressed={a.prefs.projection} data-testid="read-projection" title="Animate the other set projected onto the basis geometry, instead of both by DOF name" onclick={() => a.setPref('projection', !a.prefs.projection)}>Projection</button>
							{/if}
							{#if offers.shapes}
								{@const up = [a.prefs.mode_table, a.prefs.mac, a.prefs.shape_values].filter(Boolean).length}
								<button class="btn ghost sm" class:on={a.prefs.mode_table} aria-pressed={a.prefs.mode_table} disabled={up === 1 && a.prefs.mode_table} data-testid="read-mode-table" title="The mode list: frequency, damping and a description to fill in" onclick={() => a.setPref('mode_table', !a.prefs.mode_table)}>Mode Table</button>
								<button class="btn ghost sm" class:on={a.prefs.mac} aria-pressed={a.prefs.mac} disabled={up === 1 && a.prefs.mac} data-testid="read-mac" title="The MAC of the set against itself: how distinct its modes are" onclick={() => a.setPref('mac', !a.prefs.mac)}>MAC</button>
								<button class="btn ghost sm" class:on={a.prefs.shape_values} aria-pressed={a.prefs.shape_values} disabled={up === 1 && a.prefs.shape_values} data-testid="read-shape-values" title="The shape values: a row per DOF and a column per mode" onclick={() => a.setPref('shape_values', !a.prefs.shape_values)}>Shape Values</button>
								{#if a.prefs.shape_values}
									{#if offers.values_dofs.length > 1}
										<select class="field sm" aria-label="Where the values are read" data-testid="read-values-dofs" value={a.prefs.values_dofs} onchange={(e) => a.setPref('values_dofs', (e.currentTarget as HTMLSelectElement).value)} title="Where the values are read: every DOF of the set, or a choice of the channel table in its group, signs as the table writes them">
											{#each offers.values_dofs as choice (choice)}
												<option value={choice}>{VALUES_CHOICES[choice] ?? choice}</option>
											{/each}
										</select>
									{/if}
									<select class="field sm" aria-label="Form" data-testid="read-values-form" value={a.prefs.values_form} onchange={(e) => a.setPref('values_form', (e.currentTarget as HTMLSelectElement).value as typeof a.prefs.values_form)} title="The shapes at the DOFs (u = Φq), or a transformation there: the response transformation Φ⁺ (q = Φ⁺u), applied to motions, or the reference transformation Φᵀ, applied to forces and drive signals">
										<option value="shapes">Shapes Φ</option>
										<option value="response">Response transformation Φ⁺</option>
										<option value="reference">Reference transformation Φᵀ</option>
									</select>
								{/if}
							{/if}
							{#if offers.environments.length > 1}
								<select class="field sm" aria-label="Environment" data-testid="read-environment" value={a.prefs.environment ?? offers.environments[0]} onchange={(e) => a.setPref('environment', (e.currentTarget as HTMLSelectElement).value)} title="Which environment's control channels the Control column shows, and whose transformation the view drop-down offers">
									{#each offers.environments as loop (loop)}<option value={loop}>{loop}</option>{/each}
								</select>
							{/if}
							{#if offers.table_views.length > 1}
								<select class="field sm" aria-label="Table view" data-testid="read-table-view" value={a.prefs.table_view} onchange={(e) => a.setPref('table_view', (e.currentTarget as HTMLSelectElement).value as typeof a.prefs.table_view)} title="The channels, or what the controller put over them: the response transformation, a row per controlled response over the control channels, or the reference transformation over the drives">
									{#each offers.table_views as form (form)}<option value={form}>{TABLE_VIEWS[form] ?? form}</option>{/each}
								</select>
							{/if}
							{#if offers.arrows}
								<button class="btn ghost sm" class:on={a.prefs.arrows} aria-pressed={a.prefs.arrows} data-testid="read-arrows" title="Mark the measurement DOFs with arrows over the stage; a force points in" onclick={() => a.setPref('arrows', !a.prefs.arrows)}>DOF Arrows</button>
							{/if}
							{#if offers.sensors}
								<button class="btn ghost sm" class:on={a.prefs.sensors} aria-pressed={a.prefs.sensors} data-testid="read-sensors" title="Draw the accelerometers and force transducers the channel table names, centered on their nodes" onclick={() => a.setPref('sensors', !a.prefs.sensors)}>Sensors</button>
							{/if}
							{#if a.comparison && a.picks.length}
								<button class="btn sm" data-testid="add-matches" title="Add the picked MAC squares to the matched modes for this pair of sets; created on first use" onclick={() => void a.addMatches()}>Add Matches</button>
							{/if}
						</div>
					{/if}
					{#if app.project.selection.objects.length === 1 && app.project.summary?.objects.find((o) => o.name === app.project.selection.objects[0])?.kind === 'report'}
						<button class="btn sm" data-testid="open-report" onclick={() => { const name = app.project.selection.objects[0]; app.go('report'); void app.report.show(name); }}>Open in Report</button>
					{/if}
					<button class="btn ghost icon sm" data-testid="selection-clear" onclick={() => app.project.clearSelection()} aria-label="Clear the selection" title="Clear the selection">
						<Icon name="close" size={14} />
					</button>
				</header>
				{#if a.shown}
					{@const linked = a.shown.views.some((v) => v.view === 'matrix' && v.selects === 'shape3d')}
					<div class="svviews" class:pair={a.shown.views.length === 2}>
						{#each a.shown.views as v, i (i)}
							<div class="view {v.view}">
								<ViewRenderer
									view={v}
									layout="strip"
									showTitle={v.view === 'spectrum'}
									minHeight={240}
									scope="selection"
									selected={v.view === 'matrix' && v.selects ? a.pair : null}
									picks={v.view === 'matrix' && v.selects ? a.picks : []}
									onselect={v.view === 'matrix' && v.selects ? (cell, extend) => a.pickCell(cell, extend) : undefined}
									bars={v.view === 'matrix' && a.prefs.bars}
									mode={v.view === 'shape3d' && linked && a.pair ? a.pair.row : 0}
									overlay={v.view === 'shape3d' && linked && a.prefs.overlay && a.pair ? a.pair.column : null}
									onedit={v.view === 'table' && v.edit ? (row, column, value) => a.editShownCell(i, v.edit!, row, column, value) : undefined}
								/>
							</div>
						{/each}
					</div>
				{/if}
			</section>
		{/if}
		{#if a.results.length === 0 && !a.shown && !a.showError && !a.reading}
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
			<div class="grid" class:one={big !== null}>
				{#each a.results as r (r.id)}
					<article class="card" class:big={r.id === big} class:tucked={big !== null && r.id !== big} data-testid="result-card" data-op={r.opId} aria-label="{r.opTitle} result">
						<header>
							<div class="ctext">
								<h3>{r.opTitle}</h3>
								<p class="meta">{r.datasetTitle}</p>
								<p class="meta params">{r.paramSummary}</p>
							</div>
							<span class="at num">{time(r.at)}</span>
							<button class="btn ghost icon sm" data-testid="result-maximize" onclick={() => (maximized = r.id === big ? null : r.id)}
								aria-pressed={r.id === big} aria-label={r.id === big ? 'Back to all the results' : 'Fill the results area with this result'}
								title={r.id === big ? 'Back to all the results (Esc)' : 'Fill the results area'}>
								<Icon name={r.id === big ? 'restore' : 'maximize'} size={14} />
							</button>
							<button class="btn ghost icon sm" data-testid="result-remove" onclick={() => a.removeResult(r.id)} aria-label="Remove this result" title="Remove">
								<Icon name="close" size={14} />
							</button>
						</header>
						<div class="views">
							{#each r.views as v, i (i)}
								<div class="view {v.view}">
									<ViewRenderer
										view={v}
										layout="strip"
										showTitle={v.view === 'spectrum'}
										minHeight={240}
										onparams={(patch) => a.rerun(r.id, patch)}
										selected={v.view === 'matrix' && v.selects ? pairOf(r) : null}
										onselect={v.view === 'matrix' && v.selects ? (cell) => (pairs[r.id] = cell) : undefined}
										mode={v.view === 'shape3d' && linked(r) ? pairOf(r).row : 0}
										overlay={v.view === 'shape3d' && linked(r) ? pairOf(r).column : null}
										scope={String(r.id)}
										onedit={v.view === 'table' && v.edit ? (row, column, value) => a.editCell(r.id, i, v.edit!, row, column, value) : undefined}
										onspan={(v.view === 'spectrum' || v.view === 'waterfall') && v.regions?.some((g) => g.drag) ? (k, x0, x1) => a.dragSpan(r.id, v.regions![k].drag!, x0, x1) : undefined}
										onpick={v.view === 'shape3d' && v.pick ? (patch) => a.pickIn(r.id, v.pick!, patch) : undefined}
									/>
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
	.group {
		margin: var(--sp-2) 0 2px;
		font-size: var(--text-2xs);
		font-weight: var(--weight-medium);
		letter-spacing: 0.04em;
		text-transform: uppercase;
		color: var(--text-4);
	}

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

	.grid.one {
		flex: 1;
		min-height: 0;
		grid-template-columns: minmax(0, 1fr);
		grid-auto-rows: minmax(0, 1fr);
		align-items: stretch;
	}

	.card.tucked {
		display: none;
	}

	.card.big {
		min-height: 0;
	}

	.card.big .views {
		flex: 1;
		min-height: 0;
		overflow-y: auto;
	}

	/* what can draw at any size takes the room; a table or a reading keeps its own height */
	.card.big .view {
		flex: none;
	}

	.card.big .view.spectrum,
	.card.big .view.waterfall,
	.card.big .view.shape3d,
	.card.big .view.matrix {
		flex: 1 1 0;
		height: auto;
		min-height: 320px;
	}

	/* the selection's views: what the tree's selection shows, filling the width; a pair side by side */
	.selected {
		display: flex;
		flex-direction: column;
		margin-bottom: var(--sp-4);
		background: var(--bg-1);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-lg);
	}

	.selected.alone {
		flex: 1;
		min-height: 0;
	}

	.selected header {
		display: flex;
		align-items: center;
		gap: var(--sp-3);
		padding: var(--sp-3) var(--sp-3) var(--sp-2) var(--sp-4);
		border-bottom: 1px solid var(--line-1);
	}

	.selected h3 {
		font-size: var(--text-md);
		font-weight: var(--weight-strong);
		white-space: nowrap;
	}

	.selected header .meta {
		flex: 1;
		min-width: 0;
	}

	.selected header .error {
		color: var(--amber-2);
	}

	.selected header .bar {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2);
	}

	.seg {
		display: inline-flex;
		border: 1px solid var(--line-2);
		border-radius: var(--radius-sm);
		overflow: hidden;
	}

	.seg button {
		padding: 2px 8px;
		font-size: var(--text-xs);
		color: var(--text-2);
		background: transparent;
		border: none;
		cursor: pointer;
	}

	.seg button.on {
		color: var(--text-1);
		background: var(--bg-3);
	}

	.selected header .btn.on {
		color: var(--text-1);
		background: var(--bg-3);
	}

	.field.sm {
		padding: 2px 6px;
		font-size: var(--text-xs);
	}

	.svviews {
		display: grid;
		grid-template-columns: minmax(0, 1fr);
		gap: var(--sp-3);
		padding: var(--sp-3);
		min-height: 0;
	}

	.svviews.pair {
		grid-template-columns: repeat(auto-fit, minmax(min(100%, 420px), 1fr));
	}

	.selected.alone .svviews {
		flex: 1;
		min-height: 0;
		align-content: start;
	}

	.selected .view.spectrum {
		height: 340px;
	}

	.selected .view.matrix {
		height: 420px;
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

	/* the 3-D views grow with the window; a plot keeps its height, which suits a column of them */
	.view.waterfall {
		height: clamp(480px, 64vh, 960px);
	}

	.view.shape3d {
		height: clamp(440px, 64vh, 960px);
	}

	.view.matrix {
		height: 420px;
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
