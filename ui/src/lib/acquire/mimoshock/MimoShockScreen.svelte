<script lang="ts">
	/**
	 * The operator's screen for MIMO Shock, on the test screen's frame: Setup in sections with its
	 * summary beside every one, then Run with its pages of plots and the control dock. The header
	 * carries the pulses fired and passed, and the time to the next one. The review hold after
	 * identification brings the identification page forward, once per hold.
	 */
	import { untrack } from 'svelte';
	import type { AppState } from '$lib/app/state.svelte';
	import StationPanel from '../StationPanel.svelte';
	import PulseScreen from '../pulse/PulseScreen.svelte';
	import { clock } from '../words';
	import MsChannels from './MsChannels.svelte';
	import MsLadder from './MsLadder.svelte';
	import MsLaw from './MsLaw.svelte';
	import MsLimits from './MsLimits.svelte';
	import MsRunView from './MsRunView.svelte';
	import MsSummary from './MsSummary.svelte';
	import MsTargets from './MsTargets.svelte';
	import MsTiming from './MsTiming.svelte';
	import { MS_SECTIONS, type MsSection } from './msSetup';

	interface Props {
		app: AppState;
	}
	let { app }: Props = $props();

	const ms = $derived(app.mimoShock);
	const views = $derived(app.mimoShockViews);
	const tests = $derived(app.tests);
	const now = $derived(ms.now);

	let reviewed: string | null = null;
	$effect(() => {
		const s = ms.status;
		if (s?.state !== 'review' || !s.run_id || reviewed === s.run_id) return;
		reviewed = s.run_id;
		untrack(() => {
			ms.step = 'run';
			views.bringForward('identification');
		});
	});

	const isMac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
	const abortKeys = isMac ? 'Cmd .' : 'Ctrl .';
</script>

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

{#snippet setup(section: string)}
	{#if section === 'channels'}
		<MsChannels {ms} />
	{:else if section === 'target'}
		<MsTargets {ms} />
	{:else if section === 'timing'}
		<MsTiming {ms} />
	{:else if section === 'ladder'}
		<MsLadder {ms} />
	{:else if section === 'law'}
		<MsLaw {ms} />
	{:else}
		<MsLimits {ms} />
	{/if}
{/snippet}

{#snippet summary(open: (id: string) => void)}
	<MsSummary {ms} onopen={(s: MsSection) => open(s)} />
{/snippet}

{#snippet station()}<StationPanel acq={ms.acq} />{/snippet}

{#snippet run()}
	<MsRunView {ms} {views} {station} {abortKeys} onpopout={(id) => app.openDisplayWindow(id, 'mimo_shock')} />
{/snippet}

<PulseScreen
	host={ms}
	screen={views}
	name="MIMO Shock"
	testid="mimo-shock-screen"
	ids="ms"
	sections={MS_SECTIONS}
	{setup}
	{summary}
	{run}
	{nums}
	onleave={tests.canLeave ? () => tests.toChooser() : null}
	takeFocus={() => app.tests.takeFocus()}
	proceedTitle="Apply the setup and show the run: start identification from there."
	{abortKeys}
/>
