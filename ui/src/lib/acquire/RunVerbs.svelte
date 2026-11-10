<script lang="ts">
	/**
	 * The verbs that apply now: Apply Setup, Arm, Start, Proceed, New Run, a test type's own, the
	 * level ladder, and Stop and Abort. Stop and Abort are offered to every screen while a run is
	 * live; everything else needs control. A verb that cannot run says why, in words, when it is
	 * focused or hovered, and the next step's reason is always shown.
	 *
	 * In the run bar they sit in a row; in a screen's Run card, large and stacked, the ends full
	 * width at the bottom.
	 */
	import Icon from '$lib/shell/Icon.svelte';
	import type { Snippet } from 'svelte';
	import type { ActionId, RunBarHost } from './actions';
	import { db } from './words';

	interface Props {
		acq: RunBarHost;
		/** Open the acknowledgement dialog for arming past a feasibility exceedance. */
		onoverride: () => void;
		abortKeys: string;
		/** A window on another monitor: Stop and Abort only. */
		compact?: boolean;
		/** A test type's own verbs, after the shared ones (classical shock's Fire and level moves). */
		verbs?: Snippet;
		/** A row in the run bar, or the stack of a Run card. */
		layout?: 'bar' | 'card';
		/** Verbs drawn elsewhere on the screen, left out here. */
		omit?: ActionId[];
	}
	let { acq, onoverride, abortKeys, compact = false, verbs, layout = 'bar', omit = [] }: Props = $props();

	const run = $derived(acq.run);
	const a = $derived(acq.actions);
	const primary = $derived(acq.primary);

	/** The verb under the pointer or the keyboard focus; its reason is read live from the state. */
	let focusId = $state<ActionId | null>(null);
	let pressedId = $state<ActionId | null>(null);
	let stepDb = $state(1);

	const hint = $derived.by(() => {
		for (const id of [pressedId, focusId]) if (id && a[id].visible && !a[id].enabled) return a[id].reason;
		if (primary && a[primary].visible && !a[primary].enabled) return a[primary].reason;
		return null;
	});

	function run_(id: ActionId, fn: () => unknown) {
		if (!a[id].enabled) {
			pressedId = id;
			return;
		}
		pressedId = null;
		void fn();
	}

	function tip(id: ActionId) {
		const leave = () => {
			if (focusId === id) focusId = null;
			if (pressedId === id) pressedId = null;
		};
		return { onfocus: () => (focusId = id), onblur: leave, onmouseenter: () => (focusId = id), onmouseleave: leave };
	}

	const shown = (id: ActionId) => a[id].visible && !omit.includes(id);
</script>

<div class="actions" class:card={layout === 'card'} class:compact>
	{#snippet verb(id: ActionId, label: string, icon: 'play' | 'stop' | 'reset' | 'action' | null, onclick: () => unknown, testid: string)}
		{@const s = a[id]}
		{#if s.visible && !omit.includes(id)}
			<button class="btn" class:primary={primary === id} data-testid={testid} aria-disabled={!s.enabled} class:off={!s.enabled} onclick={() => run_(id, onclick)} {...tip(id)}>
				{#if acq.busy === label}
					<svg class="spin" viewBox="0 0 12 12" width="12" height="12" aria-hidden="true"><path d="M6 1.5a4.5 4.5 0 104.5 4.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" /></svg>
				{:else if icon}
					<Icon name={icon} size={13} />
				{/if}
				{label}
			</button>
		{/if}
	{/snippet}

	{#if !compact}
	<div class="verbs">
		{@render verb('apply', 'Apply Setup', 'action', () => acq.apply(), 'act-apply')}
		{@render verb('arm', 'Arm', null, () => acq.arm(), 'act-arm')}
		{@render verb('override', 'Arm Past Feasibility', null, onoverride, 'act-override')}
		{@render verb('start', 'Start', 'play', () => acq.start(), 'act-start')}
		{@render verb('proceed', 'Proceed', 'play', () => acq.proceed(), 'act-proceed')}
		{@render verb('reset', 'New Run', 'reset', () => acq.reset(), 'act-reset')}
	</div>

	{#if verbs}{@render verbs()}{/if}

	{#if shown('mode')}
		<div class="ladder">
			<span class="eyebrow">Ladder</span>
			<div class="aq-seg sm" role="radiogroup" aria-label="Level ladder" data-testid="mode">
				{#each [['auto', 'Automatic'], ['manual', 'Manual']] as [m, label] (m)}
					<button
						type="button"
						role="radio"
						data-testid="mode-{m}"
						aria-checked={run?.mode === m}
						aria-disabled={!a.mode.enabled}
						onclick={() => run_('mode', () => run?.mode !== m && acq.setMode(m as 'auto' | 'manual'))}
						{...tip('mode')}>{label}</button
					>
				{/each}
			</div>
		</div>
	{/if}

	{#if shown('level')}
		<div class="level" role="group" aria-label="Level">
			<button class="btn icon" data-testid="level-down" aria-label="Level down {stepDb} dB" aria-disabled={!a.level.enabled} class:off={!a.level.enabled} onclick={() => run_('level', () => acq.level(-stepDb))} {...tip('level')}>
				<svg viewBox="0 0 12 12" width="12" height="12" aria-hidden="true"><path d="M2 6h8" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" /></svg>
			</button>
			<select class="field step num" aria-label="Level step" bind:value={stepDb} data-testid="level-step">
				{#each [0.5, 1, 2, 3] as s (s)}<option value={s}>{s} dB</option>{/each}
			</select>
			<button class="btn icon" data-testid="level-up" aria-label="Level up {stepDb} dB" aria-disabled={!a.level.enabled} class:off={!a.level.enabled} onclick={() => run_('level', () => acq.level(stepDb))} {...tip('level')}>
				<svg viewBox="0 0 12 12" width="12" height="12" aria-hidden="true"><path d="M2 6h8M6 2v8" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" /></svg>
			</button>
			<select
				class="field goto num"
				aria-label="Go to a level of the schedule"
				data-testid="level-goto"
				value=""
				disabled={!a.level.enabled}
				title={a.level.reason ?? 'Go to a level of the schedule'}
				onchange={(e) => {
					const v = Number(e.currentTarget.value);
					e.currentTarget.value = '';
					if (Number.isFinite(v)) void acq.goTo(v);
				}}
			>
				<option value="" disabled>Go to level</option>
				{#each run?.levels_db ?? [] as l (l)}<option value={l}>{db(l)}</option>{/each}
			</select>
		</div>
	{/if}

	{/if}

	<div class="ends">
		{#if shown('stop')}
			<button class="btn stop" data-testid="act-stop" aria-disabled={!a.stop.enabled} class:off={!a.stop.enabled} onclick={() => run_('stop', () => acq.stop())} {...tip('stop')}>
				<svg viewBox="0 0 16 16" width="15" height="15" aria-hidden="true"><path d="M2 4h4l8 8" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" /></svg>
				<span class="two"><span>Stop</span><small>Ramped</small></span>
			</button>
		{/if}
		{#if shown('abort')}
			<button class="btn abort" data-testid="act-abort" aria-disabled={!a.abort.enabled} class:off={!a.abort.enabled} aria-keyshortcuts={abortKeys.replace('Cmd', 'Meta').replace('Ctrl', 'Control').replace(' ', '+')} title={layout === 'card' ? `Abort now (${abortKeys})` : undefined} onclick={() => run_('abort', () => acq.abort())} {...tip('abort')}>
				<svg viewBox="0 0 14 14" width="16" height="16" aria-hidden="true"><path d="M4.5 1h5L13 4.5v5L9.5 13h-5L1 9.5v-5z" fill="currentColor" /><path d="M4.8 7h4.4" stroke="var(--status-fail-fill)" stroke-width="1.8" stroke-linecap="round" /></svg>
				<span class="two"><span>Abort</span><small>Immediate</small></span>
				<kbd>{abortKeys}</kbd>
			</button>
		{/if}
	</div>
</div>

{#if hint}
	<p class="hint" data-testid="run-hint" aria-live="polite">
		<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><circle cx="7" cy="7" r="6" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M7 6.2v4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /><circle cx="7" cy="4.1" r="0.9" fill="currentColor" /></svg>
		{hint}
	</p>
{/if}

<style>
	.actions {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2) var(--sp-4);
		padding: 10px var(--sp-4);
	}

	.verbs {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-2);
	}

	.verbs:empty {
		display: none;
	}

	.btn.off {
		opacity: 0.45;
		cursor: default;
	}

	.btn.primary {
		min-width: 92px;
	}

	.ladder,
	.level {
		display: flex;
		align-items: center;
		gap: 6px;
	}

	.ladder .eyebrow {
		margin-right: 2px;
	}

	.level .field {
		height: var(--control-height);
		width: auto;
		font-size: var(--text-xs);
	}

	.level .step {
		min-width: 74px;
	}

	.level .goto {
		min-width: 118px;
	}

	.ends {
		display: flex;
		gap: var(--sp-2);
		margin-left: auto;
	}

	.two {
		display: inline-flex;
		flex-direction: column;
		align-items: flex-start;
		line-height: 1.05;
	}

	.two small {
		font-size: var(--text-2xs);
		font-weight: var(--weight-regular);
		opacity: 0.8;
	}

	.stop,
	.abort {
		height: 38px;
		padding: 0 14px 0 11px;
	}

	.stop:not(.off) {
		color: var(--amber-1);
		background: var(--amber-weak);
		border-color: var(--amber-line);
	}

	.abort {
		height: 38px;
		padding: 0 14px 0 11px;
	}

	.abort:hover:not(.off) {
		background: var(--amber-1) !important;
		border-color: var(--amber-1) !important;
	}

	.abort kbd {
		margin-left: 4px;
		color: var(--abort-ink);
		background: transparent;
		border-color: color-mix(in srgb, var(--abort-ink) 40%, transparent);
	}

	.hint {
		display: flex;
		align-items: center;
		gap: 6px;
		margin: -2px var(--sp-4) 10px;
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	.hint svg {
		flex: none;
		color: var(--text-3);
	}

	.spin {
		animation: spin 0.8s linear infinite;
	}

	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}

	@media (max-width: 900px) {
		.ends {
			width: 100%;
			margin-left: 0;
		}
		.ends .btn {
			flex: 1;
		}
	}

	/* A Run card: the verbs large and stacked, Stop and Abort side by side across the bottom. */
	.actions.card {
		--abort-ink: var(--status-fail-text);
		flex-direction: column;
		flex-wrap: nowrap;
		align-items: stretch;
		gap: var(--sp-3);
		padding: 0;
	}

	.card .verbs {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(110px, 1fr));
	}

	.card .verbs:empty {
		display: none;
	}

	.card .verbs .btn {
		height: 38px;
		font-size: var(--text-md);
	}

	.card .ladder,
	.card .level {
		flex-wrap: wrap;
	}

	.card .level .goto {
		flex: 1;
	}

	.card .ends {
		display: grid;
		grid-template-columns: 1fr 1fr;
		margin-left: 0;
	}

	.card .stop,
	.card .abort {
		justify-content: flex-start;
		height: 48px;
		min-width: 0;
	}

	/* The key is named on hover in a card, where the button is too narrow to show it. */
	.card .abort kbd {
		display: none;
	}

	.card + .hint {
		margin: var(--sp-2) 0 0;
	}
</style>
