<script lang="ts">
	/**
	 * The operator's screen for classical shock, in the format the other test types set: a link back
	 * to the test-type chooser, two steps (Setup, then Run), the run bar always in view, and the setup
	 * in sections with its summary beside every one, so whatever is missing is one click away. Start
	 * brings the run forward; the review hold after identification shows on the Identification page,
	 * and Proceed is the run bar's.
	 */
	import { onMount, untrack } from 'svelte';
	import '$lib/acquire/acquire.css';
	import type { AppState } from '$lib/app/state.svelte';
	import ChannelTable from '../ChannelTable.svelte';
	import Notices from '../Notices.svelte';
	import OverrideDialog from '../OverrideDialog.svelte';
	import RunBar from '../RunBar.svelte';
	import StationPanel from '../StationPanel.svelte';
	import { clock } from '../words';
	import ShockControl from './ShockControl.svelte';
	import ShockIdentification from './ShockIdentification.svelte';
	import ShockLimits from './ShockLimits.svelte';
	import ShockPulseEditor from './ShockPulseEditor.svelte';
	import ShockRunVerbs from './ShockRunVerbs.svelte';
	import ShockRunView from './ShockRunView.svelte';
	import ShockSchedule from './ShockSchedule.svelte';
	import ShockStandard from './ShockStandard.svelte';
	import ShockSummary from './ShockSummary.svelte';
	import ShockTrains from './ShockTrains.svelte';
	import { runChannels, SHOCK_SECTIONS, type ShockSection } from './shockSetup';

	interface Props {
		app: AppState;
	}
	let { app }: Props = $props();

	const shock = $derived(app.shock);
	const views = $derived(app.shockViews);
	const tests = $derived(app.tests);
	let heading: HTMLElement;
	let overrideOpen = $state(false);

	onMount(() => {
		if (app.tests.takeFocus()) heading.focus();
	});

	const tabs = $derived(SHOCK_SECTIONS.map((s) => ({ ...s, errors: shock.counts[s.id].errors, warnings: shock.counts[s.id].warnings })));
	const section = $derived(shock.section);
	const step = $derived(shock.step);
	const now = $derived(shock.now);
	const liveDot = $derived(now?.state === 'running' || now?.state === 'review' || now?.state === 'stopping');
	const channels = $derived(runChannels(shock.rows));

	// The review hold brings the identified system forward, once per hold.
	let reviewed: string | null = null;
	$effect(() => {
		const s = shock.status;
		if (s?.state !== 'review' || !s.run_id || reviewed === s.run_id) return;
		reviewed = s.run_id;
		untrack(() => {
			shock.step = 'run';
			const id = views.pages.find((p) => p.kind === 'identification')?.id;
			if (id) views.selectPage(id);
		});
	});

	/** Why Proceed to Run waits, or null. An observer only looks, so it never waits. */
	const proceedWhy = $derived.by(() => {
		if (!shock.isController || shock.isLive) return null;
		if (shock.setupStatus === 'applied' || shock.setupStatus === 'unread') return null;
		const a = shock.actions.apply;
		return a.enabled ? null : a.reason;
	});
	const flowNote = $derived.by(() => {
		if (shock.isLive) return { tone: 'info', text: 'The run is live; the setup is locked until it ends.' };
		if (!shock.isController) return { tone: 'info', text: 'Observing: the run the service holds is shown.' };
		if (proceedWhy) return { tone: 'warn', text: proceedWhy };
		if (shock.setupStatus === 'applied') return { tone: 'pass', text: 'The service holds this setup.' };
		const w = Object.values(shock.counts).reduce((n, c) => n + c.warnings, 0);
		return { tone: 'info', text: `The setup checks clean${w ? `, with ${w} ${w === 1 ? 'warning' : 'warnings'}` : ''}. Proceed to Run applies it.` };
	});
	let proceeding = $state(false);

	async function proceed() {
		if (proceedWhy || proceeding) return;
		if (shock.isController && !shock.isLive && shock.setupStatus !== 'applied' && shock.setupStatus !== 'unread' && shock.actions.apply.enabled) {
			proceeding = true;
			const ok = await shock.apply();
			proceeding = false;
			if (!ok) return;
		}
		shock.step = 'run';
	}

	function open(s: ShockSection) {
		shock.step = 'setup';
		shock.section = s;
	}

	let tablist = $state<HTMLElement | null>(null);
	let body = $state<HTMLElement | null>(null);

	$effect(() => {
		void shock.section;
		void shock.step;
		body?.scrollTo({ top: 0 });
	});

	function onTabKey(e: KeyboardEvent) {
		const ids = SHOCK_SECTIONS.map((t) => t.id);
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

<div class="acquire" data-testid="shock-screen">
	<div class="top">
		<div class="crumb">
			{#if tests.canLeave}
				<button class="btn ghost sm back" data-testid="test-type-back" onclick={() => tests.toChooser()}>
					<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M7.5 2.5L4 6l3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
					Test type
				</button>
				<span class="sep" aria-hidden="true">/</span>
			{/if}
			<h1 class="here" tabindex="-1" bind:this={heading}>Classical Shock</h1>
			<ol class="flow" aria-label="Steps">
				{#each STEPS as st, i (st.id)}
					{#if i > 0}
						<li aria-hidden="true" class="arrow"><svg viewBox="0 0 12 12" width="10" height="10"><path d="M4.5 2.5L8 6l-3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" /></svg></li>
					{/if}
					<li>
						<button class="step" class:on={step === st.id} aria-current={step === st.id ? 'step' : undefined} data-testid="flow-{st.id}" onclick={() => (st.id === 'run' && step === 'setup' ? proceed() : (shock.step = st.id))}>
							<span class="n">{st.n}</span>{st.label}
							{#if st.id === 'run' && liveDot}<span class="mark dot" aria-label="running"><svg viewBox="0 0 10 10" width="8" height="8" aria-hidden="true"><circle cx="5" cy="5" r="4" fill="currentColor" /></svg></span>{/if}
						</button>
					</li>
				{/each}
			</ol>
		</div>
		<RunBar acq={shock} onoverride={() => (overrideOpen = true)} {abortKeys}>
			{#snippet verbs()}<ShockRunVerbs {shock} />{/snippet}
			{#snippet nums()}
				<div>
					<dt>Pulses</dt>
					<dd class="num" data-testid="run-pulses">{now ? `${now.pulses.fired} fired, ${now.pulses.passed} passed` : '-'}</dd>
				</div>
				{#if now?.next_pulse_s !== null && now?.next_pulse_s !== undefined && now.state === 'running'}
					<div>
						<dt>Next Pulse</dt>
						<dd class="num" data-testid="run-next">{clock(now.next_pulse_s)}</dd>
					</div>
				{/if}
			{/snippet}
		</RunBar>
		<Notices acq={shock} />
	</div>

	{#if step === 'setup'}
		<div class="tabs" role="tablist" aria-label="Classical shock setup sections" bind:this={tablist} tabindex="-1" onkeydown={onTabKey}>
			{#each tabs as t (t.id)}
				<button role="tab" id="sk-tab-{t.id}" data-tab={t.id} data-testid="tab-{t.id}" aria-selected={section === t.id} aria-controls="sk-panel" tabindex={section === t.id ? 0 : -1} class:on={section === t.id} onclick={() => open(t.id)}>
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
			{#if shock.locked}
				<span class="locked" data-testid="setup-locked">
					<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><rect x="3" y="6.5" width="8" height="6" rx="1" fill="currentColor" /><path d="M4.8 6.5V4.8a2.2 2.2 0 014.4 0v1.7" fill="none" stroke="currentColor" stroke-width="1.4" /></svg>
					Locked while the run is live
				</span>
			{/if}
		</div>

		<div class="body" bind:this={body} id="sk-panel" role="tabpanel" aria-labelledby="sk-tab-{section}" tabindex="-1">
			<div class="withsum">
				<div class="main">
					{#if section === 'station'}
						<div class="stack">
							<StationPanel acq={shock.acq} />
							<p class="aq-small aq-muted" data-testid="shock-run-channels">The run controls on the table's first enabled control row ({channels.control ? `channel ${channels.control.index}` : 'none yet'}) and drives on its first enabled drive row ({channels.drive ? `channel ${channels.drive.index}` : 'none yet'}).</p>
							<ChannelTable acq={shock.acq} />
						</div>
					{:else if section === 'pulse'}
						<ShockPulseEditor {shock} />
					{:else if section === 'standard'}
						<ShockStandard {shock} />
					{:else if section === 'schedule'}
						<ShockSchedule {shock} />
					{:else if section === 'identification'}
						<ShockIdentification {shock} />
					{:else if section === 'control'}
						<ShockControl {shock} />
					{:else if section === 'limits'}
						<ShockLimits {shock} />
					{:else}
						<ShockTrains {shock} />
					{/if}
				</div>
				<ShockSummary {shock} onopen={open} />
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
			<button class="btn primary" data-testid="proceed-to-run" aria-disabled={!!proceedWhy || proceeding} class:off={!!proceedWhy} title={proceedWhy ?? 'Apply the setup and show the run: start identification from there.'} onclick={proceed}>
				Proceed to Run
				<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M4.5 2.5L8 6l-3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
			</button>
		</footer>
	{:else}
		<div class="body fit" bind:this={body} data-testid="run-view">
			<ShockRunView {shock} {views} onpopout={(id) => app.openDisplayWindow(id, 'shock')}>
				{#snippet station()}<StationPanel acq={shock.acq} />{/snippet}
				{#snippet end()}
					<button class="btn sm" data-testid="back-to-setup" onclick={() => (shock.step = 'setup')} title={shock.isLive ? 'Look at the setup. It stays locked while the run is live.' : 'Back to the setup.'}>
						<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M7.5 2.5L4 6l3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
						Back to Setup
					</button>
				{/snippet}
			</ShockRunView>
		</div>
	{/if}
</div>

<OverrideDialog
	acq={shock.acq}
	bind:open={overrideOpen}
	variant={{
		title: 'Arm Past the Shaker Ratings',
		lead: shock.status?.feasibility?.message ?? 'The pulse exceeds the shaker ratings that were given.',
		verb: 'Arming',
		confirm: 'Arm With This Acknowledgement',
		send: (text) => shock.arm(text),
		refusal: shock.refusal && shock.refusal.verb === 'Arm Past Feasibility' ? `${shock.refusal.title}: ${shock.refusal.message}` : null,
		busy: shock.busy !== null
	}}
/>

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
