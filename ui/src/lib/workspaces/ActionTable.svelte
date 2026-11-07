<script lang="ts">
	/**
	 * The action table beside the project tree (Brandon, 2026-11-06). Its first column is the
	 * selection and stays put; each other column is the selection with one more object, and scrolls.
	 * Rows are short verbs: a default is what the selection draws by itself, so it is not clicked
	 * (until a reading has taken its place, when it is the way back); a verb runs or opens its form;
	 * a grey verb says why it cannot. A combination's verb adds the object it needs, asking in the
	 * tree when there is more than one. The form opens in this column, so the plot stays in view.
	 */
	import type { AppState } from '$lib/app/state.svelte';
	import { workspaceInfo } from '$lib/app/workspaces';
	import ParamForm from '$lib/forms/ParamForm.svelte';
	import type { ActionColumn, ActionRow, Workspace } from '$lib/protocol/types';
	import { columnsFor, kindWords, moreIn } from '$lib/project/actionTable';
	import Glyph from '$lib/project/Glyph.svelte';
	import UnitsEditor from '$lib/project/UnitsEditor.svelte';
	import Icon from '$lib/shell/Icon.svelte';

	interface Props {
		app: AppState;
		/** The module whose verbs are offered. */
		only: Workspace;
	}
	let { app, only }: Props = $props();

	const a = $derived(app.analyze);
	const p = $derived(app.project);
	const table = $derived(a.actions?.table ?? null);
	const columns = $derived(table ? columnsFor(table, only) : []);
	const current = $derived(columns.find((c) => c.current) ?? null);
	const others = $derived(columns.filter((c) => !c.current));
	const rowCount = $derived(Math.max(0, ...columns.map((c) => c.rows.length)));
	const more = $derived(table ? moreIn(table, only) : []);
	const picked = $derived(p.selection.objects);
	const one = $derived(picked.length === 1 ? picked[0] : null);
	const coherences = $derived((p.summary?.objects ?? []).filter((o) => o.kind === 'coherence' || o.kind === 'multiple_coherence').map((o) => o.name));
	const isMac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);

	let weight = $state('');
	let groupName = $state('');
	$effect(() => {
		if (p.namingGroup) {
			const group = (p.summary?.links ?? []).find((g) => g.members.includes(p.namingGroup ?? ''));
			groupName = group?.name ?? '';
		}
	});

	function focusField(node: HTMLInputElement) {
		node.focus();
		node.select();
	}

	/** A default is a plain word while it is drawn; once a reading or another view has replaced it, it is the way back. */
	function live(row: ActionRow, column: ActionColumn): boolean {
		if (!row.ready) return false;
		return !(row.kind === 'default' && column.current && !a.reading && !a.shapeView);
	}

	function beginFit() {
		const kinds = picked.map((n) => p.summary?.objects.find((o) => o.name === n)?.kind);
		const frf = picked[kinds.indexOf('frf')];
		const shapes = kinds.includes('shape_set') ? picked[kinds.indexOf('shape_set')] : undefined;
		a.fitWeighing = false;
		void app.fit.start(frf, shapes, weight || undefined);
	}

	function onKey(e: KeyboardEvent) {
		if (e.key === 'Escape' && a.picking) {
			e.preventDefault();
			a.picking = null;
		}
	}

	// the columns that can run off the right edge, counted for the cue a Mac's hidden scrollbar leaves to us
	let scroller = $state<HTMLDivElement | null>(null);
	let hidden = $state(0);
	function measure() {
		if (!scroller) return;
		const edge = scroller.getBoundingClientRect().right;
		const heads = [...scroller.querySelectorAll('th.other:not(.off)')];
		hidden = heads.filter((th) => th.getBoundingClientRect().left + 8 >= edge).length;
	}
	$effect(() => {
		if (!scroller) return;
		void columns;
		measure();
		const seen = new ResizeObserver(measure);
		seen.observe(scroller);
		return () => seen.disconnect();
	});
</script>

<svelte:window onkeydown={onKey} />

<div class="column" data-testid="action-column">
	{#if a.picking}
		<p class="banner" role="status" data-testid="action-picking">
			Pick a {kindWords(a.picking.kind)} for {a.picking.verb}.
			<button class="link" onclick={() => (a.picking = null)}>Cancel</button>
		</p>
	{/if}
	{#if p.confirming && picked.length}
		{@const parts = Object.entries(p.pickedParts)}
		<div class="banner" data-testid="action-confirm">
			{#if parts.length}
				<span>Delete {parts.map(([name, picks]) => `${picks.length} of ${name}`).join(', ')}?</span>
				<button class="btn ghost sm danger" data-testid="remove-confirm" onclick={() => p.removeSelected()}>Delete</button>
			{:else}
				<span>Remove {one ?? `${picked.length} objects`}?</span>
				<button class="btn ghost sm danger" data-testid="remove-confirm" onclick={() => p.removeSelected()}>Remove</button>
			{/if}
			<button class="btn ghost sm" onclick={() => (p.confirming = false)}>Keep</button>
		</div>
	{/if}
	{#if p.namingGroup && one}
		<form class="banner" onsubmit={(e) => { e.preventDefault(); const text = groupName; p.namingGroup = null; void p.nameGroup(one, text); }}>
			<input class="field" aria-label="Name for the group of {one}" data-testid="group-name" bind:value={groupName} use:focusField placeholder="Group name; blank removes it" onkeydown={(e) => e.key === 'Escape' && (p.namingGroup = null)} />
			<button class="btn primary sm" type="submit">Name</button>
		</form>
	{/if}

	{#if p.units}
		<div class="phead">
			<button class="btn ghost icon sm" data-testid="units-back" aria-label="Back to the table" title="Back to the table" onclick={() => (p.units = null)}><Icon name="chevron" size={13} /></button>
			<h2>Units of {p.units.name}</h2>
		</div>
		<div class="pane"><UnitsEditor project={p} tables={p.units} /></div>
	{:else if a.fitWeighing}
		<div class="phead">
			<button class="btn ghost icon sm" aria-label="Back to the table" title="Back to the table" onclick={() => (a.fitWeighing = false)}><Icon name="chevron" size={13} /></button>
			<h2>Fit by hand</h2>
		</div>
		<div class="pane">
			<label class="weigh">
				Weight each residual by
				<select class="field sm" data-testid="fit-weight" bind:value={weight}>
					<option value="">No weighting</option>
					{#each coherences as name (name)}<option value={name}>{name}</option>{/each}
				</select>
			</label>
			<button class="btn primary run" data-testid="fit-begin" onclick={beginFit}>Begin</button>
			{#if app.fit.error && !app.fit.state}<p class="error" role="alert">{app.fit.error}</p>{/if}
		</div>
	{:else if a.formOpen && a.op}
		<section class="pane form" aria-labelledby="op-title">
			<div class="phead">
				<button class="btn ghost icon sm" data-testid="form-back" aria-label="Back to the table" title="Back to the table" onclick={() => a.closeForm()}><Icon name="chevron" size={13} /></button>
				<h2 id="op-title">{a.op.title}</h2>
				<button class="btn ghost sm" onclick={() => a.resetParams()} title="Reset every field to its default">Defaults</button>
			</div>
			{#if a.op.summary}<p class="psum">{a.op.summary}</p>{/if}
			<ParamForm params={a.formParams} bind:values={a.values[a.op.id]} showAllErrors={a.triedRun} idPrefix={a.op.id.replace('.', '-')} />
			<div class="runrow">
				<button class="btn primary run" data-testid="run-op" onclick={() => a.run()} disabled={a.running || !app.analysis.isOpen}>
					{#if a.running}Running{:else}<Icon name="play" size={13} /> Run{/if}
				</button>
				<span class="keys"><kbd>{isMac ? 'Cmd' : 'Ctrl'}</kbd><kbd>Enter</kbd></span>
			</div>
			{#if a.runError}<p class="error" role="alert">{a.runError}</p>{/if}
		</section>
	{:else if current}
		{#if a.runError}<p class="error" role="alert">{a.runError}</p>{/if}
		<div class="wrap">
			<div class="scroll" bind:this={scroller} onscroll={measure} data-testid="action-table">
				<table>
					<thead>
						<tr>
							<th class="sel" scope="col" title={picked.length ? picked.join(', ') : 'The project'}>
								{#if current.kinds.length}
									<span class="heads">{#each current.kinds.slice(0, 3) as kind, i (i)}<Glyph name={kind} size={16} />{/each}{#if current.kinds.length > 3}<span class="count">+{current.kinds.length - 3}</span>{/if}</span>
								{:else}
									<span class="word">Project</span>
								{/if}
							</th>
							{#each others as column (column.kinds.join('+'))}
								{@const last = column.kinds[column.kinds.length - 1]}
								<th class="other" class:off={!column.rows.some((r) => r.ready)} scope="col" title="With a {kindWords(last)}" data-testid="action-head-{last}">
									<span class="heads">
										{#each column.kinds.slice(0, -1).slice(0, 2) as kind, i (i)}<Glyph name={kind} size={16} />{/each}
										{#if last === '*'}<span class="any" aria-label="any object">*</span>{:else}<Glyph name={last} size={16} />{/if}
									</span>
								</th>
							{/each}
						</tr>
					</thead>
					<tbody>
						{#each { length: rowCount } as _, i (i)}
							<tr>
								{#each columns as column (column.kinds.join('+'))}
									{@const row = column.rows[i]}
									<td class:sel={column.current}>
										{#if row}
											{@const clickable = live(row, column)}
											{#if clickable}
												<button class="verb" class:default={row.kind === 'default'} class:on={a.readingId === row.id || (column.current && row.id === `show.${a.shapeView}`)} data-testid="action-{row.id}" title={column.current ? undefined : `Adds a ${kindWords(column.kinds[column.kinds.length - 1])}`} onclick={() => a.perform(row, column)}>{row.verb}</button>
											{:else if row.kind === 'default' && row.ready}
												<span class="verb default still" data-testid="action-{row.id}" title="Drawn by itself when this is selected">{row.verb}</span>
											{:else}
												<span class="verb gone" data-testid="action-{row.id}" title={row.reason}>{row.verb}</span>
											{/if}
										{/if}
									</td>
								{/each}
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
			{#if hidden > 0}
				<div class="fade" aria-hidden="true"></div>
				<span class="hiddencount" data-testid="action-hidden" title="{hidden} more to the right">+{hidden}</span>
			{/if}
		</div>
		{#if more.length}
			<p class="more" data-testid="action-more">
				More in
				{#each more as m, i (m.module)}{#if i > 0}{', '}{/if}<button class="link" data-testid="action-more-{m.module}" onclick={() => app.go(m.module)}>{workspaceInfo(m.module).title} ({m.count})</button>{/each}
			</p>
		{/if}
	{:else if p.summary}
		<p class="muted">Asking what the selection can take.</p>
	{/if}
</div>

<style>
	.column {
		display: flex;
		flex-direction: column;
		min-height: 0;
		height: 100%;
		min-width: 0;
		overflow: hidden;
		background: var(--bg-1);
		font-size: var(--text-sm);
	}
	.banner {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2);
		padding: var(--sp-2) var(--sp-3);
		border-bottom: 1px solid var(--line-1);
		background: var(--blue-weak);
		color: var(--text-1);
	}
	.banner .field {
		flex: 1;
		min-width: 80px;
	}
	.phead {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		padding: var(--sp-2) var(--sp-3);
	}
	.phead h2 {
		flex: 1;
		min-width: 0;
		font-size: var(--text-md);
		font-weight: var(--weight-strong);
	}
	.phead :global(svg) {
		transform: rotate(180deg);
	}
	.pane {
		flex: 1;
		min-height: 0;
		overflow-y: auto;
		padding: 0 var(--sp-3) var(--sp-3);
	}
	.pane.form {
		padding: 0 0 var(--sp-3);
	}
	.pane.form > :global(*:not(.phead)) {
		margin-left: var(--sp-3);
		margin-right: var(--sp-3);
	}
	.psum {
		margin-bottom: var(--sp-3);
		color: var(--text-3);
		font-size: var(--text-xs);
	}
	.runrow {
		display: flex;
		align-items: center;
		gap: var(--sp-3);
		margin-top: var(--sp-3);
	}
	.keys {
		display: flex;
		gap: 2px;
		color: var(--text-4);
		font-size: var(--text-2xs);
	}
	.weigh {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		margin-bottom: var(--sp-3);
		color: var(--text-2);
	}
	.wrap {
		position: relative;
		flex: 1;
		min-height: 0;
	}
	.scroll {
		height: 100%;
		overflow: auto;
	}
	table {
		border-collapse: separate;
		border-spacing: 0;
	}
	th {
		position: sticky;
		top: 0;
		z-index: 1;
		padding: 6px 8px;
		background: var(--bg-1);
		border-bottom: 1px solid var(--line-2);
		font-weight: normal;
		white-space: nowrap;
	}
	th.off .heads {
		opacity: 0.45;
	}
	.heads {
		display: inline-flex;
		align-items: center;
		gap: 2px;
	}
	.any {
		width: 16px;
		color: var(--text-2);
		font-size: var(--text-md);
		line-height: 1;
		text-align: center;
	}
	.word,
	.count {
		color: var(--text-3);
		font-size: var(--text-xs);
	}
	td {
		padding: 0 2px;
		vertical-align: middle;
		text-align: center;
		white-space: nowrap;
	}
	th.sel,
	td.sel {
		position: sticky;
		left: 0;
		background: var(--bg-2);
		border-right: 1px solid var(--line-2);
	}
	th.sel {
		z-index: 2;
	}
	.verb {
		display: block;
		width: 100%;
		padding: 3px 6px;
		border: 0;
		border-radius: var(--radius-sm);
		background: none;
		color: var(--accent);
		font: inherit;
		text-align: center;
		white-space: nowrap;
	}
	button.verb {
		cursor: pointer;
	}
	button.verb:hover {
		background: var(--blue-weak);
	}
	button.verb.on {
		background: var(--blue-weak);
		color: var(--text-1);
	}
	.verb.default {
		color: var(--action-default);
	}
	button.verb.default {
		text-decoration: underline;
		text-underline-offset: 2px;
	}
	.verb.still {
		font-weight: var(--weight-strong);
		cursor: default;
	}
	.verb.gone {
		color: var(--text-4);
		cursor: default;
	}
	.fade {
		position: absolute;
		top: 0;
		right: 0;
		bottom: 0;
		width: 36px;
		background: linear-gradient(to right, transparent, var(--bg-1));
		pointer-events: none;
	}
	.hiddencount {
		position: absolute;
		top: 6px;
		right: 6px;
		padding: 0 5px;
		border-radius: var(--radius-sm);
		background: var(--bg-4);
		color: var(--text-2);
		font-size: var(--text-xs);
		pointer-events: none;
	}
	.more {
		padding: var(--sp-2) var(--sp-3);
		border-top: 1px solid var(--line-1);
		color: var(--text-3);
		font-size: var(--text-xs);
	}
	.link {
		border: 0;
		padding: 0;
		background: none;
		color: var(--accent);
		font: inherit;
		cursor: pointer;
	}
	.muted {
		padding: var(--sp-3);
		color: var(--text-3);
	}
	.error {
		margin: var(--sp-2) var(--sp-3);
		padding: 6px 8px;
		font-size: var(--text-xs);
		color: var(--amber-2);
		background: var(--amber-weak);
		border-left: 2px solid var(--amber-2);
		border-radius: var(--radius-sm);
	}
</style>
