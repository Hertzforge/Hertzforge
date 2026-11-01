<script lang="ts">
	/**
	 * The operator's screen for a closed-loop Random test, in two steps as Eigen ran it: Setup
	 * (station and channels, profile and schedule, control and limits, one section at a time, with
	 * "Proceed to Run" once the setup checks clean), then Run (the display pages and their plots,
	 * the level card, the run's numbers and log). A link back to the test-type chooser heads it
	 * while no run is live. The run bar is always in view: where the data comes from, the run's
	 * state, who holds control, the verbs that apply now, Stop and Abort. When a run starts driving
	 * the Run step comes forward; Back to Setup shows the setup (locked while the run is live).
	 */
	import { onMount } from 'svelte';
	import '$lib/acquire/acquire.css';
	import type { AppState } from '$lib/app/state.svelte';
	import ChannelTable from '$lib/acquire/ChannelTable.svelte';
	import ControlEditor from '$lib/acquire/ControlEditor.svelte';
	import RunView from '$lib/acquire/RunView.svelte';
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
	const display = $derived(app.display);
	const running = $derived(acq.tab === 'live');
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
			{ id: 'control' as const, label: 'Control & Limits', errors: countUnder(CONTROL_KEYS, E), warnings: countUnder(CONTROL_KEYS, W) }
		];
	});

	const setupErrors = $derived(tabs.reduce((n, t) => n + t.errors, 0));
	const setupWarnings = $derived(tabs.reduce((n, t) => n + t.warnings, 0));
	/** Why Proceed to Run waits, or null. An observer only looks, so it never waits. */
	const proceedWhy = $derived.by(() => {
		if (!acq.isController || acq.isLive) return null;
		if (acq.pending) return acq.pending;
		if (setupErrors > 0) return `Fix the ${setupErrors === 1 ? 'error' : `${setupErrors} errors`} in the setup first.`;
		if (acq.tableErrorCount > 0) return 'Fix the channel table first.';
		return null;
	});
	const flowNote = $derived.by(() => {
		if (acq.isLive) return { tone: 'info', text: 'The run is live; the setup is locked until it ends.' };
		if (!acq.isController) return { tone: 'info', text: 'Observing: the setup the service holds is shown.' };
		if (proceedWhy) return { tone: 'warn', text: proceedWhy };
		if (acq.setupStatus === 'applied') return { tone: 'pass', text: 'The service holds this setup.' };
		return { tone: 'info', text: `The setup checks clean${setupWarnings ? `, with ${setupWarnings} ${setupWarnings === 1 ? 'warning' : 'warnings'}` : ''}. Proceed to Run applies it.` };
	});
	let proceeding = $state(false);

	/** Setup to Run: apply the setup if this screen holds control and it changed, then show the run. */
	async function proceed() {
		if (proceedWhy || proceeding) return;
		if (acq.isController && !acq.isLive && acq.setupStatus !== 'applied' && acq.actions.apply.enabled) {
			proceeding = true;
			const ok = await acq.apply();
			proceeding = false;
			if (!ok) return;
		}
		acq.tab = 'live';
	}

	function toSetup() {
		if (acq.tab === 'live') acq.tab = 'station';
	}

	/** A page in a window of its own, for another monitor: an observer of the run with its own plots. */
	function popout(pageId: string): boolean {
		return app.openDisplayWindow(pageId);
	}

	let tablist = $state<HTMLElement | null>(null);
	let body = $state<HTMLElement | null>(null);

	// Each section opens at its top.
	$effect(() => {
		void acq.tab;
		body?.scrollTo({ top: 0 });
	});

	function onTabKey(e: KeyboardEvent) {
		const ids = tabs.map((t) => t.id);
		const i = ids.indexOf(acq.tab as (typeof ids)[number]);
		let next = -1;
		if (e.key === 'ArrowRight') next = (i + 1) % ids.length;
		else if (e.key === 'ArrowLeft') next = (i - 1 + ids.length) % ids.length;
		else if (e.key === 'Home') next = 0;
		else if (e.key === 'End') next = ids.length - 1;
		if (next < 0) return;
		e.preventDefault();
		select(ids[next]);
		tablist?.querySelector<HTMLButtonElement>(`[data-tab="${ids[next]}"]`)?.focus();
	}

	function select(id: AcquireTab) {
		acq.tab = id;
	}

	const isMac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
	const abortKeys = isMac ? 'Cmd .' : 'Ctrl .';
	const liveDot = $derived(acq.run?.state === 'running' || acq.run?.state === 'review' || acq.run?.state === 'stopping');
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
			<ol class="flow" aria-label="Steps">
				<li>
					<button class="step" class:on={!running} aria-current={!running ? 'step' : undefined} data-testid="flow-setup" onclick={toSetup}><span class="n">1</span>Setup</button>
				</li>
				<li aria-hidden="true" class="arrow">
					<svg viewBox="0 0 12 12" width="10" height="10"><path d="M4.5 2.5L8 6l-3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" /></svg>
				</li>
				<li>
					<button class="step" class:on={running} aria-current={running ? 'step' : undefined} data-testid="flow-run" aria-disabled={!running && !!proceedWhy} title={!running && proceedWhy ? proceedWhy : undefined} onclick={() => (running ? null : proceed())}>
						<span class="n">2</span>Run{#if liveDot}<span class="mark dot" aria-label="running"><svg viewBox="0 0 10 10" width="8" height="8" aria-hidden="true"><circle cx="5" cy="5" r="4" fill="currentColor" /></svg></span>{/if}
					</button>
				</li>
			</ol>
		</div>
		<RunBar {acq} onoverride={() => (overrideOpen = true)} {abortKeys} />
		<Notices {acq} />
	</div>

	{#if !running}
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
			</button>
		{/each}
		{#if acq.locked}
			<span class="locked" data-testid="setup-locked">
				<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><rect x="3" y="6.5" width="8" height="6" rx="1" fill="currentColor" /><path d="M4.8 6.5V4.8a2.2 2.2 0 014.4 0v1.7" fill="none" stroke="currentColor" stroke-width="1.4" /></svg>
				Locked while the run is live
			</span>
		{/if}
	</div>

	<div class="body" bind:this={body} id="aq-panel-{acq.tab}" role="tabpanel" aria-labelledby="aq-tab-{acq.tab}" tabindex="-1">
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
		{:else}
			<div class="narrow">
				<ControlEditor {acq} />
			</div>
		{/if}
	</div>

	<footer class="flowbar" data-testid="setup-footer">
		<p class="flownote {flowNote.tone}" data-testid="flow-check" aria-live="polite">
			{#if flowNote.tone === 'warn'}
				<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><path d="M7 1.6l5.6 10H1.4z" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round" /><path d="M7 5.4v3" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
			{:else if flowNote.tone === 'pass'}
				<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><path d="M2.5 7.3l2.8 2.8 6-6.3" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" /></svg>
			{/if}
			{flowNote.text}
		</p>
		<button class="btn primary" data-testid="proceed-to-run" aria-disabled={!!proceedWhy || proceeding} class:off={!!proceedWhy} title={proceedWhy ?? 'Show the run: the display pages, the level and the run\'s numbers.'} onclick={proceed}>
			Proceed to Run
			<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M4.5 2.5L8 6l-3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
		</button>
	</footer>
	{:else}
	<div class="body fit" id="aq-panel-live" data-testid="run-view">
		<RunView {acq} {display} onpopout={popout}>
			{#snippet end()}
				<button class="btn sm" data-testid="back-to-setup" onclick={toSetup} title={acq.isLive ? 'Look at the setup. It stays locked while the run is live.' : 'Back to the setup.'}>
					<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M7.5 2.5L4 6l3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
					Back to Setup
				</button>
			{/snippet}
		</RunView>
	</div>
	{/if}
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

	.flow {
		display: flex;
		align-items: center;
		gap: 2px;
		margin: 0 0 0 var(--sp-3);
		padding: 0;
		list-style: none;
	}

	.flow .arrow {
		display: inline-flex;
		color: var(--text-4);
	}

	.step {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		height: 26px;
		padding: 0 10px 0 4px;
		font-size: var(--text-sm);
		font-weight: var(--weight-medium);
		color: var(--text-3);
		background: transparent;
		border: 1px solid transparent;
		border-radius: 13px;
	}

	.step:hover {
		color: var(--text-1);
		background: var(--bg-2);
	}

	.step .n {
		display: inline-grid;
		place-items: center;
		width: 18px;
		height: 18px;
		font-size: var(--text-2xs);
		font-weight: var(--weight-strong);
		border: 1.3px solid currentColor;
		border-radius: 50%;
	}

	.step.on {
		color: var(--text-1);
		font-weight: var(--weight-strong);
		background: var(--blue-weak);
		border-color: var(--blue-line);
	}

	.step.on .n {
		color: var(--bg-1);
		background: var(--blue-2);
		border-color: var(--blue-2);
	}

	.step[aria-disabled='true'] {
		cursor: default;
	}

	.flowbar {
		display: flex;
		align-items: center;
		gap: var(--sp-3);
		padding: var(--sp-2) var(--sp-5);
		background: var(--bg-1);
		border-top: 1px solid var(--line-1);
	}

	.flownote {
		display: inline-flex;
		flex: 1;
		align-items: center;
		gap: 6px;
		min-width: 0;
		font-size: var(--text-sm);
		color: var(--text-2);
	}

	.flownote.warn {
		font-weight: var(--weight-strong);
		color: var(--amber-1);
	}

	.flownote.pass {
		color: var(--blue-1);
	}

	.flowbar .btn.off {
		opacity: 0.55;
		cursor: default;
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

	/* The run fills the window when there is room, so the plots and the numbers are all in view
	   during a run; on a small window it scrolls like the rest. */
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
