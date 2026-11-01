<script lang="ts">
	/**
	 * The operator's screen for a closed-loop Random test. A link back to the test-type chooser
	 * heads it while no run is live. The run bar is always in view (where the data comes from, the
	 * run's state, who holds control, the verbs that apply now, Stop and Abort); below it, the
	 * station and channels, the profile and schedule, the control parameters, and the live run,
	 * one section at a time.
	 */
	import { onMount } from 'svelte';
	import '$lib/acquire/acquire.css';
	import type { AppState } from '$lib/app/state.svelte';
	import ChannelTable from '$lib/acquire/ChannelTable.svelte';
	import ControlEditor from '$lib/acquire/ControlEditor.svelte';
	import LiveView from '$lib/acquire/LiveView.svelte';
	import Notices from '$lib/acquire/Notices.svelte';
	import OverrideDialog from '$lib/acquire/OverrideDialog.svelte';
	import ProfileEditor from '$lib/acquire/ProfileEditor.svelte';
	import ReferencePreview from '$lib/acquire/ReferencePreview.svelte';
	import RunBar from '$lib/acquire/RunBar.svelte';
	import ScheduleEditor from '$lib/acquire/ScheduleEditor.svelte';
	import SetupSummary from '$lib/acquire/SetupSummary.svelte';
	import StationPanel from '$lib/acquire/StationPanel.svelte';
	import type { AcquireTab } from '$lib/acquire/store.svelte';

	interface Props {
		app: AppState;
	}
	let { app }: Props = $props();

	const acq = $derived(app.acquire);
	const tests = $derived(app.tests);
	let overrideOpen = $state(false);
	let heading: HTMLElement;

	// Arriving from the chooser or the run pill, the keyboard lands on the screen's name.
	onMount(() => {
		if (app.tests.takeFocus()) heading.focus();
	});

	const TEST_KEYS = ['title', 'profile', 'acquisition', 'schedule'];
	const CONTROL_KEYS = ['control', 'limits', 'feasibility'];

	function countUnder(prefixes: string[], map: Record<string, string[]>): number {
		return Object.entries(map)
			.filter(([k]) => prefixes.some((p) => k === p || k.startsWith(`${p}.`)))
			.reduce((n, [, v]) => n + v.length, 0);
	}

	const tabs = $derived.by(() => {
		const E = acq.fieldErrors;
		const W = acq.fieldWarnings;
		const tf = acq.findings;
		const rows = [...tf.table, ...[...tf.rows.values()].flat()];
		return [
			{ id: 'station' as const, label: 'Station & Channels', errors: rows.filter((f) => f.level === 'error').length, warnings: rows.filter((f) => f.level === 'warning').length },
			{ id: 'test' as const, label: 'Profile & Schedule', errors: countUnder(TEST_KEYS, E), warnings: countUnder(TEST_KEYS, W) },
			{ id: 'control' as const, label: 'Control & Limits', errors: countUnder(CONTROL_KEYS, E), warnings: countUnder(CONTROL_KEYS, W) },
			{ id: 'live' as const, label: 'Live', errors: 0, warnings: 0 }
		];
	});

	let tablist: HTMLElement;
	let body = $state<HTMLElement | null>(null);

	// Each section opens at its top.
	$effect(() => {
		void acq.tab;
		body?.scrollTo({ top: 0 });
	});

	function onTabKey(e: KeyboardEvent) {
		const ids = tabs.map((t) => t.id);
		const i = ids.indexOf(acq.tab);
		let next = -1;
		if (e.key === 'ArrowRight') next = (i + 1) % ids.length;
		else if (e.key === 'ArrowLeft') next = (i - 1 + ids.length) % ids.length;
		else if (e.key === 'Home') next = 0;
		else if (e.key === 'End') next = ids.length - 1;
		if (next < 0) return;
		e.preventDefault();
		select(ids[next]);
		tablist.querySelector<HTMLButtonElement>(`[data-tab="${ids[next]}"]`)?.focus();
	}

	function select(id: AcquireTab) {
		acq.tab = id;
	}

	const isMac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
	const abortKeys = isMac ? 'Cmd .' : 'Ctrl .';
	const liveDot = $derived(acq.run?.state === 'running' || acq.run?.state === 'review' || acq.run?.state === 'stopping');
	const ended = $derived(acq.run?.state === 'finished' ? (acq.run.end?.outcome ?? null) : null);
</script>

<div class="acquire" data-testid="acquire">
	<div class="top">
		<div class="crumb">
			{#if tests.canLeave}
				<button class="btn ghost sm back" data-testid="test-type-back" onclick={() => tests.toChooser()}>
					<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M7.5 2.5L4 6l3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
					Test type
				</button>
				<span class="sep" aria-hidden="true">/</span>
			{/if}
			<h1 class="here" tabindex="-1" bind:this={heading}>Random</h1>
		</div>
		<RunBar {acq} onoverride={() => (overrideOpen = true)} {abortKeys} />
		<Notices {acq} />
	</div>

	<div class="tabs" role="tablist" aria-label="Acquire and control sections" bind:this={tablist} tabindex="-1" onkeydown={onTabKey}>
		{#each tabs as t (t.id)}
			<button
				role="tab"
				id="aq-tab-{t.id}"
				data-tab={t.id}
				data-testid="tab-{t.id}"
				aria-selected={acq.tab === t.id}
				aria-controls="aq-panel-{t.id}"
				tabindex={acq.tab === t.id ? 0 : -1}
				class:on={acq.tab === t.id}
				onclick={() => select(t.id)}
			>
				{t.label}
				{#if t.errors > 0}
					<span class="mark err" title="{t.errors} {t.errors === 1 ? 'error' : 'errors'}">
						<svg viewBox="0 0 14 14" width="11" height="11" aria-hidden="true"><path d="M4.5 1h5L13 4.5v5L9.5 13h-5L1 9.5v-5z" fill="currentColor" /><path d="M5 5l4 4M9 5l-4 4" stroke="var(--status-fail-text)" stroke-width="1.7" stroke-linecap="round" /></svg>
						<span class="num">{t.errors}</span><span class="visually-hidden">{t.errors === 1 ? 'error' : 'errors'}</span>
					</span>
				{:else if t.warnings > 0}
					<span class="mark warn" title="{t.warnings} {t.warnings === 1 ? 'warning' : 'warnings'}">
						<svg viewBox="0 0 14 14" width="11" height="11" aria-hidden="true"><path d="M7 1.6l5.6 10H1.4z" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round" /></svg>
						<span class="num">{t.warnings}</span><span class="visually-hidden">{t.warnings === 1 ? 'warning' : 'warnings'}</span>
					</span>
				{/if}
				{#if t.id === 'live' && liveDot}
					<span class="mark dot" aria-label="running"><svg viewBox="0 0 10 10" width="8" height="8" aria-hidden="true"><circle cx="5" cy="5" r="4" fill="currentColor" /></svg></span>
				{:else if t.id === 'live' && ended}
					<span class="mark ended" aria-label="finished"><svg viewBox="0 0 10 10" width="8" height="8" aria-hidden="true"><rect x="1" y="1" width="8" height="8" rx="1" fill="currentColor" /></svg></span>
				{/if}
			</button>
		{/each}
		{#if acq.locked && acq.tab !== 'live'}
			<span class="locked" data-testid="setup-locked">
				<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><rect x="3" y="6.5" width="8" height="6" rx="1" fill="currentColor" /><path d="M4.8 6.5V4.8a2.2 2.2 0 014.4 0v1.7" fill="none" stroke="currentColor" stroke-width="1.4" /></svg>
				Locked while the run is live
			</span>
		{/if}
	</div>

	<div class="body" bind:this={body} class:fit={acq.tab === 'live'} id="aq-panel-{acq.tab}" role="tabpanel" aria-labelledby="aq-tab-{acq.tab}" tabindex="-1">
		{#if acq.tab === 'station'}
			<div class="stack">
				<StationPanel {acq} />
				<ChannelTable {acq} />
			</div>
		{:else if acq.tab === 'test'}
			<div class="two">
				<div class="stack">
					<ProfileEditor {acq} />
					<ReferencePreview {acq} />
				</div>
				<div class="stack">
					<SetupSummary {acq} />
					<ScheduleEditor {acq} />
				</div>
			</div>
		{:else if acq.tab === 'control'}
			<div class="narrow">
				<ControlEditor {acq} />
			</div>
		{:else}
			<LiveView {acq} />
		{/if}
	</div>
</div>

<OverrideDialog {acq} bind:open={overrideOpen} />

<style>
	.acquire {
		display: flex;
		flex-direction: column;
		height: 100%;
		min-height: 0;
	}

	.top {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		padding: var(--sp-2) var(--sp-5) 0;
	}

	.crumb {
		display: flex;
		align-items: center;
		gap: var(--sp-1);
		min-height: 24px;
		min-width: 0;
	}

	.crumb .back {
		gap: 4px;
		margin-left: calc(-1 * var(--sp-2));
		color: var(--text-2);
	}

	.crumb .sep {
		color: var(--text-4);
	}

	.here {
		font-size: var(--text-md);
		font-weight: var(--weight-strong);
		color: var(--text-1);
	}

	.here:focus {
		outline: none;
	}

	.tabs {
		display: flex;
		align-items: flex-end;
		gap: 2px;
		margin: var(--sp-3) var(--sp-5) 0;
		border-bottom: 1px solid var(--line-2);
		overflow-x: auto;
		scrollbar-width: none;
	}

	.tabs button {
		position: relative;
		display: inline-flex;
		align-items: center;
		gap: 6px;
		height: 34px;
		padding: 0 14px;
		font-size: var(--text-sm);
		font-weight: var(--weight-medium);
		color: var(--text-3);
		background: transparent;
		border: 0;
		border-radius: var(--radius-md) var(--radius-md) 0 0;
		white-space: nowrap;
	}

	.tabs button:hover {
		color: var(--text-1);
		background: var(--bg-2);
	}

	.tabs button.on {
		color: var(--text-1);
		font-weight: var(--weight-strong);
	}

	.tabs button.on::after {
		content: '';
		position: absolute;
		left: 8px;
		right: 8px;
		bottom: -1px;
		height: 3px;
		background: var(--blue-2);
		border-radius: 2px 2px 0 0;
	}

	.mark {
		display: inline-flex;
		align-items: center;
		gap: 3px;
		font-size: var(--text-2xs);
	}

	.mark.err {
		color: var(--amber-1);
	}

	.mark.warn {
		color: var(--amber-2);
	}

	.mark.dot {
		color: var(--blue-2);
		animation: pulse 1.2s ease-in-out infinite;
	}

	.mark.ended {
		color: var(--text-3);
	}

	.locked {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		margin: 0 0 8px auto;
		padding-left: var(--sp-3);
		font-size: var(--text-xs);
		color: var(--text-2);
		white-space: nowrap;
	}

	.body {
		flex: 1;
		min-height: 0;
		overflow-y: auto;
		padding: var(--sp-3) var(--sp-5) var(--sp-6);
	}

	.body:focus {
		outline: none;
	}

	/* The live view fills the window when there is room, so the charts and the numbers are all
	   in view during a run; on a small window it scrolls like the rest. */
	@media (min-height: 700px) and (min-width: 1101px) {
		.body.fit {
			display: flex;
			flex-direction: column;
			overflow: hidden;
			padding-bottom: var(--sp-3);
		}
	}

	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
	}

	.two {
		display: grid;
		grid-template-columns: minmax(0, 1.25fr) minmax(340px, 1fr);
		gap: var(--sp-3);
		align-items: start;
	}

	.narrow {
		max-width: 1180px;
	}

	@keyframes pulse {
		50% {
			opacity: 0.35;
		}
	}

	@media (max-width: 1180px) {
		.two {
			grid-template-columns: minmax(0, 1fr);
		}
	}

	@media (max-width: 760px) {
		.top,
		.body {
			padding-left: var(--sp-3);
			padding-right: var(--sp-3);
		}
		.tabs {
			margin-left: var(--sp-3);
			margin-right: var(--sp-3);
		}
	}
</style>
