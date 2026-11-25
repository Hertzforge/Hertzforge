<script lang="ts">
	/**
	 * The operator's screen for Playback, on the test screen's frame: Setup in sections with its
	 * summary beside every one (the station and channels, the drive file, its check, the level and
	 * limits, and the records), then Run with its pages of plots and the control dock. The header
	 * carries the file's rate and the play's progress beside the run's state.
	 */
	import type { AppState } from '$lib/app/state.svelte';
	import ChannelTable from '../ChannelTable.svelte';
	import PulseScreen from '../pulse/PulseScreen.svelte';
	import SetupFileBar from '../setupFiles/SetupFileBar.svelte';
	import PlaybackCapture from './PlaybackCapture.svelte';
	import PlaybackCheck from './PlaybackCheck.svelte';
	import PlaybackDrive from './PlaybackDrive.svelte';
	import PlaybackLevel from './PlaybackLevel.svelte';
	import PlaybackRunView from './PlaybackRunView.svelte';
	import PlaybackSummary from './PlaybackSummary.svelte';
	import { PLAYBACK_SECTIONS, playbackRows, type PlaybackSection } from './playbackSetup';
	import { secs } from './playbackWords';

	interface Props {
		app: AppState;
	}
	let { app }: Props = $props();

	const playback = $derived(app.playback);
	const views = $derived(app.playbackViews);
	const tests = $derived(app.tests);
	const now = $derived(playback.now);
	const rows = $derived(playbackRows(playback.rows));

	const isMac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
	const abortKeys = isMac ? 'Cmd .' : 'Ctrl .';
</script>

{#snippet nums()}
	<div>
		<dt>Rate</dt>
		<dd class="num" data-testid="run-rate">{now ? `${now.fs_hz} Hz` : '-'}</dd>
	</div>
	<div>
		<dt>Played</dt>
		<dd class="num" data-testid="run-played">{now ? `${secs(now.progress.played_s)} of ${secs(now.drive.duration_s)}` : '-'}</dd>
	</div>
{/snippet}

{#snippet setup(section: string)}
	{#if section === 'station'}
		<div class="stack">
			<p class="aq-small aq-muted" data-testid="playback-run-channels">The run records every enabled input row of the table, in table order: {rows.inputs.length ? `row${rows.inputs.length === 1 ? '' : 's'} ${rows.inputs.map((r) => r.index).join(', ')}` : 'none yet'}. Each file channel plays on an enabled drive row ({rows.drives.length ? rows.drives.map((r) => r.index).join(', ') : 'none yet'}), and every drive is recorded as played. A drive row no channel plays on is driven at 0 V. Each input row's abort limits are the run's response aborts.</p>
			<ChannelTable acq={playback.acq} />
		</div>
	{:else if section === 'drive'}
		<PlaybackDrive {playback} />
	{:else if section === 'check'}
		<PlaybackCheck {playback} />
	{:else if section === 'level'}
		<PlaybackLevel {playback} />
	{:else}
		<PlaybackCapture {playback} />
	{/if}
{/snippet}

{#snippet summary(open: (id: string) => void)}
	<SetupFileBar {app} module="playback" />
	<PlaybackSummary {playback} onopen={(s: PlaybackSection) => open(s)} />
{/snippet}

{#snippet run()}
	<PlaybackRunView {playback} {views} {abortKeys} opener={app.playbackRunOpen} />
{/snippet}

<PulseScreen
	host={playback}
	screen={views}
	name="Playback"
	testid="playback-screen"
	ids="playback"
	sections={PLAYBACK_SECTIONS}
	{setup}
	{summary}
	{run}
	{nums}
	onleave={tests.canLeave ? () => tests.toChooser() : null}
	onconfigure={() => tests.toConfigure()}
	takeFocus={() => app.tests.takeFocus()}
	proceedTitle="Apply the setup, which the control service checks with the drive file, and show the run: start from there."
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
