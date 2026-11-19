<script lang="ts">
	/**
	 * The Recorder's own verbs in the run bar, beside the shared ones, as the trigger mode offers
	 * them: Arm and Disarm (manual and level), Record (manual) and Cut (from the start and manual).
	 * A verb that cannot run says why when it is focused or hovered, as the bar's own verbs do.
	 */
	import type { RecorderStore } from './recorderStore.svelte';
	import { RECORDER_VERB_IDS, type RecorderVerbId } from './recorderVerbs';

	interface Props {
		recorder: RecorderStore;
	}
	let { recorder }: Props = $props();

	const v = $derived(recorder.verbs);
	const any = $derived(RECORDER_VERB_IDS.some((id) => v[id].visible));
	let hintId = $state<RecorderVerbId | null>(null);
	const hint = $derived(hintId && v[hintId].visible && !v[hintId].enabled ? v[hintId].reason : null);

	const LABEL: Record<RecorderVerbId, string> = { arm: 'Arm', disarm: 'Disarm', record: 'Record', cut: 'Cut' };
	const RUN: Record<RecorderVerbId, () => unknown> = {
		arm: () => recorder.armTrigger(),
		disarm: () => recorder.disarm(),
		record: () => recorder.record(),
		cut: () => recorder.cut()
	};

	function go(id: RecorderVerbId) {
		if (!v[id].enabled) {
			hintId = id;
			return;
		}
		void RUN[id]();
	}

	const tip = (id: RecorderVerbId) => ({ onfocus: () => (hintId = id), onmouseenter: () => (hintId = id), onblur: () => hintId === id && (hintId = null), onmouseleave: () => hintId === id && (hintId = null) });
</script>

{#if any}
	<div class="rverbs" role="group" aria-label="Recording" data-testid="recorder-verbs">
		{#each RECORDER_VERB_IDS as id (id)}
			{#if v[id].visible}
				<button class="btn" class:primary={id === 'record' || id === 'arm'} data-testid="act-rec-{id}" aria-disabled={!v[id].enabled} class:off={!v[id].enabled} onclick={() => go(id)} {...tip(id)}>
					{#if id === 'record'}
						<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><circle cx="7" cy="7" r="5" fill="currentColor" /></svg>
					{:else if id === 'cut'}
						<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><rect x="3" y="3" width="8" height="8" rx="1" fill="currentColor" /></svg>
					{:else if id === 'arm'}
						<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><circle cx="7" cy="7" r="5" fill="none" stroke="currentColor" stroke-width="1.5" /><path d="M7 2a5 5 0 010 10z" fill="currentColor" /></svg>
					{/if}
					{LABEL[id]}
				</button>
			{/if}
		{/each}
		{#if hint}<span class="hint" aria-live="polite" data-testid="recorder-verb-hint">{hint}</span>{/if}
	</div>
{/if}

<style>
	.rverbs {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2);
	}

	.btn.off {
		opacity: 0.55;
		cursor: default;
	}

	.hint {
		font-size: var(--text-xs);
		color: var(--text-2);
	}
</style>
