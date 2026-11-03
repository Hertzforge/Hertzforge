<script lang="ts">
	/** The shell frame: top bar, workspace rail, the active workspace, and the command palette. */
	import { onMount } from 'svelte';
	import { getApp } from '$lib/app/state.svelte';
	import { listenForDrops } from '$lib/project/files';
	import { WORKSPACES, workspaceInfo } from '$lib/app/workspaces';
	import { theme } from '$lib/theme/theme.svelte';
	import Acquire from '$lib/workspaces/Acquire.svelte';
	import Analyze from '$lib/workspaces/Analyze.svelte';
	import Modal from '$lib/workspaces/Modal.svelte';
	import Simulate from '$lib/workspaces/Simulate.svelte';
	import CommandPalette from './CommandPalette.svelte';
	import DisplayWindow from './DisplayWindow.svelte';
	import Placeholder from './Placeholder.svelte';
	import Model from '$lib/workspaces/Model.svelte';
	import Specify from '$lib/workspaces/Specify.svelte';
	import Report from '$lib/workspaces/Report.svelte';
	import Rail from './Rail.svelte';
	import TopBar from './TopBar.svelte';

	const app = getApp();

	onMount(() => {
		if (app.config.popout) return;
		theme.init(app.config.theme);
		app.start();
		// files dropped anywhere on the window open or import by their paths (the desktop app)
		return listenForDrops((paths) => {
			if (paths) return void app.project.openPaths(paths);
			app.go('analyze');
			app.project.error = 'A browser gives the shell a dropped file\'s contents but not its path. Drop files on the desktop app, or give the path under Project.';
		});
	});

	// each time the analysis service connects it is told the display units chosen before
	$effect(() => {
		if (!app.config.popout && app.analysis.isOpen) void app.units.sync();
	});

	function onKey(e: KeyboardEvent) {
		// A display window keeps its own keys (it aborts on Cmd or Ctrl and period too).
		if (app.config.popout) return;
		// Abort a live run from anywhere in the shell, whatever has focus (Cmd or Ctrl and period).
		if ((e.metaKey || e.ctrlKey) && !e.altKey && !e.shiftKey && (e.key === '.' || e.code === 'Period')) {
			if (app.acquire.actions.abort.enabled) {
				e.preventDefault();
				void app.acquire.abort();
			}
			return;
		}
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
<svelte:head>{#if !app.config.popout}<title>{info.title} | Hertzforge</title>{/if}</svelte:head>

{#if app.config.popout}
	<DisplayWindow {app} />
{:else}
<div class="shell">
	<TopBar {app} />
	<div class="body" class:collapsed={app.railCollapsed}>
		{#if !app.railCollapsed}<Rail current={app.workspace} onselect={(ws) => app.go(ws)} />{/if}
		<div class="workspace">
			{#if app.workspace === 'acquire'}
				<Acquire {app} />
			{:else if app.workspace === 'analyze'}
				<Analyze {app} />
			{:else if app.workspace === 'modal'}
				<Modal {app} />
			{:else if app.workspace === 'simulate'}
				<Simulate {app} />
			{:else if app.workspace === 'model'}
				<Model {app} />
			{:else if app.workspace === 'specify'}
				<Specify {app} />
			{:else if app.workspace === 'report'}
				<Report {app} />
			{:else}
				<Placeholder {info} />
			{/if}
		</div>
	</div>
	<CommandPalette {app} />
</div>
{/if}

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

	.body.collapsed {
		grid-template-columns: minmax(0, 1fr);
	}

	.workspace {
		min-width: 0;
		min-height: 0;
		overflow: hidden;
	}
</style>
