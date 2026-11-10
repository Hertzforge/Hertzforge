<script lang="ts">
	/**
	 * The operator's screen for MIMO Random, in three steps: Setup (the setup in sections, Channels,
	 * Reference, Control, Schedule and Backend, with the setup summary beside every one, so whatever
	 * is missing is one click away, and Apply Setup and Proceed to Run in the bar at the bottom),
	 * Predict (after identification the service holds for the prediction, which comes forward by
	 * itself), then Run (the pages of plots and the control dock; engaging brings it forward). One
	 * header line heads all three: the way back to the test types while no run is live, the steps,
	 * the run's state, clock and level, where the data comes from, and who holds control.
	 */
	import { onMount } from 'svelte';
	import '$lib/acquire/acquire.css';
	import type { AppState } from '$lib/app/state.svelte';
	import Findings from '../Findings.svelte';
	import Notices from '../Notices.svelte';
	import ScheduleEditor from '../ScheduleEditor.svelte';
	import StationPanel from '../StationPanel.svelte';
	import ScreenHeader from '../screen/ScreenHeader.svelte';
	import MimoBackend from './MimoBackend.svelte';
	import MimoChannels from './MimoChannels.svelte';
	import MimoControl from './MimoControl.svelte';
	import MimoPredict from './MimoPredict.svelte';
	import MimoReference from './MimoReference.svelte';
	import { runBandsOf, runGeometryOf } from './runFrame';
	import MimoRunView from './views/MimoRunView.svelte';
	import MimoSpace from './MimoSpace.svelte';
	import MimoSummary from './MimoSummary.svelte';
	import type { MimoSection } from './mimoSetup';

	interface Props {
		app: AppState;
	}
	let { app }: Props = $props();

	const mimo = $derived(app.mimo);
	const screen = $derived(app.mimoViews);
	const tests = $derived(app.tests);
	let heading = $state<HTMLElement | null>(null);

	// Arriving from the chooser or the run pill, the keyboard lands on the screen's name.
	onMount(() => {
		if (app.tests.takeFocus()) heading?.focus();
	});

	type SetupSection = Exclude<MimoSection, 'predict'>;
	const SECTIONS: { id: SetupSection; label: string }[] = [
		{ id: 'channels', label: 'Channels' },
		{ id: 'reference', label: 'Reference' },
		{ id: 'control', label: 'Control' },
		{ id: 'schedule', label: 'Schedule' },
		{ id: 'backend', label: 'Backend' }
	];

	const tabs = $derived(SECTIONS.map((s) => ({ ...s, errors: mimo.counts[s.id].errors, warnings: mimo.counts[s.id].warnings })));
	const section = $derived((mimo.section === 'predict' ? 'channels' : mimo.section) as SetupSection);
	const step = $derived(mimo.step);
	const review = $derived(mimo.status?.state === 'review');
	/** What the run's plots draw against: each controlled quantity's bands, and where the sensors are. */
	const runBands = $derived(runBandsOf(mimo.status));
	const geometry = $derived(runGeometryOf(mimo.setup, mimo.status));

	/** Why Proceed to Run waits, or null. An observer only looks, so it never waits. */
	const proceedWhy = $derived.by(() => {
		if (!mimo.isController || mimo.isLive) return null;
		const a = mimo.actions.apply;
		if (mimo.setupStatus === 'applied' || mimo.setupStatus === 'unread') return null;
		return a.enabled ? null : a.reason;
	});
	const flowNote = $derived.by(() => {
		if (mimo.isLive) return { tone: 'info', text: 'The run is live; the setup is locked until it ends.' };
		if (!mimo.isController) return { tone: 'info', text: 'Observing: the run the service holds is shown.' };
		if (proceedWhy) return { tone: 'warn', text: proceedWhy };
		if (mimo.setupStatus === 'applied') return { tone: 'pass', text: 'The service holds this setup.' };
		const w = Object.values(mimo.counts).reduce((n, c) => n + c.warnings, 0);
		return { tone: 'info', text: `The setup checks clean${w ? `, with ${w} ${w === 1 ? 'warning' : 'warnings'}` : ''}. Proceed to Run applies it.` };
	});
	let proceeding = $state(false);

	/** Setup to Run: apply the setup if this screen holds control and it changed, then show the run. */
	async function proceed() {
		if (proceedWhy || proceeding) return;
		if (mimo.isController && !mimo.isLive && mimo.setupStatus !== 'applied' && mimo.setupStatus !== 'unread' && mimo.actions.apply.enabled) {
			proceeding = true;
			const ok = await mimo.apply();
			proceeding = false;
			if (!ok) return;
		}
		mimo.step = 'run';
	}

	function open(s: SetupSection | MimoSection) {
		mimo.step = 'setup';
		mimo.section = s;
	}

	const apply = $derived(mimo.actions.apply);
	let applying = $state(false);
	async function applySetup() {
		if (!apply.enabled || applying) return;
		applying = true;
		await mimo.apply();
		applying = false;
	}

	let tablist = $state<HTMLElement | null>(null);
	let body = $state<HTMLElement | null>(null);

	// Each section opens at its top.
	$effect(() => {
		void mimo.section;
		void mimo.step;
		body?.scrollTo({ top: 0 });
	});

	function onTabKey(e: KeyboardEvent) {
		const ids = SECTIONS.map((t) => t.id);
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
	/** Stop and Abort stay in reach: in the header whenever a live run's Run card is out of view. */
	const endsInHeader = $derived(mimo.isLive && !(step === 'run' && screen.cardInView('run')));
	/** What the state chip says on hover: how the run ended, why it holds, or what the next step waits on. */
	const reason = $derived.by(() => {
		const s = mimo.status;
		if (s?.state === 'finished' && s.end) return s.end.message;
		if (s?.state === 'review') return 'Nothing drives at test level until you proceed.';
		const p = mimo.primary;
		return p && mimo.actions[p].visible && !mimo.actions[p].enabled ? mimo.actions[p].reason : null;
	});
</script>

<div class="acquire" data-testid="mimo-screen">
	<ScreenHeader
		acq={mimo}
		name="MIMO Random"
		{step}
		onsetup={() => (mimo.step = 'setup')}
		onrun={() => (step === 'setup' ? proceed() : (mimo.step = 'run'))}
		predict={{ onopen: () => (mimo.step = 'predict'), waiting: review }}
		runWhy={step === 'setup' ? proceedWhy : null}
		onleave={tests.canLeave && !review ? () => tests.toChooser() : null}
		{reason}
		ends={endsInHeader}
		{abortKeys}
		bind:heading
	/>
	<!-- Notices float over the top of the page, so one never pushes the plots down. -->
	<div class="notices"><div class="float"><Notices acq={mimo} /></div></div>

	{#if step === 'setup'}
		<div class="tabs" role="tablist" aria-label="MIMO Random setup sections" bind:this={tablist} tabindex="-1" onkeydown={onTabKey}>
			{#each tabs as t (t.id)}
				<button role="tab" id="mm-tab-{t.id}" data-tab={t.id} data-testid="tab-{t.id}" aria-selected={section === t.id} aria-controls="mm-panel" tabindex={section === t.id ? 0 : -1} class:on={section === t.id} onclick={() => open(t.id)}>
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
			{#if mimo.locked}
				<span class="locked" data-testid="setup-locked">
					<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><rect x="3" y="6.5" width="8" height="6" rx="1" fill="currentColor" /><path d="M4.8 6.5V4.8a2.2 2.2 0 014.4 0v1.7" fill="none" stroke="currentColor" stroke-width="1.4" /></svg>
					Locked while the run is live
				</span>
			{/if}
		</div>

		<div class="body" bind:this={body} id="mm-panel" role="tabpanel" aria-labelledby="mm-tab-{section}" tabindex="-1">
			<div class="withsum">
				<div class="main">
					{#if section === 'channels'}
						<div class="column">
							<section class="aq-panel titlep">
								<label for="mm-title" class="lbl">Test title</label>
								<input id="mm-title" class="field" bind:value={mimo.setup.title} disabled={mimo.locked} aria-invalid={(mimo.fieldErrors.title ?? []).length > 0} data-testid="mimo-title" />
								<Findings items={(mimo.fieldErrors.title ?? []).map((message) => ({ level: 'error' as const, message }))} />
							</section>
							<MimoChannels {mimo} />
							<MimoSpace {mimo} />
						</div>
					{:else if section === 'reference'}
						<MimoReference {mimo} onanalyze={() => app.go('signals')} />
					{:else if section === 'control'}
						<MimoControl {mimo} />
					{:else if section === 'schedule'}
						<div class="narrow">
							<ScheduleEditor acq={mimo} />
						</div>
					{:else}
						<MimoBackend {mimo} />
					{/if}
				</div>
				<MimoSummary {mimo} onopen={open} />
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
			{#if apply.visible}
				<button class="btn" data-testid="act-apply" aria-disabled={!apply.enabled || applying} class:off={!apply.enabled} title={apply.reason ?? 'Send this setup to the control service.'} onclick={applySetup}>
					{#if applying}<svg class="spin" viewBox="0 0 12 12" width="12" height="12" aria-hidden="true"><path d="M6 1.5a4.5 4.5 0 104.5 4.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" /></svg>{/if}
					Apply Setup
				</button>
			{/if}
			<button class="btn primary" data-testid="proceed-to-run" aria-disabled={!!proceedWhy || proceeding} class:off={!!proceedWhy} title={proceedWhy ?? 'Apply the setup and show the run: start identification from there.'} onclick={proceed}>
				Proceed to Run
				<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M4.5 2.5L8 6l-3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
			</button>
		</footer>
	{:else if step === 'predict'}
		<div class="body" bind:this={body} data-testid="predict-step">
			<MimoPredict {mimo} />
		</div>
	{:else}
		<div class="run" id="mm-panel-live" data-testid="run-view">
			<MimoRunView {mimo} {screen} onpopout={(id) => app.openDisplayWindow(id, 'mimo_random')} {geometry} bands={runBands} diagnostics={mimo.law?.diagnostics ?? []} {abortKeys}>
				{#snippet station()}<StationPanel acq={mimo.acq} />{/snippet}
			</MimoRunView>
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

	.withsum {
		display: grid;
		grid-template-columns: minmax(0, 1fr) 300px;
		gap: var(--sp-3);
		align-items: start;
	}

	.main {
		min-width: 0;
	}

	.column {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
	}

	.titlep {
		display: grid;
		grid-template-columns: auto minmax(0, 420px);
		align-items: center;
		gap: 4px var(--sp-3);
	}

	.titlep :global(.aq-findings) {
		grid-column: 2;
	}

	.lbl {
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}

	.narrow {
		max-width: 720px;
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

	.flowbar .spin {
		animation: spin 0.8s linear infinite;
	}

	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}

	/* The run fills what the header leaves: the plots on their page, the controls beside them. */
	.run {
		flex: 1;
		min-height: 0;
		padding: 6px 6px 4px var(--sp-3);
	}

	@media (max-width: 1100px) {
		.withsum {
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
