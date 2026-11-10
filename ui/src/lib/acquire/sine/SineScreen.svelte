<script lang="ts">
	/**
	 * The operator's screen for Sine, on the test screen's frame: Setup in sections with its summary
	 * beside every one, then Run with its pages of plots and the control dock. The header carries
	 * the tone's frequency and the sweep beside the run's state and level. The review hold after
	 * identification brings the page with the identified gain forward, once per hold.
	 */
	import { untrack } from 'svelte';
	import type { AppState } from '$lib/app/state.svelte';
	import ChannelTable from '../ChannelTable.svelte';
	import OverrideDialog from '../OverrideDialog.svelte';
	import StationPanel from '../StationPanel.svelte';
	import PulseScreen from '../pulse/PulseScreen.svelte';
	import SineCapture from './SineCapture.svelte';
	import SineControl from './SineControl.svelte';
	import SineLimits from './SineLimits.svelte';
	import SineModeEditor from './SineModeEditor.svelte';
	import SineProfileEditor from './SineProfileEditor.svelte';
	import SineRunView from './SineRunView.svelte';
	import SineScheduleEditor from './SineScheduleEditor.svelte';
	import SineSummary from './SineSummary.svelte';
	import { runChannels, SINE_SECTIONS, type SineSection } from './sineSetup';
	import { hz, sweepWords } from './sineWords';

	interface Props {
		app: AppState;
	}
	let { app }: Props = $props();

	const sine = $derived(app.sine);
	const views = $derived(app.sineViews);
	const tests = $derived(app.tests);
	let overrideOpen = $state(false);

	const now = $derived(sine.now);
	const channels = $derived(runChannels(sine.rows));
	const freq = $derived(sine.frameOfRun?.frequency_hz ?? now?.frequency_hz ?? null);
	const sweep = $derived(sweepWords(now));

	// The review hold brings the identified system forward, once per hold.
	let reviewed: string | null = null;
	$effect(() => {
		const s = sine.status;
		if (s?.state !== 'review' || !s.run_id || reviewed === s.run_id) return;
		reviewed = s.run_id;
		untrack(() => {
			sine.step = 'run';
			views.bringForward('identification');
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
	{#if sweep}
		<div>
			<dt>Sweep</dt>
			<dd class="num" data-testid="run-sweep">{sweep}</dd>
		</div>
	{/if}
{/snippet}

{#snippet setup(section: string)}
	{#if section === 'station'}
		<div class="stack">
			<StationPanel acq={sine.acq} />
			<p class="aq-small aq-muted" data-testid="sine-run-channels">The run controls on the table's enabled control rows ({channels.control ? `the first is channel ${channels.control.index}` : 'none yet'}) and drives on its first enabled drive row ({channels.drive ? `channel ${channels.drive.index}` : 'none yet'}).</p>
			<ChannelTable acq={sine.acq} />
		</div>
	{:else if section === 'profile'}
		<SineProfileEditor {sine} />
	{:else if section === 'test'}
		<SineModeEditor {sine} />
	{:else if section === 'schedule'}
		<SineScheduleEditor {sine} />
	{:else if section === 'control'}
		<SineControl {sine} />
	{:else if section === 'limits'}
		<SineLimits {sine} />
	{:else}
		<SineCapture {sine} />
	{/if}
{/snippet}

{#snippet summary(open: (id: string) => void)}
	<SineSummary {sine} onopen={(s: SineSection) => open(s)} />
{/snippet}

{#snippet station()}<StationPanel acq={sine.acq} />{/snippet}

{#snippet run()}
	<SineRunView {sine} {views} {station} {abortKeys} onoverride={() => (overrideOpen = true)} onpopout={(id) => app.openDisplayWindow(id, 'sine')} />
{/snippet}

<PulseScreen
	host={sine}
	screen={views}
	name="Sine"
	testid="sine-screen"
	ids="sine"
	sections={SINE_SECTIONS}
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
	acq={sine.acq}
	bind:open={overrideOpen}
	variant={{
		title: 'Arm Past the Shaker Ratings',
		lead: sine.status?.feasibility?.message ?? "The profile exceeds the shaker's ratings that were given.",
		verb: 'Arming',
		confirm: 'Arm With This Acknowledgement',
		send: (text) => sine.arm(text),
		refusal: sine.refusal && sine.refusal.verb === 'Arm Past Feasibility' ? `${sine.refusal.title}: ${sine.refusal.message}` : null,
		busy: sine.busy !== null
	}}
/>

<style>
	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
	}

	.stack p {
		margin: 0;
	}
</style>
