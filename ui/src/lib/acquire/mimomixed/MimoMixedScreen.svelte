<script lang="ts">
	/**
	 * The operator's screen for MIMO Mixed Mode, on the test screen's frame: Setup in sections with
	 * its summary beside every one (the station and channels, the floor's reference, the parts, the
	 * schedule and identification, the five laws, limits and protections, the prediction and the
	 * refusals), then Run with its pages of plots and the control dock. The header carries the run's
	 * time, the parts playing and, while a drive is out of room, the word Limiting. The review hold
	 * after identification brings the run forward, once per hold, with the identification and the
	 * prediction in the Run card, and proceeding from it brings the controls page back.
	 */
	import { untrack } from 'svelte';
	import type { AppState } from '$lib/app/state.svelte';
	import StationPanel from '../StationPanel.svelte';
	import PulseScreen from '../pulse/PulseScreen.svelte';
	import { clock } from '../words';
	import MmixChannels from './MmixChannels.svelte';
	import MmixForecast from './MmixForecast.svelte';
	import MmixLaws from './MmixLaws.svelte';
	import MmixLimits from './MmixLimits.svelte';
	import MmixParts from './MmixParts.svelte';
	import MmixReference from './MmixReference.svelte';
	import MmixRunView from './MmixRunView.svelte';
	import MmixSchedule from './MmixSchedule.svelte';
	import MmixSummary from './MmixSummary.svelte';
	import { MMIX_SECTIONS, type MmixSection } from './mmixSetup';
	import { reviewPage, type ReviewPages } from './mmixViews.svelte';

	interface Props {
		app: AppState;
	}
	let { app }: Props = $props();

	const mm = $derived(app.mimoMixed);
	const views = $derived(app.mimoMixedViews);
	const tests = $derived(app.tests);
	const now = $derived(mm.now);
	const on = $derived(now?.parts.filter((p) => p.on && p.kind !== 'floor').length ?? 0);

	const seen: ReviewPages = { review: null, proceeded: null };
	$effect(() => {
		const page = reviewPage(mm.status, seen);
		if (!page) return;
		untrack(() => {
			mm.step = 'run';
			views.bringForward(page);
		});
	});

	const isMac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
	const abortKeys = isMac ? 'Cmd .' : 'Ctrl .';
</script>

{#snippet nums()}
	<div>
		<dt>Run time</dt>
		<dd class="num" data-testid="run-t">{now?.t_s === null || now?.t_s === undefined ? '-' : clock(now.t_s)}</dd>
	</div>
	<div>
		<dt>Parts on</dt>
		<dd class="num" data-testid="run-parts-on">{on}</dd>
	</div>
	{#if now?.state === 'running' && now.limiting}
		<div>
			<dt>Limiting</dt>
			<dd class="limiting" data-testid="run-limiting"><svg viewBox="0 0 12 12" width="12" height="12" aria-hidden="true"><rect x="1.5" y="5" width="9" height="2" fill="currentColor" /></svg> {now.scale !== null && now.scale < 0.9995 ? `Tones at ${Math.round(now.scale * 100)} %` : 'A part is held back'}</dd>
		</div>
	{/if}
{/snippet}

{#snippet setup(section: string)}
	{#if section === 'channels'}
		<MmixChannels {mm} />
	{:else if section === 'reference'}
		<MmixReference {mm} />
	{:else if section === 'parts'}
		<MmixParts {mm} />
	{:else if section === 'schedule'}
		<MmixSchedule {mm} />
	{:else if section === 'laws'}
		<MmixLaws {mm} />
	{:else if section === 'limits'}
		<MmixLimits {mm} />
	{:else}
		<MmixForecast {mm} />
	{/if}
{/snippet}

{#snippet summary(open: (id: string) => void)}
	<MmixSummary {mm} onopen={(s: MmixSection) => open(s)} />
{/snippet}

{#snippet station()}<StationPanel acq={mm.acq} />{/snippet}

{#snippet run()}
	<MmixRunView {mm} {views} {station} {abortKeys} onpopout={(id) => app.openDisplayWindow(id, 'mimo_mixed')} opener={app.mimoMixedRunOpen} />
{/snippet}

<PulseScreen
	host={mm}
	screen={views}
	name="MIMO Mixed Mode"
	testid="mimo-mixed-screen"
	ids="mmix"
	sections={MMIX_SECTIONS}
	{setup}
	{summary}
	{run}
	{nums}
	onleave={tests.canLeave ? () => tests.toChooser() : null}
	takeFocus={() => app.tests.takeFocus()}
	proceedTitle="Apply the setup and show the run: start identification from there."
	{abortKeys}
/>

<style>
	.limiting {
		color: var(--amber-1);
		white-space: nowrap;
	}
</style>
