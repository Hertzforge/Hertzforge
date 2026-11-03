<script lang="ts">
	/** Top bar: where you are, the palette trigger, the SIM badge, service state and theme. */
	import type { AppState } from '$lib/app/state.svelte';
	import { workspaceInfo } from '$lib/app/workspaces';
	import { theme } from '$lib/theme/theme.svelte';
	import ConnectionPill from './ConnectionPill.svelte';
	import { db, STATE_WORDS } from '$lib/acquire/words';
	import Icon from './Icon.svelte';
	import ProvenanceBadge from './ProvenanceBadge.svelte';

	interface Props {
		app: AppState;
	}
	let { app }: Props = $props();

	const isMac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
	const themeLabel = $derived(theme.pref === 'system' ? 'System' : theme.pref === 'dark' ? 'Dark' : 'Light');
	const prov = $derived(app.provenance);
	/** A live Random run stays in reach from wherever it is not on screen: another workspace, or the test-type chooser. */
	const away = $derived(app.acquire.isLive && !app.runOnScreen ? app.acquire : null);
</script>

<header class="topbar">
	<div class="brand">
		<button
			class="btn ghost icon sm railbtn"
			data-testid="rail-toggle"
			aria-expanded={!app.railCollapsed}
			aria-label={app.railCollapsed ? 'Show the workspace rail' : 'Minimize the workspace rail'}
			title={app.railCollapsed ? 'Show the workspace rail' : 'Minimize the workspace rail, to give the plots the screen'}
			onclick={() => app.toggleRail()}
		>
			<svg viewBox="0 0 16 16" width="15" height="15" aria-hidden="true">
				<rect x="1.5" y="2.5" width="13" height="11" rx="1.6" fill="none" stroke="currentColor" stroke-width="1.3" />
				{#if app.railCollapsed}
					<path d="M5 2.5v11" stroke="currentColor" stroke-width="1.3" /><path d="M8.5 6l2 2-2 2" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" />
				{:else}
					<rect x="1.5" y="2.5" width="4" height="11" rx="1.2" fill="currentColor" opacity="0.35" /><path d="M5.5 2.5v11" stroke="currentColor" stroke-width="1.3" /><path d="M10.5 6l-2 2 2 2" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" />
				{/if}
			</svg>
		</button>
		<svg class="mark" viewBox="0 0 24 24" width="22" height="22" aria-hidden="true">
			<rect x="0.75" y="0.75" width="22.5" height="22.5" rx="6" fill="var(--bg-3)" stroke="var(--line-2)" />
			<path d="M4 13.5c2.2 0 2.2-6 4.4-6s2.2 10 4.4 10 2.2-7 4.4-7H20" fill="none" stroke="var(--blue-2)" stroke-width="1.8" stroke-linecap="round" />
		</svg>
		<span class="name">Hertzforge</span>
		<span class="sep" aria-hidden="true">/</span>
		<span class="ws">{workspaceInfo(app.workspace).title}</span>
	</div>

	<button class="trigger" data-testid="palette-trigger" onclick={() => (app.paletteOpen = true)} aria-keyshortcuts={isMac ? 'Meta+K' : 'Control+K'}>
		<Icon name="search" size={15} />
		<span class="ph">Search tools, datasets, actions</span>
		<span class="keys"><kbd>{isMac ? 'Cmd' : 'Ctrl'}</kbd><kbd>K</kbd></span>
	</button>

	<div class="right">
		{#if app.config.mock}
			<span class="mock" data-testid="mock-indicator" title="In-process mock services with synthetic data. Remove ?mock=1 to use the real services.">
				<svg viewBox="0 0 12 12" width="10" height="10" aria-hidden="true"><rect x="1.5" y="1.5" width="9" height="9" rx="1.5" fill="none" stroke="currentColor" stroke-width="1.3" stroke-dasharray="2 1.5" /></svg>
				Mock Services
			</span>
		{/if}
		{#if away}
			<span class="runpill" data-testid="run-pill">
				<button class="go" data-testid="run-pill-go" onclick={() => app.showRun()} title="Return to the run">
					<svg viewBox="0 0 10 10" width="8" height="8" aria-hidden="true"><circle cx="5" cy="5" r="4" fill="currentColor" /></svg>
					Random {STATE_WORDS[away.run?.state ?? 'running'].toLowerCase()}{#if away.frame?.level_db !== null && away.frame?.level_db !== undefined}<span class="num lv">{db(away.frame.level_db)}</span>{/if}
				</button>
				<button class="abort" data-testid="run-pill-abort" disabled={!away.actions.abort.enabled} onclick={() => away.abort()} title="Abort the run now ({isMac ? 'Cmd' : 'Ctrl'} and period)">Abort</button>
			</span>
		{/if}
		{#if prov}<ProvenanceBadge source={prov.source} cls={prov.cls} compact />{/if}
		{#if app.units.systems.length}
			<select class="field units" data-testid="unit-system" aria-label="Display units" title="Display units: every plot and reading is drawn in them"
				value={app.units.name} onchange={(e) => app.units.choose(e.currentTarget.value)}>
				{#each app.units.systems as name (name)}<option value={name}>{name}</option>{/each}
			</select>
		{/if}
		<ConnectionPill client={app.analysis} label="Analysis" testid="conn-analysis" />
		<ConnectionPill client={app.control} label="Control" testid="conn-control" />
		<button class="btn ghost sm themebtn" data-testid="theme-toggle" onclick={() => theme.cycle()} title="Theme: {themeLabel}. Click to change." aria-label="Theme: {themeLabel}">
			<Icon name={theme.pref === 'system' ? 'system' : theme.pref === 'dark' ? 'moon' : 'sun'} size={16} />
			<span>{themeLabel}</span>
		</button>
	</div>
</header>

<style>
	.units {
		width: auto;
		max-width: 190px;
		font-size: var(--text-xs);
	}

	.topbar {
		display: grid;
		/* The right side keeps its full width; the brand and the search give way to it. */
		grid-template-columns: minmax(24px, 1fr) minmax(36px, 400px) max-content;
		align-items: center;
		gap: var(--sp-4);
		height: var(--topbar-height);
		padding: 0 var(--sp-3) 0 var(--sp-4);
		background: var(--bg-1);
		border-bottom: 1px solid var(--line-1);
	}

	.brand {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
		white-space: nowrap;
	}

	.name {
		font-weight: 760;
		letter-spacing: 0.06em;
		font-size: var(--text-sm);
	}

	.sep {
		color: var(--text-4);
	}

	.ws {
		color: var(--text-2);
		font-size: var(--text-sm);
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.trigger {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		height: 30px;
		padding: 0 6px 0 10px;
		color: var(--text-3);
		background: var(--bg-2);
		border: 1px solid var(--line-2);
		border-radius: var(--radius-md);
		transition: border-color var(--dur-fast) var(--ease);
	}

	.trigger:hover {
		border-color: var(--line-3);
		color: var(--text-2);
	}

	.ph {
		flex: 1;
		text-align: left;
		font-size: var(--text-sm);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.keys {
		display: inline-flex;
		gap: 3px;
	}

	.right > :global(*) {
		flex-shrink: 0;
	}

	.right {
		display: flex;
		align-items: center;
		justify-content: flex-end;
		gap: var(--sp-2);
		min-width: 0;
	}

	.mock {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		height: 22px;
		padding: 0 8px;
		font-size: var(--text-2xs);
		font-weight: var(--weight-strong);
		letter-spacing: 0.06em;
		text-transform: uppercase;
		color: var(--text-2);
		border: 1px dashed var(--line-3);
		border-radius: 11px;
		white-space: nowrap;
	}

	.runpill {
		display: inline-flex;
		align-items: stretch;
		height: 26px;
		overflow: hidden;
		border: 1px solid var(--blue-line);
		border-radius: var(--radius-md);
		font-size: var(--text-xs);
	}

	.runpill .go {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		padding: 0 9px;
		color: var(--blue-1);
		background: var(--blue-weak);
		border: 0;
		white-space: nowrap;
	}

	.runpill .lv {
		margin-left: 2px;
		padding-left: 7px;
		border-left: 1px solid var(--blue-line);
	}

	.runpill .go svg {
		animation: pulse 1.2s ease-in-out infinite;
	}

	.runpill .abort {
		padding: 0 10px;
		font-weight: var(--weight-strong);
		color: var(--status-fail-text);
		background: var(--status-fail-fill);
		border: 0;
	}

	@keyframes pulse {
		50% {
			opacity: 0.35;
		}
	}

	.themebtn span {
		min-width: 5ch;
		text-align: left;
	}

	@media (max-width: 1020px) {
		.ph,
		.keys,
		.sep,
		.ws,
		.name,
		.runpill .lv {
			display: none;
		}

		.trigger {
			justify-content: center;
			padding: 0;
		}
	}

	@media (max-width: 1180px) {
		.themebtn span,
		.mock {
			display: none;
		}
	}

	/* Narrower still, the data-source badge keeps its SIM or LIVE stamp and drops the words beside it. */
	@media (max-width: 860px) {
		.right :global(.sim .text),
		.right :global(.live .text) {
			display: none;
		}
	}
</style>
