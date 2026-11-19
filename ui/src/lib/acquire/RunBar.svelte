<script lang="ts">
	/**
	 * The run bar: the run's status (where the data comes from, its state, phase, clock and level,
	 * who holds control) above the verbs that apply now. Stop and Abort are offered to every screen
	 * while a run is live; everything else needs control.
	 */
	import type { Snippet } from 'svelte';
	import type { RunBarHost } from './actions';
	import RunStatus from './RunStatus.svelte';
	import RunVerbs from './RunVerbs.svelte';

	interface Props {
		acq: RunBarHost;
		/** Open the acknowledgement dialog for arming past a feasibility exceedance. */
		onoverride: () => void;
		abortKeys: string;
		/**
		 * A window that shows plots on another monitor: the run's state, the level, who holds
		 * control, and Stop and Abort. The verbs that set up and steer the run stay with the main
		 * window.
		 */
		compact?: boolean;
		/** A test type's own verbs, after the shared ones (classical shock's Fire and level moves). */
		verbs?: Snippet;
		/** A test type's own numbers in place of the time at level (classical shock's pulse counts). */
		nums?: Snippet;
	}
	let { acq, onoverride, abortKeys, compact = false, verbs, nums }: Props = $props();
</script>

<section class="runbar" class:compact aria-label="Run control" data-testid="run-bar">
	<RunStatus {acq} {compact} {nums} />
	<RunVerbs {acq} {onoverride} {abortKeys} {compact} {verbs} />
</section>

<style>
	.runbar {
		--abort-ink: var(--status-fail-text);
		display: flex;
		flex-direction: column;
		background: var(--bg-1);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-lg);
	}
</style>
