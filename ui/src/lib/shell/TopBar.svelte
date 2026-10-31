<script lang="ts">
	/** Top bar: where you are, the palette trigger, the SIM badge, service state and theme. */
	import type { AppState } from '$lib/app/state.svelte';
	import { workspaceInfo } from '$lib/app/workspaces';
	import { theme } from '$lib/theme/theme.svelte';
	import ConnectionPill from './ConnectionPill.svelte';
	import Icon from './Icon.svelte';
	import SimBadge from './SimBadge.svelte';

	interface Props {
		app: AppState;
	}
	let { app }: Props = $props();

	const isMac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
	const themeLabel = $derived(theme.pref === 'system' ? 'System' : theme.pref === 'dark' ? 'Dark' : 'Light');
</script>

<header class="topbar">
	<div class="brand">
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
		{#if app.simOnScreen}<SimBadge compact />{/if}
		<ConnectionPill client={app.analysis} label="Analysis" testid="conn-analysis" />
		<ConnectionPill client={app.control} label="Control" testid="conn-control" />
		<button class="btn ghost sm themebtn" data-testid="theme-toggle" onclick={() => theme.cycle()} title="Theme: {themeLabel}. Click to change." aria-label="Theme: {themeLabel}">
			<Icon name={theme.pref === 'system' ? 'system' : theme.pref === 'dark' ? 'moon' : 'sun'} size={16} />
			<span>{themeLabel}</span>
		</button>
	</div>
</header>

<style>
	.topbar {
		display: grid;
		grid-template-columns: minmax(24px, 1fr) minmax(36px, 400px) auto;
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

	.themebtn span {
		min-width: 5ch;
		text-align: left;
	}

	@media (max-width: 1020px) {
		.ph,
		.keys,
		.sep,
		.ws,
		.name {
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
</style>
