<script lang="ts">
	/**
	 * The screen of a test type on the pulse page layer, in two steps as Random's: Setup (the setup
	 * in sections with its summary beside every one, so whatever is missing is one click away, and
	 * Apply Setup and Proceed to Run in the bar at the bottom), then Run (the pages of plots and the
	 * control dock). One header line heads both: the way back to the test types while no run is
	 * live, the steps, the run's state, clock, level and the type's own numbers, where the data comes
	 * from, and who holds control.
	 */
	import { onMount, type Snippet } from 'svelte';
	import '$lib/acquire/acquire.css';
	import Notices from '../Notices.svelte';
	import ScreenHeader from '../screen/ScreenHeader.svelte';
	import type { ScreenState } from '../screen/screen.svelte';
	import type { PulseScreenHost } from './pulseScreen';

	interface Props {
		host: PulseScreenHost;
		/** The run's screen, for whether its Run card is in view. */
		screen: ScreenState;
		/** The test's name. */
		name: string;
		/** The screen's test id (`sine-screen`). */
		testid: string;
		/** The setup tabs' and panel's id prefix. */
		ids: string;
		sections: { id: string; label: string }[];
		/** The setup section shown. */
		setup: Snippet<[string]>;
		/** The setup's summary, beside every section; it opens a section by its id. */
		summary: Snippet<[(id: string) => void]>;
		/** The run: its pages and its dock. */
		run: Snippet;
		/** The type's own numbers in the header, in place of the time at level. */
		nums?: Snippet;
		/** Back to the test types; absent while the screen cannot be left. */
		onleave?: (() => void) | null;
		/** True when the keyboard should land on the test's name on arrival. */
		takeFocus?: () => boolean;
		/** The footer's words while the run is live. */
		liveNote?: string;
		/** Proceed to Run's title when it can run. */
		proceedTitle?: string;
		abortKeys: string;
	}
	let { host, screen, name, testid, ids, sections, setup, summary, run, nums, onleave = null, takeFocus, liveNote = 'The run is live; the setup is locked until it ends.', proceedTitle = 'Apply the setup and show the run.', abortKeys }: Props = $props();

	let heading = $state<HTMLElement | null>(null);
	onMount(() => {
		if (takeFocus?.()) heading?.focus();
	});

	const step = $derived(host.step);
	const section = $derived(host.section);
	const tabs = $derived(sections.map((s) => ({ ...s, errors: host.counts[s.id]?.errors ?? 0, warnings: host.counts[s.id]?.warnings ?? 0 })));

	/** Why Proceed to Run waits, or null. An observer only looks, so it never waits. */
	const proceedWhy = $derived.by(() => {
		if (!host.isController || host.isLive) return null;
		if (host.setupStatus === 'applied' || host.setupStatus === 'unread') return null;
		const a = host.actions.apply;
		return a.enabled ? null : a.reason;
	});
	const flowNote = $derived.by(() => {
		if (host.isLive) return { tone: 'info', text: liveNote };
		if (!host.isController) return { tone: 'info', text: 'Observing: the run the service holds is shown.' };
		if (proceedWhy) return { tone: 'warn', text: proceedWhy };
		if (host.setupStatus === 'applied') return { tone: 'pass', text: 'The service holds this setup.' };
		const w = Object.values(host.counts).reduce((n, c) => n + c.warnings, 0);
		return { tone: 'info', text: `The setup checks clean${w ? `, with ${w} ${w === 1 ? 'warning' : 'warnings'}` : ''}. Proceed to Run applies it.` };
	});
	let proceeding = $state(false);

	/** Setup to Run: apply the setup if this screen holds control and it changed, then show the run. */
	async function proceed() {
		if (proceedWhy || proceeding) return;
		if (host.isController && !host.isLive && host.setupStatus !== 'applied' && host.setupStatus !== 'unread' && host.actions.apply.enabled) {
			proceeding = true;
			const ok = await host.apply();
			proceeding = false;
			if (!ok) return;
		}
		host.step = 'run';
	}

	function open(id: string) {
		host.step = 'setup';
		host.section = id;
	}

	const apply = $derived(host.actions.apply);
	let applying = $state(false);
	async function applySetup() {
		if (!apply.enabled || applying) return;
		applying = true;
		await host.apply();
		applying = false;
	}

	/** Stop and Abort stay in reach: in the header whenever a live run's Run card is out of view. */
	const endsInHeader = $derived(host.isLive && !(step === 'run' && screen.cardInView('run')));
	/** What the state chip says on hover: how the run ended, or what the next step waits on. */
	const reason = $derived.by(() => {
		const r = host.run;
		if (r?.state === 'finished' && r.end) return r.end.message;
		const p = host.primary;
		return p && host.actions[p].visible && !host.actions[p].enabled ? host.actions[p].reason : null;
	});

	let tablist = $state<HTMLElement | null>(null);
	let body = $state<HTMLElement | null>(null);

	// Each section opens at its top.
	$effect(() => {
		void host.section;
		void host.step;
		body?.scrollTo({ top: 0 });
	});

	function onTabKey(e: KeyboardEvent) {
		const list = sections.map((t) => t.id);
		const i = list.indexOf(section);
		let next = -1;
		if (e.key === 'ArrowRight') next = (i + 1) % list.length;
		else if (e.key === 'ArrowLeft') next = (i - 1 + list.length) % list.length;
		else if (e.key === 'Home') next = 0;
		else if (e.key === 'End') next = list.length - 1;
		if (next < 0) return;
		e.preventDefault();
		open(list[next]);
		tablist?.querySelector<HTMLButtonElement>(`[data-tab="${list[next]}"]`)?.focus();
	}
</script>

<div class="acquire" data-testid={testid}>
	<ScreenHeader acq={host} {name} {step} onsetup={() => (host.step = 'setup')} onrun={proceed} runWhy={proceedWhy} {onleave} {reason} ends={endsInHeader} {abortKeys} {nums} bind:heading />
	<!-- Notices float over the top of the page, so one never pushes the plots down. -->
	<div class="notices"><div class="stack"><Notices acq={host} /></div></div>

	{#if step === 'setup'}
		<div class="tabs" role="tablist" aria-label="{name} setup sections" bind:this={tablist} tabindex="-1" onkeydown={onTabKey}>
			{#each tabs as t (t.id)}
				<button role="tab" id="{ids}-tab-{t.id}" data-tab={t.id} data-testid="tab-{t.id}" aria-selected={section === t.id} aria-controls="{ids}-panel" tabindex={section === t.id ? 0 : -1} class:on={section === t.id} onclick={() => open(t.id)}>
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
			{#if host.locked}
				<span class="locked" data-testid="setup-locked">
					<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><rect x="3" y="6.5" width="8" height="6" rx="1" fill="currentColor" /><path d="M4.8 6.5V4.8a2.2 2.2 0 014.4 0v1.7" fill="none" stroke="currentColor" stroke-width="1.4" /></svg>
					Locked while the run is live
				</span>
			{/if}
		</div>

		<div class="body" bind:this={body} id="{ids}-panel" role="tabpanel" aria-labelledby="{ids}-tab-{section}" tabindex="-1">
			<div class="withsum">
				<div class="main">{@render setup(section)}</div>
				{@render summary(open)}
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
			<button class="btn primary" data-testid="proceed-to-run" aria-disabled={!!proceedWhy || proceeding} class:off={!!proceedWhy} title={proceedWhy ?? proceedTitle} onclick={proceed}>
				Proceed to Run
				<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M4.5 2.5L8 6l-3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
			</button>
		</footer>
	{:else}
		<div class="run" id="{ids}-panel-live" data-testid="run-view">{@render run()}</div>
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

	.stack {
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

	.stack > :global(*) {
		pointer-events: auto;
		box-shadow: var(--shadow-pop);
	}

	/* The tinted notices are translucent: lay them on the page colour so a plot never shows through. */
	.stack > :global(.aq-notice.warn) {
		background: linear-gradient(var(--amber-weak), var(--amber-weak)), var(--bg-2);
	}

	.stack > :global(.aq-notice.pass) {
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
