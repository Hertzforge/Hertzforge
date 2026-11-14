<script lang="ts">
	/**
	 * The operator's screen for a closed-loop Random test, in two steps as Eigen ran it: Setup
	 * (station and channels, profile and schedule, control and limits, one section at a time, with
	 * Apply Setup and "Proceed to Run" in the bar at the bottom), then Run (the pages of plots, and
	 * the control dock with the run's verbs, level, numbers and log). One header line heads both:
	 * the way back to the test types while no run is live, the steps, the run's state, clock and
	 * level, where the data comes from, and who holds control. When a run starts driving the Run
	 * step comes forward; the Setup step shows the setup (locked while the run is live).
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
	import RunRecordEditor from '$lib/acquire/RunRecordEditor.svelte';
	import ScheduleEditor from '$lib/acquire/ScheduleEditor.svelte';
	import SetupSummary from '$lib/acquire/SetupSummary.svelte';
	import StationPanel from '$lib/acquire/StationPanel.svelte';
	import ScreenHeader from '$lib/acquire/screen/ScreenHeader.svelte';
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
	let heading = $state<HTMLElement | null>(null);

	// Arriving from the chooser or the run pill, the keyboard lands on the screen's name.
	onMount(() => {
		if (app.tests.takeFocus()) heading?.focus();
	});

	const TEST_KEYS = ['title', 'profile', 'acquisition', 'schedule', 'specification'];
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
	/** Stop and Abort stay in reach: in the header whenever a live run's Run card is out of view. */
	const endsInHeader = $derived(acq.isLive && !(running && display.cardInView('run')));
	/** What the state chip says on hover: how the run ended, why it holds, or what the next step waits on. */
	const reason = $derived.by(() => {
		const run = acq.run;
		if (run?.state === 'finished' && run.end) return run.end.message;
		if (run?.state === 'review') return 'Nothing drives at test level until you proceed.';
		const p = acq.primary;
		return p && acq.actions[p].visible && !acq.actions[p].enabled ? acq.actions[p].reason : null;
	});
	const apply = $derived(acq.actions.apply);
	let applying = $state(false);
	async function applySetup() {
		if (!apply.enabled || applying) return;
		applying = true;
		await acq.apply();
		applying = false;
	}
</script>

<div class="acquire" data-testid="acquire">
	<ScreenHeader
		{acq}
		name="Random"
		step={running ? 'run' : 'setup'}
		onsetup={toSetup}
		onrun={proceed}
		runWhy={proceedWhy}
		onleave={tests.canLeave ? () => tests.toChooser() : null}
		{reason}
		ends={endsInHeader}
		{abortKeys}
		bind:heading
	/>
	<!-- Notices float over the top of the page, so one never pushes the plots down. -->
	<div class="notices"><div class="float"><Notices {acq} /></div></div>

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
					<RunRecordEditor {acq} />
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
		{#if apply.visible}
			<button class="btn" data-testid="act-apply" aria-disabled={!apply.enabled || applying} class:off={!apply.enabled} title={apply.reason ?? 'Send this setup to the control service.'} onclick={applySetup}>
				{#if applying}<svg class="spin" viewBox="0 0 12 12" width="12" height="12" aria-hidden="true"><path d="M6 1.5a4.5 4.5 0 104.5 4.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" /></svg>{/if}
				Apply Setup
			</button>
		{/if}
		<button class="btn primary" data-testid="proceed-to-run" aria-disabled={!!proceedWhy || proceeding} class:off={!!proceedWhy} title={proceedWhy ?? 'Show the run: the display pages, the level and the run\'s numbers.'} onclick={proceed}>
			Proceed to Run
			<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M4.5 2.5L8 6l-3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
		</button>
	</footer>
	{:else}
	<div class="run" id="aq-panel-live" data-testid="run-view">
		<RunView {acq} {display} onpopout={popout} onoverride={() => (overrideOpen = true)} {abortKeys} />
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

	.notices {
		position: relative;
		height: 0;
		z-index: 40;
	}

	.float {
		position: absolute;
		top: var(--sp-2);
		left: 50%;
		transform: translateX(-50%);
		width: min(640px, calc(100% - 32px));
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		pointer-events: none;
	}

	.float > :global(*) {
		pointer-events: auto;
		box-shadow: var(--shadow-pop);
	}

	/* The tinted notices are translucent: lay them on the page colour so a plot never shows through. */
	.float > :global(.aq-notice.warn) {
		background: linear-gradient(var(--amber-weak), var(--amber-weak)), var(--bg-2);
	}

	.float > :global(.aq-notice.pass) {
		background: linear-gradient(var(--blue-weak), var(--blue-weak)), var(--bg-2);
	}

	/* The run fills what the header leaves: the plots on their page, the controls beside them. */
	.run {
		flex: 1;
		min-height: 0;
		padding: 6px 6px 4px var(--sp-3);
	}

	.flowbar .spin {
		animation: spin 0.8s linear infinite;
	}

	@keyframes spin {
		to {
			transform: rotate(360deg);
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

	@media (max-width: 1180px) {
		.two {
			grid-template-columns: minmax(0, 1fr);
		}
	}

	@media (max-width: 760px) {
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
