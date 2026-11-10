<script lang="ts">
	/**
	 * A display window: one page of a run, opened from the main window to place on another monitor.
	 * It observes the run with a subscription of its own, shows the run's state and level, and keeps
	 * Stop and Abort in reach (Cmd or Ctrl and period aborts from here too). Its page can be switched
	 * and its plots changed like the main window's; the layout is shared between windows.
	 */
	import { onMount } from 'svelte';
	import '$lib/acquire/acquire.css';
	import RunBar from '$lib/acquire/RunBar.svelte';
	import RunView from '$lib/acquire/RunView.svelte';
	import ScreenHeader from '$lib/acquire/screen/ScreenHeader.svelte';
	import MimoRunView from '$lib/acquire/mimo/views/MimoRunView.svelte';
	import ShockRunView from '$lib/acquire/shock/ShockRunView.svelte';
	import SrsRunView from '$lib/acquire/srs/SrsRunView.svelte';
	import SineRunView from '$lib/acquire/sine/SineRunView.svelte';
	import TwrRunView from '$lib/acquire/twr/TwrRunView.svelte';
	import ExplorerRunView from '$lib/acquire/explorer/ExplorerRunView.svelte';
	import RecorderRunView from '$lib/acquire/recorder/RecorderRunView.svelte';
	import { runBandsOf, runGeometryOf } from '$lib/acquire/mimo/runFrame';
	import type { AppState } from '$lib/app/state.svelte';
	import { theme } from '$lib/theme/theme.svelte';
	import ConnectionPill from './ConnectionPill.svelte';
	import Icon from './Icon.svelte';

	interface Props {
		app: AppState;
	}
	let { app }: Props = $props();

	const mimoRun = $derived(app.config.popoutTest === 'mimo_random');
	const shockRun = $derived(app.config.popoutTest === 'shock');
	const srsRun = $derived(app.config.popoutTest === 'srs');
	const sineRun = $derived(app.config.popoutTest === 'sine');
	const twrRun = $derived(app.config.popoutTest === 'twr');
	const explorerRun = $derived(app.config.popoutTest === 'explorer');
	const recorderRun = $derived(app.config.popoutTest === 'recorder');
	const randomRun = $derived(!mimoRun && !shockRun && !srsRun && !sineRun && !twrRun && !explorerRun && !recorderRun);
	const mimo = $derived(app.mimo);
	const shock = $derived(app.shock);
	const srs = $derived(app.srs);
	const sine = $derived(app.sine);
	const twr = $derived(app.twr);
	const explorer = $derived(app.explorer);
	const recorder = $derived(app.recorder);
	const views = $derived(app.mimoViews);
	const acq = $derived(app.acquire);
	const display = $derived(app.display);
	const testName = $derived(mimoRun ? 'MIMO Random' : shockRun ? 'Classical Shock' : srsRun ? 'SRS' : sineRun ? 'Sine' : twrRun ? 'TWR' : explorerRun ? 'Explorer' : recorderRun ? 'Recorder' : 'Random');
	const pageName = $derived(mimoRun ? views.page.name : shockRun ? app.shockViews.page.name : srsRun ? app.srsViews.page.name : sineRun ? app.sineViews.page.name : twrRun ? app.twrViews.page.name : explorerRun ? app.explorerViews.page.name : recorderRun ? app.recorderViews.page.name : display.page.name);
	const isMac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
	const abortKeys = isMac ? 'Cmd .' : 'Ctrl .';
	const themeLabel = $derived(theme.pref === 'system' ? 'System' : theme.pref === 'dark' ? 'Dark' : 'Light');

	onMount(() => {
		theme.init(app.config.theme);
		app.startDisplayWindow();
	});

	function onKey(e: KeyboardEvent) {
		if ((e.metaKey || e.ctrlKey) && !e.altKey && !e.shiftKey && (e.key === '.' || e.code === 'Period')) {
			if (mimoRun) {
				if (mimo.actions.abort.enabled) {
					e.preventDefault();
					void mimo.abort();
				}
			} else if (shockRun) {
				if (shock.actions.abort.enabled) {
					e.preventDefault();
					void shock.abort();
				}
			} else if (srsRun) {
				if (srs.actions.abort.enabled) {
					e.preventDefault();
					void srs.abort();
				}
			} else if (sineRun) {
				if (sine.actions.abort.enabled) {
					e.preventDefault();
					void sine.abort();
				}
			} else if (twrRun) {
				if (twr.actions.abort.enabled) {
					e.preventDefault();
					void twr.abort();
				}
			} else if (explorerRun) {
				if (explorer.actions.abort.enabled) {
					e.preventDefault();
					void explorer.abort();
				}
			} else if (recorderRun) {
				if (recorder.actions.abort.enabled) {
					e.preventDefault();
					void recorder.abort();
				}
			} else if (acq.actions.abort.enabled) {
				e.preventDefault();
				void acq.abort();
			}
		}
	}
</script>

<svelte:window onkeydown={onKey} />
<svelte:head><title>{pageName} | {testName} | Hertzforge</title></svelte:head>

<div class="window" data-testid="display-window">
	<header class="bar">
		<div class="brand">
			<svg class="mark" viewBox="0 0 24 24" width="20" height="20" aria-hidden="true">
				<rect x="0.75" y="0.75" width="22.5" height="22.5" rx="6" fill="var(--bg-3)" stroke="var(--line-2)" />
				<path d="M4 13.5c2.2 0 2.2-6 4.4-6s2.2 10 4.4 10 2.2-7 4.4-7H20" fill="none" stroke="var(--blue-2)" stroke-width="1.8" stroke-linecap="round" />
			</svg>
			<span class="name">Hertzforge</span>
			<span class="sep" aria-hidden="true">/</span>
			<span>{testName}</span>
			<span class="sep" aria-hidden="true">/</span>
			<span class="page" data-testid="display-window-page">{pageName}</span>
		</div>
		<div class="right">
			<ConnectionPill client={app.control} label="Control" testid="conn-control" />
			<button class="btn ghost sm" onclick={() => theme.cycle()} title="Theme: {themeLabel}. Click to change." aria-label="Theme: {themeLabel}">
				<Icon name={theme.pref === 'system' ? 'system' : theme.pref === 'dark' ? 'moon' : 'sun'} size={15} />
			</button>
		</div>
	</header>
	{#if randomRun}
		<ScreenHeader {acq} name="Random" {abortKeys} compact ends={acq.isLive && !display.cardInView('run')} />
	{:else}
		<div class="top">
			<RunBar acq={mimoRun ? mimo : shockRun ? shock : srsRun ? srs : sineRun ? sine : twrRun ? twr : explorerRun ? explorer : recorderRun ? recorder : acq} onoverride={() => {}} {abortKeys} compact />
		</div>
	{/if}
	<div class="body" class:screen={randomRun}>
		{#if mimoRun}
			<MimoRunView {views} compact onpopout={(id) => app.openDisplayWindow(id, 'mimo_random')} geometry={runGeometryOf(mimo.setup, mimo.status)} bands={runBandsOf(mimo.status)} diagnostics={mimo.law?.diagnostics ?? []} />
		{:else if shockRun}
			<ShockRunView {shock} views={app.shockViews} compact onpopout={(id) => app.openDisplayWindow(id, 'shock')} />
		{:else if srsRun}
			<SrsRunView {srs} views={app.srsViews} compact onpopout={(id) => app.openDisplayWindow(id, 'srs')} />
		{:else if sineRun}
			<SineRunView {sine} views={app.sineViews} compact onpopout={(id) => app.openDisplayWindow(id, 'sine')} />
		{:else if twrRun}
			<TwrRunView {twr} views={app.twrViews} compact onpopout={(id) => app.openDisplayWindow(id, 'twr')} />
		{:else if explorerRun}
			<ExplorerRunView {explorer} views={app.explorerViews} compact onpopout={(id) => app.openDisplayWindow(id, 'explorer')} />
		{:else if recorderRun}
			<RecorderRunView {recorder} views={app.recorderViews} compact onpopout={(id) => app.openDisplayWindow(id, 'recorder')} />
		{:else}
			<RunView {acq} {display} compact {abortKeys} onpopout={(id) => app.openDisplayWindow(id)} />
		{/if}
	</div>
</div>

<style>
	.window {
		display: grid;
		grid-template-rows: auto auto minmax(0, 1fr);
		height: 100vh;
		height: 100dvh;
		background: var(--bg-0);
	}

	.bar {
		display: flex;
		align-items: center;
		gap: var(--sp-3);
		height: 40px;
		padding: 0 var(--sp-3);
		background: var(--bg-1);
		border-bottom: 1px solid var(--line-1);
	}

	.brand {
		display: flex;
		flex: 1;
		align-items: center;
		gap: 6px;
		min-width: 0;
		font-size: var(--text-sm);
		color: var(--text-2);
		white-space: nowrap;
	}

	.name,
	.page {
		font-weight: var(--weight-strong);
		color: var(--text-1);
	}

	.sep {
		color: var(--text-4);
	}

	.right {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
	}

	.top {
		padding: var(--sp-2) var(--sp-3) 0;
	}

	.body {
		display: flex;
		flex-direction: column;
		min-height: 0;
		padding: var(--sp-2) var(--sp-3) var(--sp-3);
		overflow: hidden;
	}

	.body.screen {
		padding: 6px 6px 4px var(--sp-3);
	}
</style>
