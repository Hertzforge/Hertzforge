<script lang="ts">
	/**
	 * TWR's own verbs, in the run bar: on the manual ladder Iterate, Play, a level step down and up
	 * and Go to a level of the table (the level verbs also at a level that failed its gate under the
	 * hold policy); and Freeze or Resume the law's corrections while the run runs. A verb that cannot
	 * run says why when it is focused or hovered, as the bar's own verbs do.
	 */
	import Icon from '$lib/shell/Icon.svelte';
	import { db } from '../words';
	import type { TwrStore } from './twrStore.svelte';
	import type { TwrVerbId } from './twrVerbs';

	interface Props {
		twr: TwrStore;
	}
	let { twr }: Props = $props();

	const v = $derived(twr.verbs);
	const levels = $derived(twr.status?.levels_db ?? []);
	const any = $derived(Object.values(v).some((s) => s.visible));
	const manual = $derived(v.iterate.visible || v.play.visible || v.levelDown.visible);
	let hintId = $state<TwrVerbId | null>(null);
	const hint = $derived(hintId && v[hintId].visible && !v[hintId].enabled ? v[hintId].reason : null);

	function go(id: TwrVerbId, fn: () => unknown) {
		if (!v[id].enabled) {
			hintId = id;
			return;
		}
		void fn();
	}

	const tip = (id: TwrVerbId) => ({ onfocus: () => (hintId = id), onmouseenter: () => (hintId = id), onblur: () => hintId === id && (hintId = null), onmouseleave: () => hintId === id && (hintId = null) });
	const busy = (verb: string) => twr.busy === verb;
</script>

{#snippet spin()}<svg class="spin" viewBox="0 0 12 12" width="12" height="12" aria-hidden="true"><path d="M6 1.5a4.5 4.5 0 104.5 4.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" /></svg>{/snippet}

{#if any}
	<div class="twrverbs" role="group" aria-label="TWR" data-testid="twr-verbs">
		{#if manual}<span class="eyebrow">{v.iterate.visible ? 'Manual' : 'Level'}</span>{/if}
		{#if v.iterate.visible}
			<button class="btn primary" data-testid="act-iterate" aria-disabled={!v.iterate.enabled} class:off={!v.iterate.enabled} onclick={() => go('iterate', () => twr.iterate())} {...tip('iterate')}>
				{#if busy('Iterate')}{@render spin()}{:else}<Icon name="play" size={13} />{/if}
				Iterate
			</button>
		{/if}
		{#if v.play.visible}
			<button class="btn" data-testid="act-play" aria-disabled={!v.play.enabled} class:off={!v.play.enabled} onclick={() => go('play', () => twr.play())} {...tip('play')}>
				{#if busy('Play')}{@render spin()}{/if}
				Play
			</button>
		{/if}
		{#if v.levelDown.visible}
			<div class="level" role="group" aria-label="Level">
				<button class="btn icon" data-testid="twr-level-down" aria-label="Level down 1 dB" aria-disabled={!v.levelDown.enabled} class:off={!v.levelDown.enabled} onclick={() => go('levelDown', () => twr.level(-1))} {...tip('levelDown')}>
					<svg viewBox="0 0 12 12" width="12" height="12" aria-hidden="true"><path d="M2 6h8" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" /></svg>
				</button>
				<span class="lv num" data-testid="twr-level-now">{db(twr.now?.level_db ?? null)}</span>
				<button class="btn icon" data-testid="twr-level-up" aria-label="Level up 1 dB" aria-disabled={!v.levelUp.enabled} class:off={!v.levelUp.enabled} onclick={() => go('levelUp', () => twr.level(1))} {...tip('levelUp')}>
					<svg viewBox="0 0 12 12" width="12" height="12" aria-hidden="true"><path d="M2 6h8M6 2v8" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" /></svg>
				</button>
				<select
					class="field num"
					aria-label="Go to a level of the table"
					data-testid="twr-level-goto"
					value=""
					disabled={!v.rung.enabled}
					title={v.rung.reason ?? 'Go to a level of the table'}
					onchange={(e) => {
						const i = Number(e.currentTarget.value);
						e.currentTarget.value = '';
						if (Number.isInteger(i) && i >= 0) void twr.toRung(i);
					}}
				>
					<option value="" disabled>Go to level</option>
					{#each levels as l, i (i)}<option value={i}>{db(l)}</option>{/each}
				</select>
			</div>
		{/if}
		{#if v.freeze.visible}
			<button class="btn" data-testid="act-freeze" aria-disabled={!v.freeze.enabled} class:off={!v.freeze.enabled} onclick={() => go('freeze', () => twr.freeze(true))} {...tip('freeze')} title="Hold the law's corrections off: the drive in force plays again.">
				{#if busy('Freeze')}{@render spin()}{/if}
				Freeze
			</button>
		{/if}
		{#if v.unfreeze.visible}
			<button class="btn" data-testid="act-unfreeze" aria-disabled={!v.unfreeze.enabled} class:off={!v.unfreeze.enabled} onclick={() => go('unfreeze', () => twr.freeze(false))} {...tip('unfreeze')}>
				{#if busy('Resume Corrections')}{@render spin()}{/if}
				Resume Corrections
			</button>
		{/if}
		{#if hint}<span class="hint" aria-live="polite" data-testid="twr-verb-hint">{hint}</span>{/if}
	</div>
{/if}

<style>
	.twrverbs {
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
