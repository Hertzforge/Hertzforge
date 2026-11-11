<script lang="ts">
	/**
	 * Mixed Mode's own verbs, in the Run card beside the shared ones: Hold, which stops parts time,
	 * every part's clock and window and the level clock with every part still under control, and
	 * Resume. A verb that cannot run says why when it is focused or hovered.
	 */
	import type { MixedStore } from './mixedStore.svelte';
	import type { MixedVerbId } from './mixedVerbs';

	interface Props {
		mixed: MixedStore;
	}
	let { mixed }: Props = $props();

	const v = $derived(mixed.verbs);
	let hintId = $state<MixedVerbId | null>(null);
	const hint = $derived(hintId && v[hintId].visible && !v[hintId].enabled ? v[hintId].reason : null);

	function go(id: MixedVerbId, fn: () => unknown) {
		if (!v[id].enabled) {
			hintId = id;
			return;
		}
		void fn();
	}

	const tip = (id: MixedVerbId) => ({ onfocus: () => (hintId = id), onmouseenter: () => (hintId = id), onblur: () => hintId === id && (hintId = null), onmouseleave: () => hintId === id && (hintId = null) });
</script>

{#if v.hold.visible || v.resume.visible}
	<div class="verbs" role="group" aria-label="Test progress" data-testid="mixed-verbs">
		{#if v.hold.visible}
			<button class="btn" data-testid="act-hold" aria-disabled={!v.hold.enabled} class:off={!v.hold.enabled} onclick={() => go('hold', () => mixed.hold())} {...tip('hold')}>
				<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><rect x="3.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" /><rect x="8.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" /></svg>
				Hold
			</button>
		{/if}
		{#if v.resume.visible}
			<button class="btn primary" data-testid="act-resume" aria-disabled={!v.resume.enabled} class:off={!v.resume.enabled} onclick={() => go('resume', () => mixed.resume())} {...tip('resume')}>
				<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><path d="M4 2.5l7.5 4.5L4 11.5z" fill="currentColor" /></svg>
				Resume
			</button>
		{/if}
		{#if hint}<span class="hint" aria-live="polite" data-testid="mixed-verb-hint">{hint}</span>{/if}
	</div>
{/if}

<style>
	.verbs {
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
