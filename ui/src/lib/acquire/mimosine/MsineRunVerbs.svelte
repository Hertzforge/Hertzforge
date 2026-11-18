<script lang="ts">
	/**
	 * MIMO Sine's own verbs, beside the shared ones in the Run card: Hold and Resume, which freeze
	 * and restart the test's progress with every drive still under control, and on a sweep Reverse,
	 * Jog and Go to, which turn the sweep and move the frequency with the drive vector held. A verb
	 * that cannot run says why when it is focused or hovered.
	 */
	import type { ActionState } from '../actions';
	import type { MsineVerbId } from './msineVerbs';

	interface Host {
		verbs: Record<MsineVerbId, ActionState>;
		hold(): unknown;
		resume(): unknown;
		reverse(): unknown;
		jog(deltaHz: number): unknown;
		gotoHz(hz: number): Promise<number | null>;
	}

	interface Props {
		ms: Host;
	}
	let { ms }: Props = $props();

	const v = $derived(ms.verbs);
	const any = $derived(Object.values(v).some((s) => s.visible));
	let hintId = $state<MsineVerbId | null>(null);
	const hint = $derived(hintId && v[hintId].visible && !v[hintId].enabled ? v[hintId].reason : null);
	let jogHz = $state(10);
	let gotoText = $state('');
	const gotoHz = $derived(Number(gotoText));
	const gotoOk = $derived(gotoText.trim() !== '' && Number.isFinite(gotoHz) && gotoHz > 0);

	function go(id: MsineVerbId, fn: () => unknown) {
		if (!v[id].enabled) {
			hintId = id;
			return;
		}
		void fn();
	}

	async function goto() {
		if (!gotoOk) return;
		if ((await ms.gotoHz(gotoHz)) !== null) gotoText = '';
	}

	const tip = (id: MsineVerbId) => ({ onfocus: () => (hintId = id), onmouseenter: () => (hintId = id), onblur: () => hintId === id && (hintId = null), onmouseleave: () => hintId === id && (hintId = null) });
</script>

{#if any}
	<div class="verbs" role="group" aria-label="Test progress" data-testid="msine-verbs">
		{#if v.hold.visible}
			<button class="btn" data-testid="act-hold" aria-disabled={!v.hold.enabled} class:off={!v.hold.enabled} onclick={() => go('hold', () => ms.hold())} {...tip('hold')}>Hold</button>
		{/if}
		{#if v.resume.visible}
			<button class="btn primary" data-testid="act-resume" aria-disabled={!v.resume.enabled} class:off={!v.resume.enabled} onclick={() => go('resume', () => ms.resume())} {...tip('resume')}>Resume</button>
		{/if}
		{#if v.reverse.visible}
			<button class="btn" data-testid="act-reverse" aria-disabled={!v.reverse.enabled} class:off={!v.reverse.enabled} onclick={() => go('reverse', () => ms.reverse())} {...tip('reverse')}>Reverse</button>
		{/if}
		{#if v.jog.visible}
			<div class="jog" role="group" aria-label="Jog the frequency">
				<button class="btn icon" data-testid="jog-down" aria-label="Jog down {jogHz} Hz" aria-disabled={!v.jog.enabled} class:off={!v.jog.enabled} onclick={() => go('jog', () => ms.jog(-jogHz))} {...tip('jog')}>-</button>
				<select class="field num step" aria-label="Jog step" bind:value={jogHz} data-testid="jog-step">
					{#each [1, 10, 100] as s (s)}<option value={s}>{s} Hz</option>{/each}
				</select>
				<button class="btn icon" data-testid="jog-up" aria-label="Jog up {jogHz} Hz" aria-disabled={!v.jog.enabled} class:off={!v.jog.enabled} onclick={() => go('jog', () => ms.jog(jogHz))} {...tip('jog')}>+</button>
			</div>
		{/if}
		{#if v.goto.visible}
			<form class="goto" onsubmit={(e) => (e.preventDefault(), go('goto', goto))}>
				<input class="field num" aria-label="Go to frequency, Hz" placeholder="Hz" inputmode="decimal" bind:value={gotoText} data-testid="goto-hz" {...tip('goto')} />
				<button class="btn" type="submit" data-testid="act-goto" aria-disabled={!v.goto.enabled || !gotoOk} class:off={!v.goto.enabled || !gotoOk} {...tip('goto')}>Go to</button>
			</form>
		{/if}
		{#if hint}<span class="hint" aria-live="polite" data-testid="msine-verb-hint">{hint}</span>{/if}
	</div>
{/if}

<style>
	.verbs {
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
