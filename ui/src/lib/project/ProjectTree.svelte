<script lang="ts">
	/**
	 * The project tree, as Visual Dynamics' window has it: one row per object in a fixed order by
	 * kind, each wearing its kind's icon, opening into what it holds. A geometry opens into its
	 * nodes, coordinate systems, element families with their element groups, solids and mated
	 * surfaces; a data object into a grid of its records, each cell wearing what that record
	 * measures; a shape set into its modes; a channel table into its channels. What is selected here
	 * is what the rest of the window works on: a click selects one row, Cmd (Ctrl elsewhere) adds or
	 * removes, Shift extends; a cell picks a record and stands for its object. No buttons live on
	 * the rows: the acts on a selection are offered beside the tree. A double-click renames.
	 */
	import type { AppState } from '$lib/app/state.svelte';
	import type { ProjectChildren, ProjectObject, SolidRow } from '$lib/protocol/types';
	import Glyph from './Glyph.svelte';
	import { kindRank, objectIcon, solidIcon } from './icons';

	interface Props {
		app: AppState;
	}
	let { app }: Props = $props();

	const p = $derived(app.project);
	/** The objects in the tree's order: by kind, then as they arrived. */
	const objects = $derived.by(() => {
		const listed = p.summary?.objects ?? [];
		return listed.map((o, i) => ({ o, i })).sort((a, b) => kindRank(a.o.kind) - kindRank(b.o.kind) || a.i - b.i).map((x) => x.o);
	});
	const order = $derived(objects.map((o) => o.name));
	const isMac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);

	/** Kinds that open into something. */
	const OPENS = new Set([
		'geometry', 'time_history', 'spectrum', 'psd', 'specification', 'srs', 'shock_specification', 'transient_specification',
		'sine_target', 'sine_level', 'frf', 'coherence', 'multiple_coherence', 'shape_set', 'channel_table', 'photos', 'matched_modes'
	]);
	const ROLE_WORD = { reference: 'ref', response: 'resp', monitor: 'mon' } as const;

	function mods(e: MouseEvent) {
		return { toggle: isMac ? e.metaKey : e.ctrlKey, extend: e.shiftKey };
	}

	function rowClick(e: MouseEvent, o: ProjectObject) {
		if (p.renaming === o.name) return;
		p.clickObject(o.name, order, mods(e));
	}

	function rowKey(e: KeyboardEvent, o: ProjectObject) {
		if (e.key === 'Enter' || e.key === ' ') {
			e.preventDefault();
			p.clickObject(o.name, order, { toggle: isMac ? e.metaKey : e.ctrlKey, extend: e.shiftKey });
		} else if (e.key === 'ArrowRight' && OPENS.has(o.kind) && !p.expanded.has(o.name)) p.toggleExpanded(o.name);
		else if (e.key === 'ArrowLeft' && p.expanded.has(o.name)) p.toggleExpanded(o.name);
		else if (e.key === 'F2') startRename(o.name);
	}

	let newName = $state('');
	function startRename(name: string) {
		p.renaming = name;
		newName = name;
	}
	async function commitRename() {
		const name = p.renaming;
		const to = newName.trim();
		p.renaming = null;
		if (name && to && to !== name) await p.rename(name, to);
	}
	function focusAndSelect(node: HTMLInputElement) {
		node.focus();
		node.select();
	}

	const picked = (name: string, index: number) => (p.selection.records[name] ?? []).includes(index);
	const partPicked = (name: string, kind: string, id: string | number) =>
		p.selection.part?.name === name && p.selection.part.kind === kind && p.selection.part.id === id;
	const held = (name: string): ProjectChildren | undefined => p.children[name];
	/** How many rows and columns a grid shows before it scrolls, as Visual Dynamics' grid does. */
	const roleWord = (role?: string) => (role ? ROLE_WORD[role as keyof typeof ROLE_WORD] : '');
</script>

{#snippet solidRows(name: string, solids: SolidRow[], depth: number)}
	{#each solids as solid (solid.id)}
		<div
			class="sub"
			class:picked={partPicked(name, 'solid', solid.id)}
			style="--depth: {depth}"
			role="treeitem"
			aria-selected={partPicked(name, 'solid', solid.id)}
			tabindex="-1"
			data-testid="solid-{name}-{solid.id}"
			onclick={() => p.clickPart(name, 'solid', solid.id)}
			onkeydown={(e) => e.key === 'Enter' && p.clickPart(name, 'solid', solid.id)}
		>
			<Glyph name={solidIcon(solid.kind)} of="child" parent="geometry" size={14} />
			<span class="label">{solid.label}</span>
		</div>
		{#if solid.children.length}{@render solidRows(name, solid.children, depth + 1)}{/if}
	{/each}
{/snippet}

<div class="tree" role="tree" aria-label="Project objects" data-testid="project-tree">
	{#each objects as o (o.name)}
		{@const open = p.expanded.has(o.name)}
		{@const on = p.isSelected(o.name)}
		{@const active = o.name === p.summary?.active_geometry}
		{@const icon = objectIcon(o.kind, o.banded)}
		<div class="row" class:on role="treeitem" aria-selected={on} aria-expanded={OPENS.has(o.kind) ? open : undefined} tabindex="0" data-testid="object-{o.name}" onclick={(e) => rowClick(e, o)} onkeydown={(e) => rowKey(e, o)} ondblclick={() => startRename(o.name)}>
			{#if OPENS.has(o.kind)}
				<button class="twist" class:open aria-label={open ? `Close ${o.name}` : `Open ${o.name}`} tabindex="-1" data-testid="expand-{o.name}" onclick={(e) => { e.stopPropagation(); p.toggleExpanded(o.name); }}>
					<svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true"><path d="M3 1.5l4 3.5-4 3.5" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" /></svg>
				</button>
			{:else}
				<span class="twist blank"></span>
			{/if}
			<Glyph name={icon} badge={o.units_defined === false} title={o.units_defined === false ? 'Units not declared' : undefined} />
			{#if p.renaming === o.name}
				<input
					class="field mono rename"
					aria-label="New name for {o.name}"
					data-testid="rename-field"
					bind:value={newName}
					use:focusAndSelect
					onclick={(e) => e.stopPropagation()}
					onkeydown={(e) => {
						e.stopPropagation();
						if (e.key === 'Enter') void commitRename();
						if (e.key === 'Escape') p.renaming = null;
					}}
					onblur={() => (p.renaming = null)}
				/>
			{:else}
				<span class="name">{o.name}</span>
				{#if active}<span class="mark" title="Data is drawn on this geometry when nothing else names one">active</span>{/if}
				{#if o.stale}<span class="mark stale" data-testid="stale-{o.name}" title={o.stale}>stale</span>{/if}
				<span class="holds" title={o.holds}>{o.holds ?? o.kind.replace(/_/g, ' ')}</span>
			{/if}
		</div>
		{#if open}
			{@const kids = held(o.name)}
			<div class="kids" role="group" data-testid="children-{o.name}">
				{#if !kids}
					<p class="note">Opening.</p>
				{:else if kids.kind === 'geometry'}
					{#each kids.parts as part (part.id)}
						<div class="sub" class:picked={partPicked(o.name, 'part', part.id)} role="treeitem" aria-selected={partPicked(o.name, 'part', part.id)} tabindex="-1" data-testid="part-{o.name}-{part.id}" onclick={() => p.clickPart(o.name, 'part', part.id)} onkeydown={(e) => e.key === 'Enter' && p.clickPart(o.name, 'part', part.id)}>
							<Glyph name={part.id} of="child" parent="geometry" empty={!part.count} size={14} />
							<span class="label">{part.label} ({part.count})</span>
						</div>
					{/each}
					{#each kids.families as family (family.id)}
						<div class="sub" class:picked={partPicked(o.name, 'family', family.id)} role="treeitem" aria-selected={partPicked(o.name, 'family', family.id)} tabindex="-1" data-testid="family-{o.name}-{family.id}" onclick={() => p.clickPart(o.name, 'family', family.id)} onkeydown={(e) => e.key === 'Enter' && p.clickPart(o.name, 'family', family.id)}>
							<Glyph name={family.id} of="child" parent="geometry" empty={!family.count} size={14} />
							<span class="label" class:dim={!family.count}>{family.label} ({family.count})</span>
						</div>
						{#each family.groups as group (group.id)}
							<div class="sub" class:picked={partPicked(o.name, 'group', group.id)} style="--depth: 1" role="treeitem" aria-selected={partPicked(o.name, 'group', group.id)} tabindex="-1" data-testid="group-{o.name}-{group.id}" onclick={() => p.clickPart(o.name, 'group', group.id)} onkeydown={(e) => e.key === 'Enter' && p.clickPart(o.name, 'group', group.id)}>
								<Glyph name="groups" of="child" parent="geometry" size={14} />
								<span class="label">{group.label} ({group.count})</span>
							</div>
						{/each}
					{/each}
					<div class="sub" class:picked={partPicked(o.name, 'solids', 0)} role="treeitem" aria-selected={partPicked(o.name, 'solids', 0)} tabindex="-1" data-testid="solids-{o.name}" title="Shapes to build parts from, before they are meshed" onclick={() => p.clickPart(o.name, 'solids', 0)} onkeydown={(e) => e.key === 'Enter' && p.clickPart(o.name, 'solids', 0)}>
						<Glyph name="solid_block" of="child" parent="geometry" size={14} />
						<span class="label" class:dim={!kids.solids.length}>Solids ({kids.solids.length})</span>
					</div>
					{@render solidRows(o.name, kids.solids, 1)}
					{#if kids.mates.length}
						<div class="sub" class:picked={partPicked(o.name, 'mates', 0)} role="treeitem" aria-selected={partPicked(o.name, 'mates', 0)} tabindex="-1" data-testid="mates-{o.name}" onclick={() => p.clickPart(o.name, 'mates', 0)} onkeydown={(e) => e.key === 'Enter' && p.clickPart(o.name, 'mates', 0)}>
							<Glyph name="mates" of="child" parent="geometry" size={14} />
							<span class="label">Mated Surfaces ({kids.mates.length})</span>
						</div>
						{#each kids.mates as mate (mate.id)}
							<div class="sub" class:picked={partPicked(o.name, 'mate', mate.id)} style="--depth: 1" role="treeitem" aria-selected={partPicked(o.name, 'mate', mate.id)} tabindex="-1" data-testid="mate-{o.name}-{mate.id}" onclick={() => p.clickPart(o.name, 'mate', mate.id)} onkeydown={(e) => e.key === 'Enter' && p.clickPart(o.name, 'mate', mate.id)}>
								<Glyph name="mates" of="child" parent="geometry" size={14} />
								<span class="label">{mate.label}</span>
							</div>
						{/each}
					{/if}
				{:else if kids.kind === 'grid'}
					{@const headed = kids.columns.some((c) => c)}
					{@const roles = kids.rows.some((r) => r.role)}
					<div class="gridbox">
						<table class="grid" data-testid="grid-{o.name}">
							{#if headed}
								<thead>
									<tr>
										<th></th>
										{#if roles}<th class="role"></th>{/if}
										{#each kids.columns as column, c (c)}<th class="col" title={column}>{column}</th>{/each}
									</tr>
								</thead>
							{/if}
							<tbody>
								{#each kids.rows as row, r (r)}
									<tr>
										<th class="dof" title={row.quantity ? `${row.dof}: ${row.quantity.replace(/_/g, ' ')}` : row.dof}>{row.label}</th>
										{#if roles}<td class="role" title={row.role}>{roleWord(row.role)}</td>{/if}
										{#each kids.cells[r] as index, c (c)}
											{#if index < 0}
												<td class="hole" aria-hidden="true"></td>
											{:else}
												<td
													class="cell"
													class:picked={picked(o.name, index)}
													role="button"
													tabindex="-1"
													aria-pressed={picked(o.name, index)}
													aria-label="{row.label}{headed ? ` against ${kids.columns[c]}` : ''}"
													data-testid="cell-{o.name}-{index}"
													onclick={(e) => { e.stopPropagation(); p.clickRecord(o.name, index, mods(e), kids.cells); }}
												>
													<Glyph name={kids.icons[index]} of="record" parent={icon} badge={kids.undefined.includes(index)} size={14} />
												</td>
											{/if}
										{/each}
									</tr>
								{/each}
							</tbody>
						</table>
					</div>
				{:else if kids.kind === 'modes'}
					{#each kids.modes as mode (mode.index)}
						<div class="sub" class:picked={picked(o.name, mode.index - 1)} role="treeitem" aria-selected={picked(o.name, mode.index - 1)} tabindex="-1" data-testid="mode-{o.name}-{mode.index}" onclick={(e) => p.clickRecord(o.name, mode.index - 1, mods(e))} onkeydown={(e) => e.key === 'Enter' && p.clickRecord(o.name, mode.index - 1)}>
							<Glyph name="mode" of="child" parent="shape_set" size={14} />
							<span class="label mono">M{mode.index}</span>
							<span class="num">{mode.frequency.toPrecision(5)} Hz</span>
							<span class="num dim">{(mode.damping * 100).toFixed(2)} %</span>
							{#if mode.description}<span class="dim desc" title={mode.description}>{mode.description}</span>{/if}
						</div>
					{/each}
				{:else if kids.kind === 'channels'}
					{#each kids.channels as channel, i (i)}
						<div class="sub" class:picked={picked(o.name, i)} role="treeitem" aria-selected={picked(o.name, i)} tabindex="-1" data-testid="channel-{o.name}-{i}" onclick={(e) => p.clickRecord(o.name, i, mods(e))} onkeydown={(e) => e.key === 'Enter' && p.clickRecord(o.name, i)}>
							<Glyph name={channel.quantity} of="record" parent="channel_table" size={14} />
							<span class="label mono">{channel.dof || `channel ${i + 1}`}</span>
						</div>
					{/each}
				{:else if kids.kind === 'photos'}
					{#each kids.photos as photo, i (i)}
						<div class="sub" class:picked={picked(o.name, i)} role="treeitem" aria-selected={picked(o.name, i)} tabindex="-1" data-testid="photo-{o.name}-{i}" onclick={(e) => p.clickRecord(o.name, i, mods(e))} onkeydown={(e) => e.key === 'Enter' && p.clickRecord(o.name, i)}>
							<Glyph name="photos" size={14} />
							<span class="label">{photo}</span>
						</div>
					{/each}
				{:else if kids.kind === 'matches'}
					{#each kids.matches as match, i (i)}
						<div class="sub" class:picked={picked(o.name, i)} role="treeitem" aria-selected={picked(o.name, i)} tabindex="-1" data-testid="match-{o.name}-{i}" onclick={(e) => p.clickRecord(o.name, i, mods(e))} onkeydown={(e) => e.key === 'Enter' && p.clickRecord(o.name, i)}>
							<Glyph name="mode" of="child" parent="matched_modes" size={14} />
							<span class="label mono">M{match.first} to M{match.second}</span>
							<span class="num dim">MAC {match.mac.toFixed(3)}</span>
						</div>
					{/each}
				{:else}
					<p class="note">Nothing to open.</p>
				{/if}
			</div>
		{/if}
	{/each}
</div>

<style>
	.tree {
		margin-top: var(--sp-2);
		font-size: var(--text-xs);
		user-select: none;
	}

	.row {
		display: flex;
		align-items: center;
		gap: 5px;
		padding: 3px 6px 3px 2px;
		border-radius: var(--radius-sm);
		cursor: default;
	}

	.row:hover {
		background: var(--bg-3);
	}

	.row.on {
		background: var(--blue-weak);
		box-shadow: inset 0 0 0 1px var(--blue-line);
	}

	.row:focus-visible,
	.sub:focus-visible {
		outline: 1px solid var(--accent);
		outline-offset: -1px;
	}

	.twist {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 14px;
		height: 14px;
		padding: 0;
		color: var(--text-4);
		background: none;
		border: none;
		border-radius: 2px;
		cursor: pointer;
		transition: transform var(--dur-fast) var(--ease);
	}

	.twist.open {
		transform: rotate(90deg);
	}

	.twist:hover {
		color: var(--text-1);
	}

	.twist.blank {
		cursor: default;
	}

	.name {
		color: var(--text-1);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.holds {
		margin-left: auto;
		padding-left: 6px;
		color: var(--text-4);
		font-size: 10px;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		max-width: 45%;
	}

	.mark {
		font-size: 10px;
		color: var(--text-3);
	}

	.mark.stale {
		color: var(--status-warn);
	}

	.rename {
		flex: 1;
		min-width: 0;
		font-size: var(--text-xs);
	}

	.kids {
		margin-left: 14px;
		padding-left: 8px;
		border-left: 1px solid var(--line-1);
	}

	.sub {
		display: flex;
		align-items: center;
		gap: 5px;
		padding: 2px 6px 2px calc(2px + 14px * var(--depth, 0));
		border-radius: var(--radius-sm);
		color: var(--text-2);
		cursor: default;
	}

	.sub:hover {
		background: var(--bg-3);
	}

	.sub.picked {
		background: var(--blue-weak);
		box-shadow: inset 0 0 0 1px var(--blue-line);
	}

	.label {
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.dim {
		color: var(--text-4);
	}

	.desc {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.num {
		font-variant-numeric: tabular-nums;
		white-space: nowrap;
	}

	.note {
		padding: 2px 6px;
		color: var(--text-4);
	}

	/* the record grid: rows by DOF, columns by reference or capture, a 14 px icon in every cell;
	   past Visual Dynamics' limits it scrolls rather than the tree */
	.gridbox {
		max-height: 320px;
		max-width: 460px;
		overflow: auto;
		margin: 2px 0 4px;
		border: 1px solid var(--line-1);
		border-radius: var(--radius-sm);
	}

	.grid {
		border-collapse: collapse;
		font-size: 10px;
	}

	.grid th,
	.grid td {
		padding: 0;
		text-align: center;
		vertical-align: middle;
		line-height: 1;
	}

	.grid thead th {
		position: sticky;
		top: 0;
		padding: 3px 4px;
		font-weight: var(--weight-medium);
		color: var(--text-3);
		background: var(--bg-1);
		white-space: nowrap;
	}

	.grid th.dof {
		position: sticky;
		left: 0;
		padding: 0 6px;
		font-family: var(--font-mono);
		font-weight: normal;
		color: var(--text-2);
		text-align: right;
		background: var(--bg-1);
		white-space: nowrap;
	}

	.grid td.role {
		padding: 0 4px;
		color: var(--text-4);
	}

	.grid td.cell {
		width: 22px;
		height: 20px;
		cursor: default;
	}

	.grid td.cell:hover {
		background: var(--bg-3);
	}

	.grid td.cell.picked {
		background: var(--blue-weak);
		box-shadow: inset 0 0 0 1px var(--blue-line);
	}

	.grid td.hole {
		width: 22px;
		height: 20px;
		background: repeating-linear-gradient(45deg, transparent 0 3px, var(--line-1) 3px 4px);
	}
</style>
