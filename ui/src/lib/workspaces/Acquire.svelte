<script lang="ts">
	/**
	 * Acquire & Control: the test-type chooser, or the chosen test type's own screen. Which one is
	 * on show is kept in the shell state, so it survives moving between workspaces. Until the
	 * control service has said whether a run is live, neither is shown, so a screen opened during
	 * a live run goes straight to it.
	 */
	import type { AppState } from '$lib/app/state.svelte';
	import RandomScreen from '$lib/acquire/RandomScreen.svelte';
	import TestTypeChooser from '$lib/acquire/TestTypeChooser.svelte';
	import { STATE_WORDS } from '$lib/acquire/words';

	interface Props {
		app: AppState;
	}
	let { app }: Props = $props();

	const tests = $derived(app.tests);
	const liveWord = $derived(app.acquire.isLive && app.acquire.run ? STATE_WORDS[app.acquire.run.state] : null);
</script>

{#if tests.deciding}
	<p class="waiting" role="status" data-testid="acquire-waiting">Waiting for the control service</p>
{:else if tests.screen === 'random'}
	<RandomScreen {app} />
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
