<script lang="ts">
	/**
	 * The operator's screen for classical shock, on the test screen's frame: Setup in sections with
	 * its summary beside every one, then Run with its pages of plots and the control dock. The header
	 * carries the pulses fired and passed, and the time to the next one, beside the run's state and
	 * level. The review hold after identification brings the page with the identified system
	 * forward, once per hold.
	 */
	import { untrack } from 'svelte';
	import type { AppState } from '$lib/app/state.svelte';
	import ChannelTable from '../ChannelTable.svelte';
	import OverrideDialog from '../OverrideDialog.svelte';
	import StationPanel from '../StationPanel.svelte';
	import PulseScreen from '../pulse/PulseScreen.svelte';
	import { clock } from '../words';
	import ShockControl from './ShockControl.svelte';
	import ShockIdentification from './ShockIdentification.svelte';
	import ShockLimits from './ShockLimits.svelte';
	import ShockPulseEditor from './ShockPulseEditor.svelte';
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
	let overrideOpen = $state(false);

	const now = $derived(shock.now);
	const channels = $derived(runChannels(shock.rows));

	// The review hold brings the identified system forward, once per hold.
	let reviewed: string | null = null;
	$effect(() => {
		const s = shock.status;
		if (s?.state !== 'review' || !s.run_id || reviewed === s.run_id) return;
		reviewed = s.run_id;
		untrack(() => {
			shock.step = 'run';
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
{/snippet}

{#snippet summary(open: (id: string) => void)}
	<ShockSummary {shock} onopen={(s: ShockSection) => open(s)} />
{/snippet}

{#snippet station()}<StationPanel acq={shock.acq} />{/snippet}

{#snippet run()}
	<ShockRunView {shock} {views} {station} {abortKeys} onoverride={() => (overrideOpen = true)} onpopout={(id) => app.openDisplayWindow(id, 'shock')} opener={app.shockRunOpen} />
{/snippet}

<PulseScreen
	host={shock}
	screen={views}
	name="Classical Shock"
	testid="shock-screen"
	ids="sk"
	sections={SHOCK_SECTIONS}
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
