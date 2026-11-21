<script lang="ts">
	/**
	 * The operator's screen for the Modal Test, on the test screen's frame: Setup in sections with its
	 * summary beside every one (the station and channels, the DOFs, the mode and acquisition, the
	 * roving plan, the impact or shaker settings, and the records), then Measure: the fixed screen a
	 * person with a hammer reads at arm's length, with the shared run bar on top. The header carries
	 * the point and the counts beside the test's state.
	 */
	import type { AppState } from '$lib/app/state.svelte';
	import ChannelTable from '../ChannelTable.svelte';
	import StationPanel from '../StationPanel.svelte';
	import PulseScreen from '../pulse/PulseScreen.svelte';
	import type { ScreenState } from '../screen/screen.svelte';
	import SetupFileBar from '../setupFiles/SetupFileBar.svelte';
	import ModalAcquisition from './ModalAcquisition.svelte';
	import ModalCapture from './ModalCapture.svelte';
	import ModalDofs from './ModalDofs.svelte';
	import ModalExcitation from './ModalExcitation.svelte';
	import ModalMeasure from './ModalMeasure.svelte';
	import ModalRoving from './ModalRoving.svelte';
	import ModalSummary from './ModalSummary.svelte';
	import { modalSections, type ModalSection } from './modalTestSetup';

	interface Props {
		app: AppState;
	}
	let { app }: Props = $props();

	const modal = $derived(app.modalTest);
	const tests = $derived(app.tests);
	const now = $derived(modal.now);
	const sections = $derived(modalSections(modal.setup.mode));

	// The Measure screen is one fixed layout, not pages of plots: its run bar is always in view, so
	// the header never needs Stop and Abort of its own.
	const fixed = { cardInView: () => true } as unknown as ScreenState;

	const isMac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
	const abortKeys = isMac ? 'Cmd .' : 'Ctrl .';
</script>

{#snippet nums()}
	<div>
		<dt>Point</dt>
		<dd class="num" data-testid="run-point">{now ? `${now.point + 1} of ${now.counts.totals.points_planned}` : '-'}</dd>
	</div>
	<div>
		<dt>{now?.mode === 'shaker' ? 'Frames' : 'Hits'}</dt>
		<dd class="num" data-testid="run-hits">{now ? `${now.counts.point.accepted} of ${now.counts.point.asked}` : '-'}</dd>
	</div>
{/snippet}

{#snippet setup(section: string)}
	{#if section === 'station'}
		<div class="stack">
			<StationPanel acq={modal.acq} />
			<p class="aq-small aq-muted">The test records every enabled input row of the table. Give each its DOF and its part (reference or response) in DOFs; in shaker mode each shaker needs a drive row.</p>
			<ChannelTable acq={modal.acq} />
		</div>
	{:else if section === 'dofs'}
		<ModalDofs {modal} />
	{:else if section === 'acquisition'}
		<ModalAcquisition {modal} />
	{:else if section === 'roving'}
		<ModalRoving {modal} />
	{:else if section === 'excitation'}
		<ModalExcitation {modal} />
	{:else}
		<ModalCapture {modal} />
	{/if}
{/snippet}

{#snippet summary(open: (id: string) => void)}
	<!-- One column beside the section: the frame's grid has a single cell for the summary. -->
	<div class="side">
		<SetupFileBar {app} module="modal_test" />
		<ModalSummary {modal} onopen={(s: ModalSection) => open(s)} />
	</div>
{/snippet}

{#snippet run()}
	<ModalMeasure {modal} {abortKeys} opener={app.modalTestRunOpen} />
{/snippet}

<PulseScreen
	host={modal}
	screen={fixed}
	name="Modal Test"
	testid="modal-test-screen"
	ids="modal-test"
	{sections}
	{setup}
	{summary}
	{run}
	{nums}
	onleave={tests.canLeave ? () => tests.toChooser() : null}
	takeFocus={() => app.tests.takeFocus()}
	proceedTitle="Apply the setup, which the control service checks with the channel table's DOFs, and show the Measure screen: start from there."
	liveNote="The test is live; the setup is locked until it ends."
	{abortKeys}
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

	.side {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
	}
</style>
