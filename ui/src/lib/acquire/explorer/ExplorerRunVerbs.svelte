<script lang="ts">
	/**
	 * Explorer's own verbs in the run bar, beside the shared ones: Hold, which ends a sweep and
	 * leaves the tone where it is, and Clear Survey, which empties the survey line so it fills again
	 * from where the tone goes next. The steering itself is the steering panel's. A verb that cannot
	 * run says why when it is focused or hovered, as the bar's own verbs do.
	 */
	import type { ExplorerStore } from './explorerStore.svelte';
	import type { ExplorerVerbId } from './explorerVerbs';

	interface Props {
		explorer: ExplorerStore;
	}
	let { explorer }: Props = $props();

	const v = $derived(explorer.verbs);
	const any = $derived(v.hold.visible || v.clearSurvey.visible);
	let hintId = $state<ExplorerVerbId | null>(null);
	const hint = $derived(hintId && v[hintId].visible && !v[hintId].enabled ? v[hintId].reason : null);

	function go(id: ExplorerVerbId, fn: () => unknown) {
		if (!v[id].enabled) {
			hintId = id;
			return;
		}
		void fn();
	}

	const tip = (id: ExplorerVerbId) => ({ onfocus: () => (hintId = id), onmouseenter: () => (hintId = id), onblur: () => hintId === id && (hintId = null), onmouseleave: () => hintId === id && (hintId = null) });
</script>

{#if any}
	<div class="xverbs" role="group" aria-label="Tone" data-testid="explorer-verbs">
		{#if v.hold.visible}
			<button class="btn" data-testid="act-hold" aria-disabled={!v.hold.enabled} class:off={!v.hold.enabled} onclick={() => go('hold', () => explorer.hold())} {...tip('hold')}>
				<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><rect x="3.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" /><rect x="8.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" /></svg>
				Hold
			</button>
		{/if}
		{#if v.clearSurvey.visible}
			<button class="btn" data-testid="act-clear-survey" aria-disabled={!v.clearSurvey.enabled} class:off={!v.clearSurvey.enabled} onclick={() => go('clearSurvey', () => explorer.clearSurvey())} {...tip('clearSurvey')}>Clear Survey</button>
		{/if}
		{#if hint}<span class="hint" aria-live="polite" data-testid="explorer-verb-hint">{hint}</span>{/if}
	</div>
{/if}

<style>
	.xverbs {
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
