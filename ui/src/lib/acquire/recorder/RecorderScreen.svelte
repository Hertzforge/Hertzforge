<script lang="ts">
	/**
	 * The operator's screen for the Recorder, on the test screen's frame: Setup in sections with its
	 * summary beside every one, then Run with its pages of plots and the control dock. The header
	 * carries the station's rate, the recording being written and the recordings saved beside the
	 * run's state.
	 */
	import type { AppState } from '$lib/app/state.svelte';
	import ChannelTable from '../ChannelTable.svelte';
	import StationPanel from '../StationPanel.svelte';
	import PulseScreen from '../pulse/PulseScreen.svelte';
	import RecorderCapture from './RecorderCapture.svelte';
	import RecorderRunView from './RecorderRunView.svelte';
	import RecorderSetup from './RecorderSetup.svelte';
	import RecorderSimSource from './RecorderSimSource.svelte';
	import RecorderSummary from './RecorderSummary.svelte';
	import { RECORDER_SECTIONS, recorderRows, type RecorderSection } from './recorderSetup';
	import { rateWords } from './recorderWords';

	interface Props {
		app: AppState;
	}
	let { app }: Props = $props();

	const recorder = $derived(app.recorder);
	const views = $derived(app.recorderViews);
	const tests = $derived(app.tests);

	const now = $derived(recorder.now);
	const rows = $derived(recorderRows(recorder.rows));
	const onSim = $derived(recorder.acq.selected?.kind === 'sim');

	const isMac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
	const abortKeys = isMac ? 'Cmd .' : 'Ctrl .';
</script>

{#snippet nums()}
	<div>
		<dt>Rate</dt>
		<dd class="num" data-testid="run-rate">{rateWords(now?.grid?.fs_hz)}</dd>
	</div>
	<div>
		<dt>Recording</dt>
		<dd class="num" data-testid="run-recording">{now?.recording?.name ?? 'None'}</dd>
	</div>
	<div>
		<dt>Saved</dt>
		<dd class="num" data-testid="run-saved">{now?.recordings ?? 0}</dd>
	</div>
{/snippet}

{#snippet setup(section: string)}
	{#if section === 'station'}
		<div class="stack">
			<StationPanel acq={recorder.acq} />
			<p class="aq-small aq-muted" data-testid="recorder-run-channels">The run records every enabled input row of the table, in table order: {rows.recorded.length ? `channel${rows.recorded.length === 1 ? '' : 's'} ${rows.recorded.map((r) => r.index).join(', ')}` : 'none yet'}. Each recording holds one column per row. {rows.drives.length ? `Drive row${rows.drives.length === 1 ? '' : 's'} ${rows.drives.map((r) => r.index).join(', ')} ${rows.drives.length === 1 ? 'is' : 'are'} not opened and not recorded: the run opens no output.` : 'The run opens no output.'} A row's abort limits mark the recording and never stop it.</p>
			{#if onSim}<div class="aq-panel"><RecorderSimSource rows={rows.recorded.map((r) => r.index)} /></div>{/if}
			<ChannelTable acq={recorder.acq} />
		</div>
	{:else if section === 'trigger'}
		<RecorderSetup {recorder} part="trigger" />
	{:else if section === 'analysis'}
		<RecorderSetup {recorder} part="analysis" />
	{:else}
		<RecorderCapture {recorder} />
	{/if}
{/snippet}

{#snippet summary(open: (id: string) => void)}
	<RecorderSummary {recorder} onopen={(s: RecorderSection) => open(s)} />
{/snippet}

{#snippet station()}<StationPanel acq={recorder.acq} />{/snippet}

{#snippet run()}
	<RecorderRunView {recorder} {views} {station} {abortKeys} onpopout={(id) => app.openDisplayWindow(id, 'recorder')} />
{/snippet}

<PulseScreen
	host={recorder}
	screen={views}
	name="Recorder"
	testid="recorder-screen"
	ids="recorder"
	sections={RECORDER_SECTIONS}
	{setup}
	{summary}
	{run}
	{nums}
	onleave={tests.canLeave ? () => tests.toChooser() : null}
	takeFocus={() => app.tests.takeFocus()}
	proceedTitle="Apply the setup and show the run: start acquisition from there."
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
</style>
