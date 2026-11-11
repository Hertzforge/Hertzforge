<script lang="ts">
	/**
	 * Acquire & Control: the test-type chooser, or the chosen test type's own screen. Which one is
	 * on show is kept in the shell state, so it survives moving between workspaces. Until the
	 * control service has said whether a run is live, neither is shown, so a screen opened during
	 * a live run goes straight to it.
	 */
	import type { AppState } from '$lib/app/state.svelte';
	import RandomScreen from '$lib/acquire/RandomScreen.svelte';
	import MimoRandomScreen from '$lib/acquire/mimo/MimoRandomScreen.svelte';
	import ShockScreen from '$lib/acquire/shock/ShockScreen.svelte';
	import SrsScreen from '$lib/acquire/srs/SrsScreen.svelte';
	import SineScreen from '$lib/acquire/sine/SineScreen.svelte';
	import MixedScreen from '$lib/acquire/mixed/MixedScreen.svelte';
	import TwrScreen from '$lib/acquire/twr/TwrScreen.svelte';
	import { TWR_STATE_WORDS } from '$lib/acquire/twr/twrWords';
	import ExplorerScreen from '$lib/acquire/explorer/ExplorerScreen.svelte';
	import { EXPLORER_STATE_WORDS } from '$lib/acquire/explorer/explorerWords';
	import RecorderScreen from '$lib/acquire/recorder/RecorderScreen.svelte';
	import { RECORDER_STATE_WORDS } from '$lib/acquire/recorder/recorderWords';
	import TestTypeChooser from '$lib/acquire/TestTypeChooser.svelte';
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
										: null
	);
</script>

{#if tests.deciding}
	<p class="waiting" role="status" data-testid="acquire-waiting">Waiting for the control service</p>
{:else if tests.screen === 'random'}
	<RandomScreen {app} />
{:else if tests.screen === 'mimo_random'}
	<MimoRandomScreen {app} />
{:else if tests.screen === 'shock'}
	<ShockScreen {app} />
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
