<script lang="ts">
	/**
	 * Acquire & Control: the test-type chooser, the chosen test type's own screen, or Configure (the
	 * station every test type runs on, opened from the upper right of each). Which one is
	 * on show is kept in the shell state, so it survives moving between workspaces. Until the
	 * control service has said whether a run is live, neither is shown, so a screen opened during
	 * a live run goes straight to it.
	 */
	import type { AppState } from '$lib/app/state.svelte';
	import RandomScreen from '$lib/acquire/RandomScreen.svelte';
	import MimoRandomScreen from '$lib/acquire/mimo/MimoRandomScreen.svelte';
	import ShockScreen from '$lib/acquire/shock/ShockScreen.svelte';
	import SrsScreen from '$lib/acquire/srs/SrsScreen.svelte';
	import MimoShockScreen from '$lib/acquire/mimoshock/MimoShockScreen.svelte';
	import MimoSineScreen from '$lib/acquire/mimosine/MimoSineScreen.svelte';
	import { MSINE_STATE_WORDS } from '$lib/acquire/mimosine/msineWords';
	import MimoMixedScreen from '$lib/acquire/mimomixed/MimoMixedScreen.svelte';
	import { MMIX_STATE_WORDS } from '$lib/acquire/mimomixed/mmixWords';
	import { MS_STATE_WORDS } from '$lib/acquire/mimoshock/msWords';
	import SineScreen from '$lib/acquire/sine/SineScreen.svelte';
	import MixedScreen from '$lib/acquire/mixed/MixedScreen.svelte';
	import TwrScreen from '$lib/acquire/twr/TwrScreen.svelte';
	import { TWR_STATE_WORDS } from '$lib/acquire/twr/twrWords';
	import ExplorerScreen from '$lib/acquire/explorer/ExplorerScreen.svelte';
	import { EXPLORER_STATE_WORDS } from '$lib/acquire/explorer/explorerWords';
	import RecorderScreen from '$lib/acquire/recorder/RecorderScreen.svelte';
	import PlaybackScreen from '$lib/acquire/playback/PlaybackScreen.svelte';
	import ModalTestScreen from '$lib/acquire/modal_test/ModalTestScreen.svelte';
	import { MODAL_CHIP_WORDS, chipState } from '$lib/acquire/modal_test/modalTestWords';
	import { RECORDER_STATE_WORDS } from '$lib/acquire/recorder/recorderWords';
	import TestTypeChooser from '$lib/acquire/TestTypeChooser.svelte';
	import ConfigureScreen from '$lib/acquire/ConfigureScreen.svelte';
	import { STATE_WORDS } from '$lib/acquire/words';

	interface Props {
		app: AppState;
	}
	let { app }: Props = $props();

	const tests = $derived(app.tests);
	const liveWord = $derived(
		app.acquire.isLive && app.acquire.run
			? STATE_WORDS[app.acquire.run.state]
			: app.mimo.isLive && app.mimo.status
				? STATE_WORDS[app.mimo.status.state]
				: app.shock.isLive && app.shock.status
					? STATE_WORDS[app.shock.status.state]
					: app.srs.isLive && app.srs.status
						? STATE_WORDS[app.srs.status.state]
						: app.sine.isLive && app.sine.status
							? STATE_WORDS[app.sine.status.state]
							: app.mixed.isLive && app.mixed.status
							? STATE_WORDS[app.mixed.status.state]
							: app.twr.isLive && app.twr.status
								? TWR_STATE_WORDS[app.twr.status.state]
								: app.explorer.isLive && app.explorer.status
									? EXPLORER_STATE_WORDS[app.explorer.status.state]
									: app.recorder.isLive && app.recorder.status
										? RECORDER_STATE_WORDS[app.recorder.status.state]
										: app.mimoShock.isLive && app.mimoShock.status
											? MS_STATE_WORDS[app.mimoShock.status.state]
											: app.mimoSine.isLive && app.mimoSine.status
												? MSINE_STATE_WORDS[app.mimoSine.status.state]
												: app.modalTest.isLive && app.modalTest.now
													? MODAL_CHIP_WORDS[chipState(app.modalTest.now.state)]
													: app.mimoMixed.isLive && app.mimoMixed.status
														? MMIX_STATE_WORDS[app.mimoMixed.status.state]
														: null
	);
</script>

{#if tests.deciding}
	<p class="waiting" role="status" data-testid="acquire-waiting">Waiting for the control service</p>
{:else if tests.screen === 'configure'}
	<ConfigureScreen {app} />
{:else if tests.screen === 'random'}
	<RandomScreen {app} />
{:else if tests.screen === 'mimo_random'}
	<MimoRandomScreen {app} />
{:else if tests.screen === 'shock'}
	<ShockScreen {app} />
{:else if tests.screen === 'mimo_shock'}
	<MimoShockScreen {app} />
{:else if tests.screen === 'mimo_sine'}
	<MimoSineScreen {app} />
{:else if tests.screen === 'mimo_mixed'}
	<MimoMixedScreen {app} />
{:else if tests.screen === 'srs'}
	<SrsScreen {app} />
{:else if tests.screen === 'sine'}
	<SineScreen {app} />
{:else if tests.screen === 'mixed'}
	<MixedScreen {app} />
{:else if tests.screen === 'twr'}
	<TwrScreen {app} />
{:else if tests.screen === 'explorer'}
	<ExplorerScreen {app} />
{:else if tests.screen === 'recorder'}
	<RecorderScreen {app} />
{:else if tests.screen === 'playback'}
	<PlaybackScreen {app} />
{:else if tests.screen === 'modal_test'}
	<ModalTestScreen {app} />
{:else}
	<TestTypeChooser {tests} {liveWord} />
{/if}

<style>
	/* Shown only if the answer is slow; a quick one never draws it. */
	.waiting {
		padding: var(--sp-5);
		font-size: var(--text-sm);
		color: var(--text-3);
		animation: appear 0s linear 0.6s both;
	}

	@keyframes appear {
		from {
			visibility: hidden;
		}
		to {
			visibility: visible;
		}
	}
</style>
