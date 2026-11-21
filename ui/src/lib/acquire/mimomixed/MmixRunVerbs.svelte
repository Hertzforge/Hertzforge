<script lang="ts">
	/**
	 * MIMO Mixed Mode's own verbs, beside the shared ones: Hold, which stops every part's clock with
	 * every part still under control, and Resume; and, while the run holds on a failed gate or a
	 * prediction over a ceiling, Proceed With Your Words. A verb that cannot run says why when it is
	 * focused or hovered.
	 */
	import type { MimoMixedStore } from './mmixStore.svelte';
	import type { MmixVerbId } from './mmixVerbs';

	interface Props {
		mm: MimoMixedStore;
	}
	let { mm }: Props = $props();

	const v = $derived(mm.verbs);
	let hintId = $state<MmixVerbId | null>(null);
	const hint = $derived(hintId && v[hintId].visible && !v[hintId].enabled ? v[hintId].reason : null);

	function go(id: MmixVerbId, fn: () => unknown) {
		if (!v[id].enabled) {
			hintId = id;
			return;
		}
		void fn();
	}

	const tip = (id: MmixVerbId) => ({ onfocus: () => (hintId = id), onmouseenter: () => (hintId = id), onblur: () => hintId === id && (hintId = null), onmouseleave: () => hintId === id && (hintId = null) });
</script>

{#if v.hold.visible || v.resume.visible || v.override.visible}
	<div class="verbs" role="group" aria-label="Test progress" data-testid="mmix-verbs">
		{#if v.override.visible}
			<button class="btn primary" data-testid="act-override-words" aria-disabled={!v.override.enabled} class:off={!v.override.enabled} onclick={() => go('override', () => mm.override())} {...tip('override')}>Proceed With Your Words</button>
		{/if}
		{#if v.hold.visible}
			<button class="btn" data-testid="act-hold" aria-disabled={!v.hold.enabled} class:off={!v.hold.enabled} onclick={() => go('hold', () => mm.hold())} {...tip('hold')}>
				<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><rect x="3.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" /><rect x="8.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" /></svg>
				Hold
			</button>
		{/if}
		{#if v.resume.visible}
			<button class="btn primary" data-testid="act-resume" aria-disabled={!v.resume.enabled} class:off={!v.resume.enabled} onclick={() => go('resume', () => mm.resume())} {...tip('resume')}>
				<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><path d="M4 2.5l7.5 4.5L4 11.5z" fill="currentColor" /></svg>
				Resume
			</button>
		{/if}
		{#if hint}<span class="hint" aria-live="polite" data-testid="mmix-verb-hint">{hint}</span>{/if}
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
