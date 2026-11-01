<script lang="ts">
	/**
	 * The run's state as a shape, a word and a color together. Running pulses; a finished run shows
	 * its outcome, and every outcome but a completed schedule or the operator's stop reads as an abort.
	 */
	import type { RandomState } from '$lib/protocol/types';
	import { OUTCOME_WORDS, outcomeTone, STATE_WORDS } from './words';

	interface Props {
		run: RandomState | null;
	}
	let { run }: Props = $props();

	const state = $derived(run?.state ?? 'unconfigured');
	const outcome = $derived(run?.end?.outcome ?? null);
	const tone = $derived(outcome ? outcomeTone(outcome) : null);
	const word = $derived(state === 'finished' && outcome ? OUTCOME_WORDS[outcome] : STATE_WORDS[state]);
	const shape = $derived.by(() => {
		if (state === 'finished') return tone === 'pass' ? 'check' : tone === 'info' ? 'square' : 'octagon';
		return state;
	});
</script>

<span class="chip {state}" class:abort={tone === 'fail'} class:done={tone === 'pass'} data-testid="run-state" data-state={state} data-outcome={outcome ?? ''} role="status" aria-label="Run state: {word}">
	<svg viewBox="0 0 14 14" width="13" height="13" aria-hidden="true">
		{#if shape === 'unconfigured'}
			<circle cx="7" cy="7" r="5" fill="none" stroke="currentColor" stroke-width="1.4" stroke-dasharray="2.2 2" />
		{:else if shape === 'configured'}
			<circle cx="7" cy="7" r="5" fill="none" stroke="currentColor" stroke-width="1.5" />
		{:else if shape === 'blocked'}
			<path d="M4.8 1.5h4.4L12.5 4.8v4.4L9.2 12.5H4.8L1.5 9.2V4.8z" fill="none" stroke="currentColor" stroke-width="1.4" />
			<path d="M4.5 7h5" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" />
		{:else if shape === 'armed'}
			<circle cx="7" cy="7" r="5" fill="none" stroke="currentColor" stroke-width="1.5" />
			<path d="M7 2a5 5 0 010 10z" fill="currentColor" />
		{:else if shape === 'running'}
			<circle cx="7" cy="7" r="5.2" fill="currentColor" />
		{:else if shape === 'review'}
			<rect x="3.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" />
			<rect x="8.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" />
		{:else if shape === 'stopping'}
			<path d="M2 3.5h10L7 11.5z" fill="currentColor" />
		{:else if shape === 'check'}
			<circle cx="7" cy="7" r="6" fill="none" stroke="currentColor" stroke-width="1.3" />
			<path d="M4.2 7.2l1.9 1.9 3.8-4" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" />
		{:else if shape === 'square'}
			<rect x="2.5" y="2.5" width="9" height="9" rx="1.3" fill="currentColor" />
		{:else}
			<path d="M4.5 1h5L13 4.5v5L9.5 13h-5L1 9.5v-5z" fill="currentColor" />
			<path d="M5 5l4 4M9 5l-4 4" stroke="var(--status-fail-text)" stroke-width="1.6" stroke-linecap="round" />
		{/if}
	</svg>
	<span>{word}</span>
</span>

<style>
	.chip {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		height: 28px;
		padding: 0 11px 0 9px;
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
		color: var(--text-2);
		background: var(--bg-2);
		border: 1px solid var(--line-2);
		border-radius: 14px;
		white-space: nowrap;
	}

	.chip.configured,
	.chip.armed {
		color: var(--text-1);
		border-color: var(--line-3);
	}

	.chip.running,
	.chip.review,
	.chip.stopping {
		color: var(--blue-1);
		border-color: var(--blue-line);
		background: var(--blue-weak);
	}

	.chip.running svg {
		animation: pulse 1.2s ease-in-out infinite;
	}

	.chip.blocked {
		color: var(--amber-1);
		border-color: var(--amber-line);
		background: var(--amber-weak);
	}

	.chip.done {
		color: var(--blue-1);
		border-color: var(--blue-line);
	}

	.chip.abort {
		color: var(--status-fail-text);
		background: var(--status-fail-fill);
		border-color: var(--status-fail-fill);
	}

	.chip.abort svg path:first-child {
		fill: var(--status-fail-text);
	}

	.chip.abort svg path:last-child {
		stroke: var(--status-fail-fill);
	}

	@keyframes pulse {
		50% {
			opacity: 0.4;
		}
	}
</style>
