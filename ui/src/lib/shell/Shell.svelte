<script lang="ts">
	/** The shell frame: top bar, workspace rail, the active workspace, and the command palette. */
	import { onMount } from 'svelte';
	import { getApp } from '$lib/app/state.svelte';
	import { WORKSPACES, workspaceInfo } from '$lib/app/workspaces';
	import { theme } from '$lib/theme/theme.svelte';
	import Analyze from '$lib/workspaces/Analyze.svelte';
	import Modal from '$lib/workspaces/Modal.svelte';
	import Simulate from '$lib/workspaces/Simulate.svelte';
	import CommandPalette from './CommandPalette.svelte';
	import Placeholder from './Placeholder.svelte';
	import Rail from './Rail.svelte';
	import TopBar from './TopBar.svelte';

	const app = getApp();

	onMount(() => {
		theme.init(app.config.theme);
		app.start();
	});

	function onKey(e: KeyboardEvent) {
		if ((e.metaKey || e.ctrlKey) && !e.altKey && e.key.toLowerCase() === 'k') {
			e.preventDefault();
			app.paletteOpen = !app.paletteOpen;
			return;
		}
		if (e.altKey && !e.metaKey && !e.ctrlKey && /^Digit[1-7]$/.test(e.code)) {
			e.preventDefault();
			app.go(WORKSPACES[Number(e.code.slice(5)) - 1].id);
		}
	}

	const info = $derived(workspaceInfo(app.workspace));
</script>

<svelte:window onkeydown={onKey} />
<svelte:head><title>{info.title} | Hertzforge</title></svelte:head>

<div class="shell">
	<TopBar {app} />
	<div class="body">
		<Rail current={app.workspace} onselect={(ws) => app.go(ws)} />
		<div class="workspace">
			{#if app.workspace === 'analyze'}
				<Analyze {app} />
			{:else if app.workspace === 'modal'}
				<Modal {app} />
			{:else if app.workspace === 'simulate'}
				<Simulate {app} />
			{:else}
				<Placeholder {info} />
			{/if}
		</div>
	</div>
	<CommandPalette {app} />
</div>

<style>
	.shell {
		display: grid;
		grid-template-rows: var(--topbar-height) minmax(0, 1fr);
		height: 100vh;
		height: 100dvh;
	}

	.body {
		display: grid;
		grid-template-columns: var(--rail-width) minmax(0, 1fr);
		min-height: 0;
	}

	.workspace {
		min-width: 0;
		min-height: 0;
		overflow: hidden;
	}
</style>
