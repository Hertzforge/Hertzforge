<script lang="ts">
	/**
	 * When each control's pulse arrives, and what makes a pulse pass. Timing is free by default:
	 * each control is judged after its own best-fit delay is taken out, and the delays are reported.
	 * Held: each control's arrival must fall within the window of the delay set for it. The verdict
	 * is every control inside its own tolerance, or, for an SRS target, the energy-weighted global
	 * SRS error over every control.
	 */
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import type { MimoShockStore } from './msStore.svelte';
	import { VERDICT_WORDS } from './msWords';

	interface Props {
		ms: MimoShockStore;
	}
	let { ms }: Props = $props();

	const d = $derived(ms.setup);
	const locked = $derived(ms.locked);
	const E = $derived(ms.fieldErrors);
	const errs = (k: string) => E[k] ?? [];
</script>

<div class="stack">
	<section class="aq-panel" aria-labelledby="ms-tm-title" data-testid="ms-timing">
		<div class="aq-head"><h2 id="ms-tm-title">Timing Between Controls</h2><span class="aq-sub">A MIMO shock test holds each point's own pulse or SRS; the time between points is free unless held here.</span></div>
		<div class="aq-fields">
			<div class="sel wide">
				<span class="lbl" id="ms-tmode-l">Timing</span>
				<div class="aq-seg" role="radiogroup" aria-labelledby="ms-tmode-l">
					<button type="button" role="radio" aria-checked={d.timing.mode === 'free'} disabled={locked} onclick={() => (d.timing.mode = 'free')} data-testid="ms-timing-free">Free</button>
					<button type="button" role="radio" aria-checked={d.timing.mode === 'hold'} disabled={locked} onclick={() => (d.timing.mode = 'hold')} data-testid="ms-timing-hold">Held</button>
				</div>
				<span class="aq-small aq-muted">
					{d.timing.mode === 'free'
						? "Each control is judged after its own delay is taken out, searched within the window; the delays are reported, and a spread past the window is a warning."
						: "Each control's pulse must arrive within the window of the delay set for it, or it fails on timing."}
				</span>
			</div>
			<NumberField id="ms-window" label="Window" unit="ms" step={0.5} min={0} max={100} bind:value={d.timing.window_ms} errors={errs('timing.window_ms')} disabled={locked} testid="ms-window" help="Usual: 2 ms." />
		</div>
		{#if d.timing.mode === 'hold'}
			<div class="aq-group">
				<h3>Delay Held at Each Control</h3>
				<div class="aq-fields">
					{#each d.controls as c, i (c)}
						<NumberField id="ms-delay-{c}" label="Control {c}" unit="ms" step={0.1} min={0} max={100} bind:value={d.timing.delays_ms[i]} errors={errs(`timing.delays_ms.${i}`)} disabled={locked} testid="ms-delay-{c}" />
					{/each}
				</div>
				<Findings items={errs('timing.delays_ms').map((message) => ({ level: 'error' as const, message }))} />
			</div>
		{/if}
	</section>

	<section class="aq-panel" aria-labelledby="ms-v-title" data-testid="ms-verdict">
		<div class="aq-head"><h2 id="ms-v-title">Verdict</h2><span class="aq-sub">What makes a pulse pass; the ladder climbs only on a passing pulse.</span></div>
		<div class="aq-seg" role="radiogroup" aria-label="Verdict">
			{#each ['all', 'global'] as const as v (v)}
				<button type="button" role="radio" aria-checked={d.verdict === v} disabled={locked || (v === 'global' && d.kind !== 'srs')} onclick={() => (d.verdict = v)} data-testid="ms-verdict-{v}">{VERDICT_WORDS[v].label}</button>
			{/each}
		</div>
		<p class="aq-small aq-muted">{VERDICT_WORDS[d.verdict].help}{d.kind !== 'srs' ? ' The global error is for an SRS target only.' : ' The global error is shown with every pulse either way.'}</p>
		<Findings items={errs('verdict').map((message) => ({ level: 'error' as const, message }))} />
	</section>
</div>

<style>
	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
	}

	.sel {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.sel.wide {
		grid-column: 1 / -1;
	}

	.lbl {
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}
</style>
