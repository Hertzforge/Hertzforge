<script lang="ts">
	/**
	 * The manual ladder's verbs, in the run bar: Fire, a level step down and up, Go to a level of the
	 * table, Train and Burst. Shown only on a manual run; a verb that cannot run says why when it is
	 * focused or hovered, as the bar's own verbs do. SRS shock shows the same verbs, with no train
	 * and no burst.
	 */
	import Icon from '$lib/shell/Icon.svelte';
	import { db } from '../words';
	import type { ShockVerbHost, ShockVerbId } from './shockVerbs';

	interface Props {
		shock: ShockVerbHost;
	}
	let { shock }: Props = $props();

	const v = $derived(shock.verbs);
	const levels = $derived(shock.status?.levels_db ?? []);
	const any = $derived(Object.values(v).some((s) => s.visible));
	let hintId = $state<ShockVerbId | null>(null);
	const hint = $derived(hintId && v[hintId].visible && !v[hintId].enabled ? v[hintId].reason : null);

	function go(id: ShockVerbId, fn: () => unknown) {
		if (!v[id].enabled) {
			hintId = id;
			return;
		}
		void fn();
	}

	const tip = (id: ShockVerbId) => ({ onfocus: () => (hintId = id), onmouseenter: () => (hintId = id), onblur: () => hintId === id && (hintId = null), onmouseleave: () => hintId === id && (hintId = null) });
	const busy = (verb: string) => shock.busy === verb;
</script>

{#if any}
	<div class="shockverbs" role="group" aria-label="Manual ladder" data-testid="shock-verbs">
		<span class="eyebrow">Manual</span>
		{#if v.fire.visible}
			<button class="btn primary" data-testid="act-fire" aria-disabled={!v.fire.enabled} class:off={!v.fire.enabled} onclick={() => go('fire', () => shock.fire())} {...tip('fire')}>
				{#if busy('Fire')}<svg class="spin" viewBox="0 0 12 12" width="12" height="12" aria-hidden="true"><path d="M6 1.5a4.5 4.5 0 104.5 4.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" /></svg>{:else}<Icon name="play" size={13} />{/if}
				Fire
			</button>
		{/if}
		{#if v.levelDown.visible}
			<div class="level" role="group" aria-label="Level">
				<button class="btn icon" data-testid="shock-level-down" aria-label="Level down 1 dB" aria-disabled={!v.levelDown.enabled} class:off={!v.levelDown.enabled} onclick={() => go('levelDown', () => shock.level(-1))} {...tip('levelDown')}>
					<svg viewBox="0 0 12 12" width="12" height="12" aria-hidden="true"><path d="M2 6h8" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" /></svg>
				</button>
				<span class="lv num" data-testid="shock-level-now">{db(shock.now?.level_db ?? null)}</span>
				<button class="btn icon" data-testid="shock-level-up" aria-label="Level up 1 dB" aria-disabled={!v.levelUp.enabled} class:off={!v.levelUp.enabled} onclick={() => go('levelUp', () => shock.level(1))} {...tip('levelUp')}>
					<svg viewBox="0 0 12 12" width="12" height="12" aria-hidden="true"><path d="M2 6h8M6 2v8" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" /></svg>
				</button>
				<select
					class="field num"
					aria-label="Go to a level of the table"
					data-testid="shock-level-goto"
					value=""
					disabled={!v.rung.enabled}
					title={v.rung.reason ?? 'Go to a level of the table'}
					onchange={(e) => {
						const i = Number(e.currentTarget.value);
						e.currentTarget.value = '';
						if (Number.isInteger(i) && i >= 0) void shock.toRung(i);
					}}
				>
					<option value="" disabled>Go to level</option>
					{#each levels as l, i (i)}<option value={i}>{db(l)}</option>{/each}
				</select>
			</div>
		{/if}
		{#if v.train.visible}
			<button class="btn" data-testid="act-train" aria-disabled={!v.train.enabled} class:off={!v.train.enabled} onclick={() => go('train', () => shock.train())} {...tip('train')}>Train</button>
		{/if}
		{#if v.burst.visible}
			<button class="btn" data-testid="act-burst" aria-disabled={!v.burst.enabled} class:off={!v.burst.enabled} onclick={() => go('burst', () => shock.burst())} {...tip('burst')}>Burst</button>
		{/if}
		{#if hint}<span class="hint" aria-live="polite" data-testid="shock-verb-hint">{hint}</span>{/if}
	</div>
{/if}

<style>
	.shockverbs {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2);
	}

	.eyebrow {
		font-size: var(--text-2xs);
		font-weight: var(--weight-strong);
		color: var(--text-3);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	.level {
		display: flex;
		align-items: center;
		gap: 4px;
	}

	.lv {
		/* The level never shrinks under its words ("-12.0 dB"): a min-width alone lets a flex item shrink to it. */
		flex: none;
		min-width: 7ch;
		white-space: nowrap;
		text-align: center;
		font-size: var(--text-sm);
	}

	.btn.off {
		opacity: 0.55;
		cursor: default;
	}

	.hint {
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	.spin {
		animation: spin 0.9s linear infinite;
	}

	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}
</style>
