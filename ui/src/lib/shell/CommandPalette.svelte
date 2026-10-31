<script lang="ts">
	/**
	 * Command palette (Cmd/Ctrl+K): fuzzy search over workspaces, every catalog operation,
	 * datasets and control actions. Fully keyboard driven: arrows move, Enter runs, Escape closes.
	 */
	import { tick } from 'svelte';
	import type { AppState } from '$lib/app/state.svelte';
	import { WORKSPACES, workspaceInfo } from '$lib/app/workspaces';
	import { theme } from '$lib/theme/theme.svelte';
	import { fuzzyMatch, highlightRuns } from './fuzzy';
	import Icon, { type IconName } from './Icon.svelte';

	interface Props {
		app: AppState;
	}

	let { app }: Props = $props();

	type Group = 'Actions' | 'Workspaces' | 'Tools' | 'Datasets';

	interface Command {
		id: string;
		group: Group;
		title: string;
		subtitle?: string;
		keywords?: string;
		icon: IconName;
		/** Why it cannot run right now. */
		disabledReason?: string | null;
		run: () => void;
	}

	const GROUP_ORDER: Group[] = ['Actions', 'Workspaces', 'Tools', 'Datasets'];

	let query = $state('');
	let active = $state(0);
	let input = $state<HTMLInputElement | null>(null);
	let list = $state<HTMLUListElement | null>(null);
	let restoreFocus: HTMLElement | null = null;

	function close() {
		app.paletteOpen = false;
	}

	function controlReason(kind: 'start' | 'stop'): string | null {
		if (!app.control.isOpen) return 'Control service is not connected';
		if (kind === 'start' && app.sim.runState === 'running') return 'A simulation is already running';
		if (kind === 'start' && !app.sim.canStart) return 'Pick a profile first';
		if (kind === 'stop' && app.sim.runState !== 'running') return 'No simulation is running';
		return null;
	}

	const commands = $derived.by((): Command[] => {
		const out: Command[] = [
			{
				id: 'act:start',
				group: 'Actions',
				title: 'Start Simulation',
				subtitle: app.sim.profile ? `${app.sim.profile.title}, ${app.sim.values.level_db} dB` : 'Simulate',
				keywords: 'run sim go begin random',
				icon: 'play',
				disabledReason: controlReason('start'),
				run: () => {
					app.go('simulate');
					void app.sim.start();
				}
			},
			{
				id: 'act:stop',
				group: 'Actions',
				title: 'Stop Simulation',
				subtitle: 'Simulate',
				keywords: 'halt end abort sim',
				icon: 'stop',
				disabledReason: controlReason('stop'),
				run: () => void app.sim.stop()
			},
			{
				id: 'act:clear',
				group: 'Actions',
				title: 'Clear Analysis Results',
				subtitle: `${app.analyze.results.length} in Analyze`,
				keywords: 'remove panes reset',
				icon: 'trash',
				disabledReason: app.analyze.results.length ? null : 'No results to clear',
				run: () => app.analyze.clearResults()
			},
			{
				id: 'act:retry',
				group: 'Actions',
				title: 'Reconnect Services',
				subtitle: 'Analysis and control',
				keywords: 'retry connection socket',
				icon: 'reset',
				run: () => {
					app.analysis.retryNow();
					app.control.retryNow();
				}
			},
			...(['system', 'dark', 'light'] as const).map(
				(t): Command => ({
					id: `act:theme-${t}`,
					group: 'Actions',
					title: t === 'system' ? 'Theme: Follow System' : t === 'dark' ? 'Theme: Dark' : 'Theme: Light',
					subtitle: theme.pref === t ? 'Current' : 'Appearance',
					keywords: 'appearance color mode',
					icon: t === 'system' ? 'system' : t === 'dark' ? 'moon' : 'sun',
					run: () => theme.set(t)
				})
			)
		];

		WORKSPACES.forEach((w, i) =>
			out.push({
				id: `ws:${w.id}`,
				group: 'Workspaces',
				title: `Go to ${w.title}`,
				subtitle: w.live ? `Alt+${i + 1}` : 'Planned',
				keywords: w.summary,
				icon: w.id,
				run: () => app.go(w.id)
			})
		);

		for (const op of app.catalog?.operations ?? []) {
			out.push({
				id: `op:${op.id}`,
				group: 'Tools',
				title: op.title,
				subtitle: `${workspaceInfo(op.workspace).title}, works on ${op.accepts.replace('_', ' ')} data`,
				keywords: `${op.id} ${op.summary ?? ''}`,
				icon: 'tool',
				run: () => void openTool(op.id)
			});
		}

		for (const ds of app.catalog?.datasets ?? []) {
			out.push({
				id: `ds:${ds.id}`,
				group: 'Datasets',
				title: ds.title,
				subtitle: ds.kind === 'random_run' ? 'Random run, opens in Analyze' : 'Modal model, opens in Modal',
				keywords: `${ds.id} ${ds.description ?? ''}`,
				icon: 'dataset',
				run: () => {
					if (ds.kind === 'random_run') {
						app.go('analyze');
						void app.analyze.selectDataset(ds.id);
					} else {
						app.go('modal');
						void app.modal.select(ds.id);
					}
				}
			});
		}
		return out;
	});

	async function openTool(opId: string) {
		const op = app.catalog?.operations.find((o) => o.id === opId);
		if (!op) return;
		if (op.workspace === 'modal' || op.accepts === 'modal_model') {
			app.go('modal');
			const first = app.modal.datasets[0];
			if (!app.modal.datasetId && first) await app.modal.select(first.id);
			return;
		}
		app.go(op.workspace === 'analyze' ? 'analyze' : op.workspace);
		if (app.analyze.opened?.kind !== op.accepts) {
			const ds = app.catalog?.datasets.find((d) => d.kind === op.accepts);
			if (ds) await app.analyze.selectDataset(ds.id);
		}
		app.analyze.selectOp(op.id);
	}

	interface Row {
		cmd: Command;
		titleIdx: number[];
		score: number;
	}

	const rows = $derived.by((): Row[] => {
		const q = query.trim();
		const scored: Row[] = [];
		for (const cmd of commands) {
			if (!q) {
				scored.push({ cmd, titleIdx: [], score: 0 });
				continue;
			}
			const t = fuzzyMatch(q, cmd.title);
			const k = t ? null : fuzzyMatch(q, `${cmd.subtitle ?? ''} ${cmd.keywords ?? ''}`);
			if (t) scored.push({ cmd, titleIdx: t.indices, score: t.score });
			else if (k) scored.push({ cmd, titleIdx: [], score: k.score * 0.4 - 5 });
		}
		// Runnable commands first, then by score; group order breaks ties.
		const rank = (r: Row) => (r.cmd.disabledReason ? -1000 : 0) + r.score - GROUP_ORDER.indexOf(r.cmd.group) * 0.5;
		if (q) scored.sort((a, b) => rank(b) - rank(a));
		else scored.sort((a, b) => GROUP_ORDER.indexOf(a.cmd.group) - GROUP_ORDER.indexOf(b.cmd.group));
		return scored.slice(0, 60);
	});

	/** With a query the list is ranked flat; without, it is grouped. Headers mark group starts. */
	const grouped = $derived(query.trim() === '');

	$effect(() => {
		void query;
		active = 0;
	});

	$effect(() => {
		if (app.paletteOpen) {
			restoreFocus = document.activeElement as HTMLElement | null;
			query = '';
			active = 0;
			void tick().then(() => input?.focus());
		}
	});

	function finish(cmd: Command) {
		if (cmd.disabledReason) return;
		close();
		restoreFocus?.focus?.();
		cmd.run();
	}

	function scrollActive() {
		void tick().then(() => list?.querySelector<HTMLElement>('[aria-selected="true"]')?.scrollIntoView({ block: 'nearest' }));
	}

	function onKey(e: KeyboardEvent) {
		const n = rows.length;
		if (e.key === 'Escape') {
			e.preventDefault();
			close();
			restoreFocus?.focus?.();
		} else if (e.key === 'ArrowDown' || (e.key === 'n' && e.ctrlKey)) {
			e.preventDefault();
			if (n) active = (active + 1) % n;
			scrollActive();
		} else if (e.key === 'ArrowUp' || (e.key === 'p' && e.ctrlKey)) {
			e.preventDefault();
			if (n) active = (active - 1 + n) % n;
			scrollActive();
		} else if (e.key === 'Home' && e.ctrlKey) {
			active = 0;
			scrollActive();
		} else if (e.key === 'End' && e.ctrlKey) {
			active = Math.max(0, n - 1);
			scrollActive();
		} else if (e.key === 'Enter') {
			e.preventDefault();
			const row = rows[active];
			if (row) finish(row.cmd);
		} else if (e.key === 'Tab') {
			e.preventDefault();
		}
	}
</script>

{#if app.paletteOpen}
	<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
	<div class="scrim" onclick={close}></div>
	<div class="palette" data-testid="palette" role="dialog" aria-modal="true" aria-label="Command palette">
		<div class="search">
			<Icon name="search" size={18} />
			<input
				bind:this={input}
				data-testid="palette-input"
				bind:value={query}
				onkeydown={onKey}
				placeholder="Search workspaces, tools, datasets and actions"
				role="combobox"
				aria-expanded="true"
				aria-controls="palette-list"
				aria-activedescendant={rows[active] ? `cmd-${active}` : undefined}
				aria-autocomplete="list"
				autocomplete="off"
				spellcheck="false"
			/>
			<kbd>Esc</kbd>
		</div>
		<ul class="list" id="palette-list" role="listbox" bind:this={list} aria-label="Commands">
			{#each rows as row, i (row.cmd.id)}
				{#if grouped && (i === 0 || rows[i - 1].cmd.group !== row.cmd.group)}
					<li class="group" role="presentation">{row.cmd.group}</li>
				{/if}
				<!-- svelte-ignore a11y_click_events_have_key_events -->
				<li
					id="cmd-{i}"
					data-testid="palette-row-{row.cmd.id}"
					role="option"
					aria-selected={i === active}
					aria-disabled={!!row.cmd.disabledReason}
					class="item"
					class:active={i === active}
					class:disabled={!!row.cmd.disabledReason}
					onmousemove={() => (active = i)}
					onclick={() => finish(row.cmd)}
				>
					<span class="ico"><Icon name={row.cmd.icon} size={16} /></span>
					<span class="text">
						<span class="title">
							{#each highlightRuns(row.cmd.title, row.titleIdx) as run, k (k)}
								{#if run.hit}<mark>{run.text}</mark>{:else}{run.text}{/if}
							{/each}
						</span>
						<span class="sub">{row.cmd.disabledReason ?? row.cmd.subtitle ?? ''}</span>
					</span>
					{#if !grouped}<span class="kind">{row.cmd.group.replace(/s$/, '')}</span>{/if}
					{#if i === active && !row.cmd.disabledReason}<kbd class="enter">Enter</kbd>{/if}
				</li>
			{:else}
				<li class="empty" role="presentation">Nothing matches "{query}".</li>
			{/each}
		</ul>
		<div class="foot">
			<span><kbd>Up</kbd><kbd>Down</kbd> move</span>
			<span><kbd>Enter</kbd> run</span>
			<span><kbd>Esc</kbd> close</span>
		</div>
	</div>
{/if}

<style>
	.scrim {
		position: fixed;
		inset: 0;
		z-index: 40;
		background: var(--scrim);
		animation: fade var(--dur-med) var(--ease);
	}

	.palette {
		position: fixed;
		z-index: 41;
		top: 12vh;
		left: 50%;
		width: min(640px, calc(100vw - 32px));
		transform: translateX(-50%);
		display: flex;
		flex-direction: column;
		max-height: 70vh;
		background: var(--bg-1);
		border: 1px solid var(--line-2);
		border-radius: var(--radius-lg);
		box-shadow: var(--shadow-pop);
		animation: rise var(--dur-med) var(--ease);
		overflow: hidden;
	}

	.search {
		display: flex;
		align-items: center;
		gap: var(--sp-3);
		padding: 0 var(--sp-4);
		height: 52px;
		color: var(--text-3);
		border-bottom: 1px solid var(--line-1);
	}

	.search input {
		flex: 1;
		height: 100%;
		font-size: var(--text-lg);
		color: var(--text-1);
		background: transparent;
		border: 0;
	}

	.search input:focus-visible {
		outline: none;
	}

	.search input::placeholder {
		color: var(--text-4);
	}

	.list {
		flex: 1;
		margin: 0;
		padding: 6px;
		list-style: none;
		overflow-y: auto;
	}

	.group {
		padding: 10px 10px 4px;
		font-size: var(--text-2xs);
		font-weight: var(--weight-strong);
		letter-spacing: 0.08em;
		text-transform: uppercase;
		color: var(--text-3);
	}

	.item {
		display: flex;
		align-items: center;
		gap: var(--sp-3);
		padding: 7px 10px;
		border-radius: var(--radius-md);
		cursor: pointer;
	}

	.item.active {
		background: var(--bg-3);
		box-shadow: inset 2px 0 0 var(--blue-2);
	}

	.item.disabled {
		cursor: default;
	}

	.item.disabled .title,
	.item.disabled .ico {
		color: var(--text-4);
	}

	.ico {
		display: inline-flex;
		color: var(--text-3);
	}

	.item.active:not(.disabled) .ico {
		color: var(--blue-2);
	}

	.text {
		flex: 1;
		min-width: 0;
		display: flex;
		align-items: baseline;
		gap: var(--sp-3);
	}

	.title {
		color: var(--text-1);
		white-space: nowrap;
	}

	.title mark {
		color: var(--blue-1);
		background: none;
		text-decoration: underline;
		text-decoration-color: var(--blue-line);
		text-underline-offset: 3px;
	}

	.sub {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-size: var(--text-xs);
		color: var(--text-3);
	}

	.kind {
		font-size: var(--text-2xs);
		color: var(--text-3);
		text-transform: uppercase;
		letter-spacing: 0.06em;
	}

	.empty {
		padding: var(--sp-6);
		text-align: center;
		color: var(--text-3);
	}

	.foot {
		display: flex;
		gap: var(--sp-4);
		padding: 8px var(--sp-4);
		font-size: var(--text-2xs);
		color: var(--text-3);
		border-top: 1px solid var(--line-1);
		background: var(--bg-0);
	}

	.foot span {
		display: inline-flex;
		align-items: center;
		gap: 4px;
	}

	@keyframes fade {
		from {
			opacity: 0;
		}
	}

	@keyframes rise {
		from {
			opacity: 0;
			transform: translate(-50%, 6px);
		}
	}
</style>
