<script lang="ts">
	/**
	 * Sine's own verbs, in the run bar beside the shared ones: Hold and Resume, which freeze and
	 * restart the test's progress with the tone still under control, and on a sweep Reverse, Jog and
	 * Go to, which turn the sweep and move the frequency with no break in the tone's phase. A verb
	 * that cannot run says why when it is focused or hovered, as the bar's own verbs do.
	 */
	import type { SineStore } from './sineStore.svelte';
	import type { SineVerbId } from './sineVerbs';

	interface Props {
		sine: SineStore;
	}
	let { sine }: Props = $props();

	const v = $derived(sine.verbs);
	const any = $derived(Object.values(v).some((s) => s.visible));
	let hintId = $state<SineVerbId | null>(null);
	const hint = $derived(hintId && v[hintId].visible && !v[hintId].enabled ? v[hintId].reason : null);
	let jogHz = $state(10);
	let gotoText = $state('');
	const gotoHz = $derived(Number(gotoText));
	const gotoOk = $derived(gotoText.trim() !== '' && Number.isFinite(gotoHz) && gotoHz > 0);

	function go(id: SineVerbId, fn: () => unknown) {
		if (!v[id].enabled) {
			hintId = id;
			return;
		}
		void fn();
	}

	async function goto() {
		if (!gotoOk) return;
		if ((await sine.gotoHz(gotoHz)) !== null) gotoText = '';
	}

	const tip = (id: SineVerbId) => ({ onfocus: () => (hintId = id), onmouseenter: () => (hintId = id), onblur: () => hintId === id && (hintId = null), onmouseleave: () => hintId === id && (hintId = null) });
</script>

{#if any}
	<div class="sineverbs" role="group" aria-label="Test progress" data-testid="sine-verbs">
		{#if v.hold.visible}
			<button class="btn" data-testid="act-hold" aria-disabled={!v.hold.enabled} class:off={!v.hold.enabled} onclick={() => go('hold', () => sine.hold())} {...tip('hold')}>
				<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><rect x="3.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" /><rect x="8.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" /></svg>
				Hold
			</button>
		{/if}
		{#if v.resume.visible}
			<button class="btn primary" data-testid="act-resume" aria-disabled={!v.resume.enabled} class:off={!v.resume.enabled} onclick={() => go('resume', () => sine.resume())} {...tip('resume')}>
				<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><path d="M4 2.5l7.5 4.5L4 11.5z" fill="currentColor" /></svg>
				Resume
			</button>
		{/if}
		{#if v.reverse.visible}
			<button class="btn" data-testid="act-reverse" aria-disabled={!v.reverse.enabled} class:off={!v.reverse.enabled} onclick={() => go('reverse', () => sine.reverse())} {...tip('reverse')}>
				<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><path d="M4 2v10M1.5 4.5L4 2l2.5 2.5M10 12V2M7.5 9.5L10 12l2.5-2.5" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" /></svg>
				Reverse
			</button>
		{/if}
		{#if v.jog.visible}
			<div class="jog" role="group" aria-label="Jog the frequency">
				<button class="btn icon" data-testid="jog-down" aria-label="Jog down {jogHz} Hz" aria-disabled={!v.jog.enabled} class:off={!v.jog.enabled} onclick={() => go('jog', () => sine.jog(-jogHz))} {...tip('jog')}>
					<svg viewBox="0 0 12 12" width="12" height="12" aria-hidden="true"><path d="M2 6h8" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" /></svg>
				</button>
				<select class="field num step" aria-label="Jog step" bind:value={jogHz} data-testid="jog-step">
					{#each [1, 10, 100] as s (s)}<option value={s}>{s} Hz</option>{/each}
				</select>
				<button class="btn icon" data-testid="jog-up" aria-label="Jog up {jogHz} Hz" aria-disabled={!v.jog.enabled} class:off={!v.jog.enabled} onclick={() => go('jog', () => sine.jog(jogHz))} {...tip('jog')}>
					<svg viewBox="0 0 12 12" width="12" height="12" aria-hidden="true"><path d="M2 6h8M6 2v8" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" /></svg>
				</button>
			</div>
		{/if}
		{#if v.goto.visible}
			<form class="goto" onsubmit={(e) => (e.preventDefault(), go('goto', goto))}>
				<input class="field num" aria-label="Go to frequency, Hz" placeholder="Hz" inputmode="decimal" bind:value={gotoText} data-testid="goto-hz" {...tip('goto')} />
				<button class="btn" type="submit" data-testid="act-goto" aria-disabled={!v.goto.enabled || !gotoOk} class:off={!v.goto.enabled || !gotoOk} {...tip('goto')}>Go to</button>
			</form>
		{/if}
		{#if hint}<span class="hint" aria-live="polite" data-testid="sine-verb-hint">{hint}</span>{/if}
	</div>
{/if}

<style>
	.sineverbs {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2);
	}

	.jog,
	.goto {
		display: flex;
		align-items: center;
		gap: 4px;
	}

	.step {
		width: 76px;
	}

	.goto input {
		width: 84px;
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
