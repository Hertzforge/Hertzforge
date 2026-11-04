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
	import Report from '$lib/workspaces/Report.svelte';
	import Rail from './Rail.svelte';
	import RunLogPanel from '$lib/runlog/RunLogPanel.svelte';
	import TopBar from './TopBar.svelte';

	const app = getApp();

	onMount(() => {
		if (app.config.popout) return;
		theme.init(app.config.theme);
		app.start();
		// files dropped anywhere on the window open or import by their paths (the desktop app)
		return listenForDrops((paths) => {
			if (paths) return void app.project.openPaths(paths);
			app.go('signals');
			app.project.error = 'A browser gives the shell a dropped file\'s contents but not its path. Drop files on the desktop app, or give the path under Project.';
		});
	});

	// each time the analysis service connects it is told the display units chosen before
	$effect(() => {
		if (!app.config.popout && app.analysis.isOpen) void app.units.sync();
	});

	// a log opened before the service reconnected is shown again as the service holds it
	$effect(() => {
		if (!app.config.popout && app.analysis.isOpen && app.runLog.shown) void app.runLog.sync();
	});

	/** Drag the run log's edge to size it beside the workspace. */
	function resizeLog(e: PointerEvent) {
		const handle = e.currentTarget as HTMLElement;
		handle.setPointerCapture(e.pointerId);
		const right = window.innerWidth;
		const move = (m: PointerEvent) => app.runLog.resize(right - m.clientX);
		const up = () => {
			handle.removeEventListener('pointermove', move);
			handle.removeEventListener('pointerup', up);
		};
		handle.addEventListener('pointermove', move);
		handle.addEventListener('pointerup', up);
	}

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
		if (e.altKey && !e.metaKey && !e.ctrlKey && /^Digit[1-9]$/.test(e.code)) {
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
	<div class="body" class:collapsed={app.railCollapsed} class:logged={app.runLog.shown} class:alone={app.runLog.shown && app.runLog.alone} style:--runlog-width="{app.runLog.width}px">
		{#if !app.railCollapsed}<Rail current={app.workspace} onselect={(ws) => app.go(ws)} />{/if}
		<div class="workspace" hidden={app.runLog.shown && app.runLog.alone}>
			{#if app.workspace === 'acquire'}
				<Acquire {app} />
			{:else if app.workspace === 'simulate'}
				<Simulate {app} />
			{:else if app.workspace === 'modal'}
				<Modal {app} />
			{:else if app.workspace === 'report'}
				<Report {app} />
			{:else}
				<Analyze {app} only={app.workspace} />
			{/if}
		</div>
		{#if app.runLog.shown}
			<div class="logside">
				{#if !app.runLog.alone}
					<!-- svelte-ignore a11y_no_static_element_interactions (a pointer handle; the width is kept between launches) -->
					<div class="edge" title="Drag to size the run log" onpointerdown={resizeLog}></div>
				{/if}
				<RunLogPanel rl={app.runLog} />
			</div>
		{/if}
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

	.body.logged {
		grid-template-columns: var(--rail-width) minmax(0, 1fr) var(--runlog-width);
	}

	.body.logged.collapsed {
		grid-template-columns: minmax(0, 1fr) var(--runlog-width);
	}

	.body.logged.alone {
		grid-template-columns: var(--rail-width) minmax(0, 1fr);
	}

	.body.logged.alone.collapsed {
		grid-template-columns: minmax(0, 1fr);
	}

	.logside {
		position: relative;
		min-width: 0;
		min-height: 0;
	}

	.edge {
		position: absolute;
		top: 0;
		bottom: 0;
		left: -3px;
		z-index: 2;
		width: 6px;
		cursor: col-resize;
	}

	.workspace {
		min-width: 0;
		min-height: 0;
		overflow: hidden;
	}
</style>
