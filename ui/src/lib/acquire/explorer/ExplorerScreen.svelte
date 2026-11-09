<script lang="ts">
	/**
	 * The operator's screen for Explorer, in the format the other test types set: a link back to the
	 * test-type chooser, two steps (Setup, then Run), the run bar always in view, and the setup in
	 * sections with its summary beside every one. Start opens the station and starts the tone at
	 * 0 V in one step and brings the run forward; there is no review hold. The tone is steered from
	 * the steering panel beside the run's pages; Hold and Clear Survey sit in the run bar beside
	 * the shared verbs, and the bar shows the tone's frequency and drive in place of a level.
	 */

	import { onMount } from 'svelte';
	import '$lib/acquire/acquire.css';
	import type { AppState } from '$lib/app/state.svelte';
	import ChannelTable from '../ChannelTable.svelte';
	import Notices from '../Notices.svelte';
	import RunBar from '../RunBar.svelte';
	import StationPanel from '../StationPanel.svelte';
	import ExplorerCapture from './ExplorerCapture.svelte';
	import ExplorerRunVerbs from './ExplorerRunVerbs.svelte';
	import ExplorerRunView from './ExplorerRunView.svelte';
	import ExplorerSetup from './ExplorerSetup.svelte';
	import ExplorerSummary from './ExplorerSummary.svelte';
	import { EXPLORER_SECTIONS, explorerChannels, type ExplorerSection } from './explorerSetup';
	import { hz, sweepWords, vpk } from './explorerWords';

	interface Props {
		app: AppState;
	}
	let { app }: Props = $props();

	const explorer = $derived(app.explorer);
	const views = $derived(app.explorerViews);
	const tests = $derived(app.tests);
	let heading: HTMLElement;

	onMount(() => {
		if (app.tests.takeFocus()) heading.focus();
	});

	const tabs = $derived(EXPLORER_SECTIONS.map((s) => ({ ...s, errors: explorer.counts[s.id].errors, warnings: explorer.counts[s.id].warnings })));
	const section = $derived(explorer.section);
	const step = $derived(explorer.step);
	const now = $derived(explorer.now);
	const liveDot = $derived(now?.state === 'running' || now?.state === 'starting' || now?.state === 'stopping');
	const channels = $derived(explorerChannels(explorer.rows));
	const sweep = $derived(sweepWords(now?.sweep ?? null, now?.frequency_hz ?? null, now?.band_hz ?? null));

	/** Why Proceed to Run waits, or null. An observer only looks, so it never waits. */
	const proceedWhy = $derived.by(() => {
		if (!explorer.isController || explorer.isLive) return null;
		if (explorer.setupStatus === 'applied' || explorer.setupStatus === 'unread') return null;
		const a = explorer.actions.apply;
		return a.enabled ? null : a.reason;
	});
	const flowNote = $derived.by(() => {
		if (explorer.isLive) return { tone: 'info', text: 'The run is live; the setup is locked until it ends. The steering steps can still change.' };
		if (!explorer.isController) return { tone: 'info', text: 'Observing: the run the service holds is shown.' };
		if (proceedWhy) return { tone: 'warn', text: proceedWhy };
		if (explorer.setupStatus === 'applied') return { tone: 'pass', text: 'The service holds this setup.' };
		const w = Object.values(explorer.counts).reduce((n, c) => n + c.warnings, 0);
		return { tone: 'info', text: `The setup checks clean${w ? `, with ${w} ${w === 1 ? 'warning' : 'warnings'}` : ''}. Proceed to Run applies it.` };
	});
	let proceeding = $state(false);

	async function proceed() {
		if (proceedWhy || proceeding) return;
		if (explorer.isController && !explorer.isLive && explorer.setupStatus !== 'applied' && explorer.setupStatus !== 'unread' && explorer.actions.apply.enabled) {
			proceeding = true;
			const ok = await explorer.apply();
			proceeding = false;
			if (!ok) return;
		}
		explorer.step = 'run';
	}

	function open(s: ExplorerSection) {
		explorer.step = 'setup';
		explorer.section = s;
	}

	let tablist = $state<HTMLElement | null>(null);
	let body = $state<HTMLElement | null>(null);

	$effect(() => {
		void explorer.section;
		void explorer.step;
		body?.scrollTo({ top: 0 });
	});

	function onTabKey(e: KeyboardEvent) {
		const ids = EXPLORER_SECTIONS.map((t) => t.id);
		const i = ids.indexOf(section);
		let next = -1;
		if (e.key === 'ArrowRight') next = (i + 1) % ids.length;
		else if (e.key === 'ArrowLeft') next = (i - 1 + ids.length) % ids.length;
		else if (e.key === 'Home') next = 0;
		else if (e.key === 'End') next = ids.length - 1;
		if (next < 0) return;
		e.preventDefault();
		open(ids[next]);
		tablist?.querySelector<HTMLButtonElement>(`[data-tab="${ids[next]}"]`)?.focus();
	}

	const isMac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
	const abortKeys = isMac ? 'Cmd .' : 'Ctrl .';
	const STEPS = [
		{ id: 'setup' as const, n: 1, label: 'Setup' },
		{ id: 'run' as const, n: 2, label: 'Run' }
	];
</script>

<div class="acquire" data-testid="explorer-screen">
	<div class="top">
		<div class="crumb">
			{#if tests.canLeave}
				<button class="btn ghost sm back" data-testid="test-type-back" onclick={() => tests.toChooser()}>
					<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M7.5 2.5L4 6l3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
					Test type
				</button>
				<span class="sep" aria-hidden="true">/</span>
			{/if}
			<h1 class="here" tabindex="-1" bind:this={heading}>Explorer</h1>
			<ol class="flow" aria-label="Steps">
				{#each STEPS as st, i (st.id)}
					{#if i > 0}
						<li aria-hidden="true" class="arrow"><svg viewBox="0 0 12 12" width="10" height="10"><path d="M4.5 2.5L8 6l-3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" /></svg></li>
					{/if}
					<li>
						<button class="step" class:on={step === st.id} aria-current={step === st.id ? 'step' : undefined} data-testid="flow-{st.id}" onclick={() => (st.id === 'run' && step === 'setup' ? proceed() : (explorer.step = st.id))}>
							<span class="n">{st.n}</span>{st.label}
							{#if st.id === 'run' && liveDot}<span class="mark dot" aria-label="running"><svg viewBox="0 0 10 10" width="8" height="8" aria-hidden="true"><circle cx="5" cy="5" r="4" fill="currentColor" /></svg></span>{/if}
						</button>
					</li>
				{/each}
			</ol>
		</div>
		<RunBar acq={explorer} onoverride={() => {}} {abortKeys}>
			{#snippet verbs()}<ExplorerRunVerbs {explorer} />{/snippet}
			{#snippet nums()}
				<div>
					<dt>Frequency</dt>
					<dd class="num" data-testid="run-frequency">{hz(now?.frequency_hz)}</dd>
				</div>
				<div>
					<dt>Drive</dt>
					<dd class="num" data-testid="run-drive">{vpk(now?.drive_vpk)}</dd>
				</div>
				{#if sweep}
					<div>
						<dt>Sweep</dt>
						<dd class="num" data-testid="run-sweep">{sweep}</dd>
					</div>
				{/if}
			{/snippet}
		</RunBar>
		<Notices acq={explorer} />
	</div>

	{#if step === 'setup'}
		<div class="tabs" role="tablist" aria-label="Explorer setup sections" bind:this={tablist} tabindex="-1" onkeydown={onTabKey}>
			{#each tabs as t (t.id)}
				<button role="tab" id="explorer-tab-{t.id}" data-tab={t.id} data-testid="tab-{t.id}" aria-selected={section === t.id} aria-controls="explorer-panel" tabindex={section === t.id ? 0 : -1} class:on={section === t.id} onclick={() => open(t.id)}>
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
			{#if explorer.locked}
				<span class="locked" data-testid="setup-locked">
					<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><rect x="3" y="6.5" width="8" height="6" rx="1" fill="currentColor" /><path d="M4.8 6.5V4.8a2.2 2.2 0 014.4 0v1.7" fill="none" stroke="currentColor" stroke-width="1.4" /></svg>
					Locked while the run is live
				</span>
			{/if}
		</div>

		<div class="body" bind:this={body} id="explorer-panel" role="tabpanel" aria-labelledby="explorer-tab-{section}" tabindex="-1">
			<div class="withsum">
				<div class="main">
					{#if section === 'station'}
						<div class="stack">
							<StationPanel acq={explorer.acq} />
							<p class="aq-small aq-muted" data-testid="explorer-run-channels">The run drives the table's one enabled drive row ({channels.drive ? `channel ${channels.drive.index}` : 'none yet'}) and reads the survey from its control row: the first enabled control row, else the first enabled input ({channels.control ? `channel ${channels.control.index}` : 'none yet'}). Every enabled input is acquired, and each row's abort limits are the run's.</p>
							<ChannelTable acq={explorer.acq} />
						</div>
					{:else if section === 'test'}
						<ExplorerSetup {explorer} />
					{:else}
						<ExplorerCapture {explorer} />
					{/if}
				</div>
				<ExplorerSummary {explorer} onopen={open} />
			</div>
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
			<button class="btn primary" data-testid="proceed-to-run" aria-disabled={!!proceedWhy || proceeding} class:off={!!proceedWhy} title={proceedWhy ?? 'Apply the setup and show the run: start the tone from there.'} onclick={proceed}>
				Proceed to Run
				<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M4.5 2.5L8 6l-3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
			</button>
		</footer>
	{:else}
		<div class="body fit" bind:this={body} data-testid="run-view">
			<ExplorerRunView {explorer} {views} onpopout={(id) => app.openDisplayWindow(id, 'explorer')}>
				{#snippet station()}<StationPanel acq={explorer.acq} />{/snippet}
				{#snippet end()}
					<button class="btn sm" data-testid="back-to-setup" onclick={() => (explorer.step = 'setup')} title={explorer.isLive ? 'Look at the setup. It stays locked while the run is live.' : 'Back to the setup.'}>
						<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M7.5 2.5L4 6l3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
						Back to Setup
					</button>
				{/snippet}
			</ExplorerRunView>
		</div>
	{/if}
</div>

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

	@media (min-height: 700px) and (min-width: 1101px) {
		.body.fit {
			display: flex;
			flex-direction: column;
			overflow: hidden;
			padding-bottom: var(--sp-3);
		}
	}

	.withsum {
		display: grid;
		grid-template-columns: minmax(0, 1fr) 300px;
		gap: var(--sp-3);
		align-items: start;
	}

	.main {
		min-width: 0;
	}

	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
	}

	.stack p {
		margin: 0;
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

	@keyframes pulse {
		50% {
			opacity: 0.35;
		}
	}

	@media (max-width: 1100px) {
		.withsum {
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
