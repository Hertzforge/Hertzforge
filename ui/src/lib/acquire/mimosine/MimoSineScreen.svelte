<script lang="ts">
	/**
	 * The operator's screen for MIMO Sine, on the test screen's frame: Setup in sections with its
	 * summary beside every one, then Run with its pages of plots and the control dock. The header
	 * carries the tone's frequency and the sweep beside the run's state and level. The review hold
	 * after identification brings the identification page forward, once per hold, and proceeding
	 * from it brings the controls page back.
	 */
	import { untrack } from 'svelte';
	import type { AppState } from '$lib/app/state.svelte';
	import OverrideDialog from '../OverrideDialog.svelte';
	import StationPanel from '../StationPanel.svelte';
	import PulseScreen from '../pulse/PulseScreen.svelte';
	import SetupFileBar from '../setupFiles/SetupFileBar.svelte';
	import MsineChannels from './MsineChannels.svelte';
	import MsineLaw from './MsineLaw.svelte';
	import MsineLimits from './MsineLimits.svelte';
	import MsineReferences from './MsineReferences.svelte';
	import MsineRunView from './MsineRunView.svelte';
	import MsineSchedule from './MsineSchedule.svelte';
	import MsineSummary from './MsineSummary.svelte';
	import MsineTest from './MsineTest.svelte';
	import { MSINE_SECTIONS, type MsineSection } from './msineSetup';
	import { reviewPage, type ReviewPages } from './msineViews.svelte';
	import { hz } from './msineWords';

	interface Props {
		app: AppState;
	}
	let { app }: Props = $props();

	const ms = $derived(app.mimoSine);
	const views = $derived(app.mimoSineViews);
	const tests = $derived(app.tests);
	let overrideOpen = $state(false);
	const now = $derived(ms.now);
	const freq = $derived(ms.frameOfRun?.frequency_hz ?? now?.frequency_hz ?? null);

	const seen: ReviewPages = { review: null, proceeded: null };
	$effect(() => {
		const page = reviewPage(ms.status, seen);
		if (!page) return;
		untrack(() => {
			ms.step = 'run';
			views.bringForward(page);
		});
	});

	const isMac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
	const abortKeys = isMac ? 'Cmd .' : 'Ctrl .';
</script>

{#snippet nums()}
	<div>
		<dt>Frequency</dt>
		<dd class="num" data-testid="run-frequency">{hz(freq)}</dd>
	</div>
	{#if now?.sweeps}
		<div>
			<dt>Sweep</dt>
			<dd class="num" data-testid="run-sweep">{now.direction ?? ''} {now.sweeps.done}{now.sweeps.of !== null ? ` of ${now.sweeps.of}` : ''}</dd>
		</div>
	{:else if now?.steps}
		<div>
			<dt>Step</dt>
			<dd class="num" data-testid="run-step">{now.steps.done} of {now.steps.of}</dd>
		</div>
	{/if}
{/snippet}

{#snippet setup(section: string)}
	{#if section === 'channels'}
		<MsineChannels {ms} />
	{:else if section === 'references'}
		<MsineReferences {ms} />
	{:else if section === 'test'}
		<MsineTest {ms} />
	{:else if section === 'schedule'}
		<MsineSchedule {ms} />
	{:else if section === 'law'}
		<MsineLaw {ms} />
	{:else}
		<MsineLimits {ms} />
	{/if}
{/snippet}

{#snippet summary(open: (id: string) => void)}
	<SetupFileBar {app} module="mimo_sine" />
	<MsineSummary {ms} onopen={(s: MsineSection) => open(s)} />
{/snippet}

{#snippet station()}<StationPanel acq={ms.acq} />{/snippet}

{#snippet run()}
	<MsineRunView {ms} {views} {station} {abortKeys} onoverride={() => (overrideOpen = true)} onpopout={(id) => app.openDisplayWindow(id, 'mimo_sine')} opener={app.mimoSineRunOpen} />
{/snippet}

<PulseScreen
	host={ms}
	screen={views}
	name="MIMO Sine"
	testid="mimo-sine-screen"
	ids="msine"
	sections={MSINE_SECTIONS}
	{setup}
	{summary}
	{run}
	{nums}
	onleave={tests.canLeave ? () => tests.toChooser() : null}
	takeFocus={() => app.tests.takeFocus()}
	proceedTitle="Apply the setup and show the run: start identification from there."
	{abortKeys}
/>

<OverrideDialog
	acq={ms.acq}
	bind:open={overrideOpen}
	variant={{
		title: 'Arm Past the Shaker Ratings',
		lead: ms.status?.feasibility?.message ?? "The references exceed a shaker's ratings that were given.",
		verb: 'Arming',
		confirm: 'Arm With This Acknowledgement',
		send: (text) => ms.arm(text),
		refusal: ms.refusal && ms.refusal.verb === 'Arm Past Feasibility' ? `${ms.refusal.title}: ${ms.refusal.message}` : null,
		busy: ms.busy !== null
	}}
/>
