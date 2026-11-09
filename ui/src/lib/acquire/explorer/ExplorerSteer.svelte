<script lang="ts">
	/**
	 * The steering panel: the tone's frequency large, fine and coarse steps down and up, and a field
	 * to go to a frequency; the level in volts peak with fine and coarse steps in dB and a field to
	 * set it in volts; a sweep up or down at a rate, and Hold. Every number shown is the service's:
	 * the frequency and the level as the run reports them, and under them the service's answer to
	 * the last verb, clamped or not. While the panel holds the keyboard focus, Left and Right step
	 * the frequency, Up and Down the level, with Shift for the coarse step and H to hold; one press
	 * is one step, and a key held down does not repeat.
	 */
	import { formatNumber } from '$lib/viz/format';
	import type { ExplorerStore } from './explorerStore.svelte';
	import { coarseOct, fineOct } from './explorerSetup';
	import { steerForKey, type ExplorerVerbId } from './explorerVerbs';
	import { dbStep, hz, octaveStep, vpk } from './explorerWords';

	interface Props {
		explorer: ExplorerStore;
	}
	let { explorer }: Props = $props();

	const v = $derived(explorer.verbs);
	const now = $derived(explorer.now);
	const steps = $derived(explorer.setup.steps);
	const any = $derived(Object.values(v).some((s) => s.visible));
	let hintId = $state<ExplorerVerbId | null>(null);
	const hint = $derived(hintId && v[hintId].visible && !v[hintId].enabled ? v[hintId].reason : null);
	let gotoText = $state('');
	let voltsText = $state('');
	let rateText = $state('');
	const gotoHz = $derived(Number(gotoText));
	const gotoOk = $derived(gotoText.trim() !== '' && Number.isFinite(gotoHz) && gotoHz > 0);
	const volts = $derived(Number(voltsText));
	const voltsOk = $derived(voltsText.trim() !== '' && Number.isFinite(volts) && volts >= 0);
	const rate = $derived(rateText.trim() === '' ? steps.sweep_oct_per_min : Number(rateText));
	const rateOk = $derived(Number.isFinite(rate) && rate > 0);
	const n = (x: number, sig = 4) => formatNumber(x, sig);

	function go(id: ExplorerVerbId, fn: () => unknown) {
		if (!v[id].enabled) {
			hintId = id;
			return;
		}
		hintId = null;
		void fn();
	}

	async function goto() {
		if (!gotoOk) return;
		if ((await explorer.goToHz(gotoHz)) !== null) gotoText = '';
	}

	async function setVolts() {
		if (!voltsOk) return;
		if ((await explorer.setVpk(volts)) !== null) voltsText = '';
	}

	function onKey(e: KeyboardEvent) {
		// A field takes its own keys; the panel steers only from its buttons and itself.
		const t = e.target as HTMLElement | null;
		if (t && (t.tagName === 'INPUT' || t.tagName === 'SELECT' || t.tagName === 'TEXTAREA')) return;
		const a = steerForKey(e, steps);
		if (!a) return;
		e.preventDefault();
		if ('hold' in a) go('hold', () => explorer.hold());
		else if ('step_oct' in a) go('frequency', () => explorer.stepFrequency(a.step_oct));
		else go('levelStep', () => explorer.stepLevel(a.step_db));
	}

	const tip = (id: ExplorerVerbId) => ({ onfocus: () => (hintId = id), onmouseenter: () => (hintId = id), onblur: () => hintId === id && (hintId = null), onmouseleave: () => hintId === id && (hintId = null) });
</script>

{#if any}
	<!-- The panel takes the keyboard focus so that its keys steer only while it holds it. -->
	<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
	<section class="steer" role="group" aria-label="Steer the tone" tabindex="0" onkeydown={onKey} data-testid="explorer-steer">
		<div class="part">
			<div class="head">
				<h3>Frequency</h3>
				<span class="big num" data-testid="explorer-steer-hz">{hz(now?.frequency_hz)}</span>
			</div>
			<div class="row" role="group" aria-label="Step the frequency">
				<button class="btn sm" data-testid="explorer-freq-coarse-down" aria-label="Down {octaveStep(coarseOct(steps))}" aria-disabled={!v.frequency.enabled} class:off={!v.frequency.enabled} onclick={() => go('frequency', () => explorer.stepFrequency(-coarseOct(steps)))} {...tip('frequency')}>-{octaveStep(coarseOct(steps))}</button>
				<button class="btn sm" data-testid="explorer-freq-fine-down" aria-label="Down {octaveStep(fineOct(steps))}" aria-disabled={!v.frequency.enabled} class:off={!v.frequency.enabled} onclick={() => go('frequency', () => explorer.stepFrequency(-fineOct(steps)))} {...tip('frequency')}>-{octaveStep(fineOct(steps))}</button>
				<button class="btn sm" data-testid="explorer-freq-fine-up" aria-label="Up {octaveStep(fineOct(steps))}" aria-disabled={!v.frequency.enabled} class:off={!v.frequency.enabled} onclick={() => go('frequency', () => explorer.stepFrequency(fineOct(steps)))} {...tip('frequency')}>+{octaveStep(fineOct(steps))}</button>
				<button class="btn sm" data-testid="explorer-freq-coarse-up" aria-label="Up {octaveStep(coarseOct(steps))}" aria-disabled={!v.frequency.enabled} class:off={!v.frequency.enabled} onclick={() => go('frequency', () => explorer.stepFrequency(coarseOct(steps)))} {...tip('frequency')}>+{octaveStep(coarseOct(steps))}</button>
			</div>
			<form class="row" onsubmit={(e) => (e.preventDefault(), go('frequency', goto))}>
				<input class="field num" aria-label="Go to frequency, Hz" placeholder="Hz" inputmode="decimal" bind:value={gotoText} data-testid="explorer-goto-hz" {...tip('frequency')} />
				<button class="btn sm" type="submit" data-testid="explorer-goto" aria-disabled={!v.frequency.enabled || !gotoOk} class:off={!v.frequency.enabled || !gotoOk} {...tip('frequency')}>Go to</button>
			</form>
		</div>

		<div class="part">
			<div class="head">
				<h3>Level</h3>
				<span class="big num" data-testid="explorer-steer-vpk">{vpk(now?.drive_vpk)}</span>
				{#if now?.target_vpk !== null && now?.target_vpk !== undefined && now.drive_vpk !== null && Math.abs(now.target_vpk - now.drive_vpk) > 5e-4}<span class="sub num">to {vpk(now.target_vpk)}</span>{/if}
			</div>
			<div class="row" role="group" aria-label="Step the level">
				<button class="btn sm" data-testid="explorer-level-coarse-down" aria-label="Down {dbStep(steps.coarse_db)}" aria-disabled={!v.levelStep.enabled} class:off={!v.levelStep.enabled} onclick={() => go('levelStep', () => explorer.stepLevel(-steps.coarse_db))} {...tip('levelStep')}>-{dbStep(steps.coarse_db)}</button>
				<button class="btn sm" data-testid="explorer-level-fine-down" aria-label="Down {dbStep(steps.fine_db)}" aria-disabled={!v.levelStep.enabled} class:off={!v.levelStep.enabled} onclick={() => go('levelStep', () => explorer.stepLevel(-steps.fine_db))} {...tip('levelStep')}>-{dbStep(steps.fine_db)}</button>
				<button class="btn sm" data-testid="explorer-level-fine-up" aria-label="Up {dbStep(steps.fine_db)}" aria-disabled={!v.levelStep.enabled} class:off={!v.levelStep.enabled} onclick={() => go('levelStep', () => explorer.stepLevel(steps.fine_db))} {...tip('levelStep')}>+{dbStep(steps.fine_db)}</button>
				<button class="btn sm" data-testid="explorer-level-coarse-up" aria-label="Up {dbStep(steps.coarse_db)}" aria-disabled={!v.levelStep.enabled} class:off={!v.levelStep.enabled} onclick={() => go('levelStep', () => explorer.stepLevel(steps.coarse_db))} {...tip('levelStep')}>+{dbStep(steps.coarse_db)}</button>
			</div>
			<form class="row" onsubmit={(e) => (e.preventDefault(), go('levelVolts', setVolts))}>
				<input class="field num" aria-label="Level, volts peak" placeholder="V pk" inputmode="decimal" bind:value={voltsText} data-testid="explorer-volts" {...tip('levelVolts')} />
				<button class="btn sm" type="submit" data-testid="explorer-set-volts" aria-disabled={!v.levelVolts.enabled || !voltsOk} class:off={!v.levelVolts.enabled || !voltsOk} {...tip('levelVolts')}>Set</button>
				<button class="btn sm" type="button" data-testid="explorer-zero" aria-disabled={!v.levelVolts.enabled} class:off={!v.levelVolts.enabled} onclick={() => go('levelVolts', () => explorer.setVpk(0))} {...tip('levelVolts')}>To 0 V</button>
			</form>
		</div>

		<div class="part">
			<div class="head"><h3>Sweep</h3></div>
			<div class="row">
				<button class="btn sm" data-testid="explorer-sweep-down" aria-disabled={!v.sweep.enabled || !rateOk} class:off={!v.sweep.enabled || !rateOk} onclick={() => go('sweep', () => explorer.sweep('down', rate))} {...tip('sweep')}>
					<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M7.5 2.5L4 6l3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
					Down
				</button>
				<input class="field num rate" aria-label="Sweep rate, octaves a minute" placeholder={n(steps.sweep_oct_per_min, 3)} inputmode="decimal" bind:value={rateText} data-testid="explorer-sweep-rate" />
				<span class="unit">oct/min</span>
				<button class="btn sm" data-testid="explorer-sweep-up" aria-disabled={!v.sweep.enabled || !rateOk} class:off={!v.sweep.enabled || !rateOk} onclick={() => go('sweep', () => explorer.sweep('up', rate))} {...tip('sweep')}>
					Up
					<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M4.5 2.5L8 6l-3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
				</button>
				<button class="btn sm" data-testid="explorer-hold" aria-disabled={!v.hold.enabled} class:off={!v.hold.enabled} onclick={() => go('hold', () => explorer.hold())} {...tip('hold')}>
					<svg viewBox="0 0 14 14" width="11" height="11" aria-hidden="true"><rect x="3.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" /><rect x="8.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" /></svg>
					Hold
				</button>
			</div>
		</div>

		{#if explorer.answer}<p class="answer" aria-live="polite" data-testid="explorer-answer">{explorer.answer.words}</p>{/if}
		{#if hint}<p class="hint" aria-live="polite" data-testid="explorer-steer-hint">{hint}</p>{/if}
		<p class="keys aq-small aq-muted">With this panel focused: Left and Right step the frequency, Up and Down the level, by {octaveStep(fineOct(steps))} and {dbStep(steps.fine_db)}; with Shift, {octaveStep(coarseOct(steps))} and {dbStep(steps.coarse_db)}. H holds a sweep.</p>
	</section>
{/if}

<style>
	.steer {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		border-radius: var(--radius-md);
	}

	.steer:focus-visible {
		outline: 2px solid var(--blue-2);
		outline-offset: 4px;
	}

	.part {
		display: flex;
		flex-direction: column;
		gap: 6px;
	}

	.head {
		display: flex;
		align-items: baseline;
		gap: var(--sp-2);
	}

	h3 {
		min-width: 70px;
		margin: 0;
		font-size: var(--text-xs);
		font-weight: var(--weight-strong);
		color: var(--text-2);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	.big {
		font-size: 20px;
		font-weight: var(--weight-strong);
		color: var(--text-1);
	}

	.sub,
	.unit {
		font-size: var(--text-sm);
		color: var(--text-3);
	}

	.row {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 4px;
	}

	.row input {
		width: 84px;
	}

	.row input.rate {
		width: 64px;
	}

	.btn.off {
		opacity: 0.55;
		cursor: default;
	}

	.answer {
		margin: 0;
		font-size: var(--text-sm);
		color: var(--text-1);
	}

	.hint {
		margin: 0;
		font-size: var(--text-xs);
		color: var(--amber-1);
	}

	.keys {
		margin: 0;
	}
</style>
