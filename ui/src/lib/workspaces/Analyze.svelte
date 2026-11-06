<script lang="ts">
	/**
	 * The study host every analysis module shares: the project tree, what the selection shows, the
	 * Actions frame and the tool panel. A module (`only`) differs in which operations its Actions
	 * frame and tool list offer, its own and those offered everywhere. An act's result joins the
	 * tree; a reading takes the view until Back.
	 */
	import type { AppState } from '$lib/app/state.svelte';
	import type { View, Workspace } from '$lib/protocol/types';
	import ProjectPanel from '$lib/project/ProjectPanel.svelte';
	import ActionsFrame from './ActionsFrame.svelte';
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

	/** A lone shape set's panes, each on a preference the service reads: off, the pane is put away and its view not computed. */
	const PANES = [
		['animation', 'Animation'],
		['mode_table', 'Mode Table'],
		['mac', 'MAC'],
		['shape_values', 'Shape Values']
	] as const;
	/** The preference a pane's view sits on: the mode table is the first table when it is up, the values table the one after it. */
	function paneOf(v: View, views: View[]): (typeof PANES)[number][0] {
		if (v.view === 'shape3d') return 'animation';
		if (v.view === 'matrix') return 'mac';
		return a.prefs.mode_table && v === views.find((w) => w.view === 'table') ? 'mode_table' : 'shape_values';
	}
	const tools = $derived(a.toolsFor(only));

	// a module works on one of its own tools
	$effect(() => {
		if (a.onProject && tools.length && !tools.some((o) => o.id === a.opId)) a.selectOp(tools[0].id);
	});


	// a settings form opens on what its source object holds, and follows it when the source changes
	$effect(() => a.followSource());

	// a stage that reads at a line follows the frequency cursor of the plot beside it
	$effect(() => {
		const follows = a.shown?.views.some((v) => v.view === 'shape3d' && v.cursor === 'frequency');
		if (!follows) return;
		return cursors.subscribe('frequency', (state) => a.moveLine(state.x));
	});

	const isMac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);

	function onKey(e: KeyboardEvent) {
		if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
			e.preventDefault();
			void a.run();
		}
	}

</script>

<svelte:window onkeydown={onKey} />

<div class="analyze">
	<aside class="side" aria-label="Dataset and tools">
		<ProjectPanel {app} />
		{#if !app.catalog}
			<section class="block">
				<h2 class="eyebrow">Analysis</h2>
				<p class="muted">
					{#if app.catalogError}{app.catalogError}{:else if app.analysis.isOpen}Loading the catalog.{:else}Waiting for the analysis service at <span class="mono">{app.analysis.url}</span>.{/if}
				</p>
			</section>
		{/if}

		<ActionsFrame {app} {only} />
	</aside>

	<main class="shown" aria-label="What the selection shows">
		{#if a.reading}
			<section class="selected" data-testid="reading-view" aria-label="The reading">
				<header>
					<h3>{a.reading.title}</h3>
					<p class="meta">{a.shown?.title ?? ''}</p>
					<button class="btn ghost sm" data-testid="reading-back" onclick={() => a.endReading()}>Back</button>
				</header>
				<div class="svviews" class:several={a.reading.views.length > 1}>
					{#each a.reading.views as v, i (i)}
						<div class="view {v.view}">
							<ViewRenderer view={v} layout="strip" showTitle={v.view === 'spectrum' || v.view === 'table'} minHeight={240} scope="reading" onparams={(patch) => a.rerunReading(patch)} />
						</div>
					{/each}
				</div>
			</section>
		{:else if a.shown || a.showError}
			<section class="selected" data-testid="selection-view" aria-label="What the selection shows">
				<header>
					<h3>{a.shown?.title ?? 'Selection'}</h3>
					{#if a.shown?.note}<p class="meta" data-testid="selection-note">{a.shown.note}</p>{/if}
					{#if a.showError}<p class="meta error" role="alert">{a.showError}</p>{/if}
					{#if a.shown?.offers}
						{@const offers = a.shown.offers}
						{#if offers.shapes}
							{@const away = PANES.filter(([pref]) => !a.prefs[pref])}
							{#if away.length}
								<div class="away" role="toolbar" aria-label="Panes put away">
									{#each away as [pref, label] (pref)}
										<button class="btn ghost sm chip" data-testid="restore-{pref}" title="Bring the {label} pane back" onclick={() => a.setPref(pref, true)}><Icon name="chevron" size={12} />{label}</button>
									{/each}
								</div>
							{/if}
						{/if}
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
							{#if a.shown?.views.some((v) => v.view === 'shape3d' && v.overlays?.length)}
								<button class="btn ghost sm" class:on={a.prefs.overlay} aria-pressed={a.prefs.overlay} data-testid="read-overlay" title="Animate the picked pair of modes over each other, the second phase-aligned to the first" onclick={() => a.setPref('overlay', !a.prefs.overlay)}>Overlay</button>
							{/if}
							{#if offers.projection}
								<button class="btn ghost sm" class:on={a.prefs.projection} aria-pressed={a.prefs.projection} data-testid="read-projection" title="Animate the other set projected onto the basis geometry, instead of both by DOF name" onclick={() => a.setPref('projection', !a.prefs.projection)}>Projection</button>
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
							{#if offers.tones.length > 1}
								<select class="field sm" aria-label="Tone" data-testid="read-tone" value={offers.tones.includes(a.prefs.tone ?? '') ? a.prefs.tone : offers.tones[0]} onchange={(e) => a.setPref('tone', (e.currentTarget as HTMLSelectElement).value)} title="Which tone of the sweep is read">
									{#each offers.tones as tone (tone)}<option value={tone}>{tone}</option>{/each}
								</select>
							{/if}
							{#if offers.sine_readings}
								<div class="seg" role="radiogroup" aria-label="Levels or error">
									<button role="radio" aria-checked={a.prefs.sine_reading === 'curves'} class:on={a.prefs.sine_reading === 'curves'} data-testid="read-sine-curves" title="The extracted levels over the tone's requirement and limits" onclick={() => a.prefs.sine_reading !== 'curves' && a.setPref('sine_reading', 'curves')}>Levels</button>
									<button role="radio" aria-checked={a.prefs.sine_reading === 'bars'} class:on={a.prefs.sine_reading === 'bars'} data-testid="read-sine-bars" title="Each channel's signed RMS deviation from its tone across the sweep, in dB" onclick={() => a.prefs.sine_reading !== 'bars' && a.setPref('sine_reading', 'bars')}>Level Error</button>
								</div>
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
					{@const selfMac = a.shown.views.some((v) => v.view === 'matrix' && v.selects === 'shape3d' && v.rows.label === v.columns.label)}
					{@const offers = a.shown.offers}
					{@const shapes = !!offers?.shapes}
					{@const views = a.shown.views}
					{@const beside = views.filter((v) => v.view !== 'shape3d')}
					<!-- every view is a pane that fills the frame, unless it is a table, which takes its rows; stacked on a narrow frame each keeps a working height and the frame scrolls -->
					<div class="svviews panes" class:several={views.length > 1}
						style:--rows={Math.max(1, beside.length)}
						style:--rows-wide={beside.length ? beside.map((v) => (v.view === 'table' ? 'auto' : 'minmax(0, 1fr)')).join(' ') : 'minmax(0, 1fr)'}
						style:--rows-stacked={views.map((v) => (v.view === 'table' ? 'auto' : v.view === 'shape3d' ? 'minmax(440px, 1fr)' : 'minmax(360px, 1fr)')).join(' ')}>
						{#each views as v, i (i)}
							{@const headed = views.length > 1 || shapes || v.view === 'matrix' || (v.view === 'shape3d' && !!(offers?.arrows || offers?.sensors))}
							<div class="view pane {v.view}" data-testid="pane-{shapes ? paneOf(v, views) : v.view}">
								{#if headed && offers}
									<!-- the pane's own header: its title and the controls that read this view, as VD's scene and data panes carry their own; a lone view with nothing to set leaves its title to the frame -->
									<header class="phead">
										<h4>{v.title}</h4>
										{#if v.view === 'shape3d'}
											{#if offers.arrows}
												<button class="btn ghost sm" class:on={a.prefs.arrows} aria-pressed={a.prefs.arrows} data-testid="read-arrows" title="Mark the measurement DOFs with arrows over the stage; a force points in" onclick={() => a.setPref('arrows', !a.prefs.arrows)}>DOF Arrows</button>
											{/if}
											{#if offers.sensors}
												<button class="btn ghost sm" class:on={a.prefs.sensors} aria-pressed={a.prefs.sensors} data-testid="read-sensors" title="Draw the accelerometers and force transducers the channel table names, centered on their nodes" onclick={() => a.setPref('sensors', !a.prefs.sensors)}>Sensors</button>
											{/if}
										{:else if v.view === 'matrix'}
											<div class="seg" role="radiogroup" aria-label="Bars or grid">
												<button role="radio" aria-checked={a.prefs.bars} class:on={a.prefs.bars} data-testid="read-bars" title="The MAC as 3-D bars, height and color the value" onclick={() => !a.prefs.bars && a.setPref('bars', true)}>Bars</button>
												<button role="radio" aria-checked={!a.prefs.bars} class:on={!a.prefs.bars} data-testid="read-grid" title="The MAC as a flat grid" onclick={() => a.prefs.bars && a.setPref('bars', false)}>Grid</button>
											</div>
										{:else if shapes && v.view === 'table' && paneOf(v, views) === 'shape_values'}
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
										{#if shapes}
											<button class="btn ghost icon sm" data-testid="minimize-{paneOf(v, views)}" aria-label="Put this pane away" title="Put this pane away; its name in the frame's header brings it back" disabled={views.length === 1} onclick={() => a.setPref(paneOf(v, views), false)}><Icon name="chevron" size={12} /></button>
										{/if}
									</header>
								{/if}
								<div class="pbody">
									<ViewRenderer
										view={v}
										layout="strip"
										showTitle={!headed && (v.view === 'spectrum' || v.view === 'table')}
										minHeight={240}
										scope="selection"
										selected={v.view === 'matrix' && v.selects ? a.pair : null}
										picks={v.view === 'matrix' && v.selects ? a.picks : []}
										onselect={v.view === 'matrix' && v.selects ? (cell, extend) => a.pickCell(cell, extend) : undefined}
										bars={v.view === 'matrix' && a.prefs.bars}
										mode={v.view === 'shape3d' && linked && a.pair ? a.pair.row : 0}
										overlay={v.view === 'shape3d' && linked && a.prefs.overlay && a.pair && !(selfMac && a.pair.row === a.pair.column) ? a.pair.column : null}
										onedit={v.view === 'table' && v.edit ? (row, column, value) => a.editShownCell(i, v.edit!, row, column, value) : undefined}
									/>
								</div>
							</div>
						{/each}
					</div>
				{/if}
			</section>
		{/if}
		{#if !a.shown && !a.showError && !a.reading}
			<div class="empty">
				<div class="frame" aria-hidden="true">
					<svg viewBox="0 0 220 90" width="220" height="90">
						<path d="M10 80V10M10 80h200" stroke="var(--line-3)" fill="none" />
						<path d="M14 66c20-2 26-34 46-34h70c20 0 40 20 76 26" stroke="var(--series-reference)" stroke-dasharray="7 4" fill="none" opacity="0.6" />
						<path d="M14 70c18-4 26-30 46-36 8 2 12-4 20 0 10 6 16-10 26-2 8 6 16-4 24 0 14 4 30 18 70 26" stroke="var(--series-measured)" fill="none" stroke-width="1.6" />
					</svg>
				</div>
				<h3>Nothing Selected</h3>
				<p>
					{#if !a.onProject}Open a project or import a file; in the desktop app, drop it on the window. Example projects are on hertzforge.com.{:else}Select something in the project tree to see it here, or run a tool.{/if}
				</p>
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

	.shown {
		display: flex;
		flex-direction: column;
		min-height: 0;
		min-width: 0;
		overflow-y: auto;
		padding: var(--sp-4) var(--sp-5) var(--sp-6);
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

	/* the selection's views: what the tree's selection shows, filling the width. Several share rows two
	   abreast, as VD's data pane puts a shape set's mode table beside its MAC. A 3-D view stands apart
	   from the rest as VD's scene pane stands apart from its data pane: on a wide frame it takes the
	   left three fifths with the other views stacked beside it, on a narrow one a row of its own. */
	.selected {
		display: flex;
		flex: 1;
		flex-direction: column;
		min-height: 0;
		margin-bottom: var(--sp-4);
		background: var(--bg-1);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-lg);
		container-type: size;
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

	.svviews.several {
		grid-template-columns: repeat(auto-fit, minmax(min(100%, 440px), 1fr));
	}

	.svviews.several .view.shape3d {
		grid-column: 1 / -1;
	}

	@container (min-width: 900px) {
		.svviews.several:has(.view.shape3d) {
			grid-template-columns: minmax(0, 3fr) minmax(0, 2fr);
			grid-auto-rows: minmax(0, auto);
		}

		.svviews.several .view.shape3d {
			grid-column: 1;
			grid-row: 1 / span 2;
			min-height: 520px;
		}

		/* the panes share the frame's height instead of growing past it: the 3-D pane down the left, the rest in rows beside it; without one, the rows share the height */
		.svviews.panes.several:has(.view.shape3d) {
			grid-template-rows: var(--rows-wide);
			overflow: hidden;
		}

		.svviews.panes.several:not(:has(.view.shape3d)) {
			grid-template-rows: none;
			grid-auto-rows: minmax(0, 1fr);
			overflow: hidden;
		}

		.svviews.panes.several .view.shape3d {
			grid-row: 1 / span var(--rows);
		}
	}

	/* a pane's track sizes it: a 3-D view or a matrix fills what the frame gives, a table takes its rows */
	.svviews.panes {
		grid-template-rows: var(--rows-stacked);
		align-content: stretch;
		overflow-y: auto;
	}

	.svviews.panes .view,
	.svviews.panes .view.shape3d,
	.svviews.panes .view.matrix,
	.svviews.panes .view.table,
	.svviews.panes .view.spectrum,
	.svviews.panes .view.waterfall {
		height: auto;
		min-height: 0;
	}

	/* a table pane scrolls inside itself once it is taller than the frame, instead of growing past it */
	.pane.table :global(.scroll) {
		max-height: calc(100cqh - 150px);
	}

	.away {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-2);
	}

	.chip {
		display: inline-flex;
		gap: 4px;
		align-items: center;
		border: 1px dashed var(--line-2);
	}

	.chip :global(svg) {
		transform: rotate(180deg);
	}

	.svviews {
		flex: 1;
		min-height: 0;
		align-content: start;
	}

	.view {
		display: flex;
		flex-direction: column;
		min-height: 0;
	}

	/* the view fills what its pane leaves; a basis of auto keeps a pane with no set height at its content's */
	.pbody {
		display: grid;
		flex: 1 1 auto;
		min-height: 0;
	}

	.pane {
		overflow: hidden;
		background: var(--bg-0);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-md);
	}

	.phead {
		display: flex;
		flex: none;
		align-items: center;
		gap: var(--sp-2);
		padding: var(--sp-2) var(--sp-3);
		border-bottom: 1px solid var(--line-1);
	}

	.phead h4 {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.phead .btn.on {
		color: var(--text-1);
		background: var(--bg-3);
	}

	.selected .view.spectrum {
		height: 340px;
	}

	.selected .view.matrix {
		height: 420px;
	}

	.meta {
		font-size: var(--text-xs);
		color: var(--text-3);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
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
</style>
