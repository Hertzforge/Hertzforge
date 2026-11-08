<script lang="ts">
	/**
	 * The pulse: its shape, peak, duration and direction, and the shoulders that bring the table to
	 * rest. Beside it the service draws the pulse (`shock.preview`): the reference with its
	 * tolerance band, its velocity and displacement, its kinematic facts, and whether the shoulders
	 * stay inside the standard's quiet window. The shell computes none of it.
	 */
	import ViewRenderer from '$lib/viz/ViewRenderer.svelte';
	import { formatNumber } from '$lib/viz/format';
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import { COMPENSATIONS, SHAPES } from './shockSetup';
	import type { ShockStore } from './shockStore.svelte';

	interface Props {
		shock: ShockStore;
	}
	let { shock }: Props = $props();

	const d = $derived(shock.setup);
	const locked = $derived(shock.locked);
	const E = $derived(shock.fieldErrors);
	const W = $derived(shock.fieldWarnings);
	const errs = (k: string) => E[k] ?? [];
	const warns = (k: string) => W[k] ?? [];
	const preview = $derived(shock.pulsePreview);
	const facts = $derived(preview?.derived ?? null);
	const feas = $derived(shock.check && shock.checkedKey === shock.key ? (shock.check.derived?.feasibility ?? null) : null);
</script>

<div class="cols">
	<div class="stack">
		<section class="aq-panel" aria-labelledby="sp-title">
			<div class="aq-head"><h2 id="sp-title">Pulse</h2><span class="aq-sub">The shape, peak and duration the control point should see.</span></div>
			<div class="aq-fields">
				<div class="sel wide">
					<label for="sp-name" class="lbl">Test title</label>
					<input id="sp-name" class="field" bind:value={d.title} disabled={locked} aria-invalid={errs('title').length > 0} data-testid="shock-title" />
					<Findings items={errs('title').map((message) => ({ level: 'error' as const, message }))} />
				</div>
				<div class="sel">
					<label for="sp-shape" class="lbl">Shape</label>
					<select id="sp-shape" class="field" bind:value={d.pulse.shape} disabled={locked} data-testid="shock-shape">
						{#each SHAPES as s (s.id)}<option value={s.id}>{s.label}</option>{/each}
					</select>
				</div>
				<NumberField id="sp-amp" label="Peak" unit="g" step={1} min={0} bind:value={d.pulse.amplitude_g} errors={errs('pulse.amplitude_g')} disabled={locked} testid="shock-amplitude" help="The main pulse's peak acceleration at full level." />
				<NumberField id="sp-dur" label="Duration" unit="ms" step={0.5} min={0} bind:value={d.pulse.duration_ms} errors={errs('pulse.duration_ms')} disabled={locked} testid="shock-duration" help="The main pulse's duration." />
				<div class="sel">
					<span class="lbl" id="sp-pol-l">Direction</span>
					<div class="aq-seg" role="radiogroup" aria-labelledby="sp-pol-l">
						<button type="button" role="radio" aria-checked={d.pulse.polarity === 'positive'} disabled={locked} onclick={() => (d.pulse.polarity = 'positive')} data-testid="shock-polarity-positive">Positive</button>
						<button type="button" role="radio" aria-checked={d.pulse.polarity === 'negative'} disabled={locked} onclick={() => (d.pulse.polarity = 'negative')} data-testid="shock-polarity-negative">Negative</button>
					</div>
					<p class="help">The standards ask for both directions. Negative turns the reference over and judges the pulse the same way.</p>
				</div>
				{#if d.pulse.shape === 'trapezoid'}
					<NumberField id="sp-rise" label="Rise, share of the duration" step={0.05} min={0} max={1} bind:value={d.pulse.rise_frac} errors={errs('pulse.rise_frac')} disabled={locked} testid="shock-rise" />
					<NumberField id="sp-fall" label="Fall, share of the duration" step={0.05} min={0} max={1} bind:value={d.pulse.fall_frac} errors={errs('pulse.fall_frac')} disabled={locked} testid="shock-fall" />
				{/if}
			</div>
		</section>

		<section class="aq-panel" aria-labelledby="sp-comp">
			<div class="aq-head"><h2 id="sp-comp">Compensation</h2><span class="aq-sub">Shoulders before and after the pulse bring the table back to rest.</span></div>
			<p class="aq-lead aq-small">A shaker cannot end a pulse moving: the shoulders take its velocity and displacement back to zero. Longer, lower shoulders stay inside the standard's quiet window; rectangular shoulders four times the duration usually do.</p>
			<div class="aq-fields">
				<div class="sel wide">
					<span class="lbl" id="sp-cs-l">Shoulder shape</span>
					<div class="aq-seg" role="radiogroup" aria-labelledby="sp-cs-l">
						{#each COMPENSATIONS as c (c.id)}
							<button type="button" role="radio" aria-checked={d.pulse.compensation.shape === c.id} disabled={locked} onclick={() => (d.pulse.compensation.shape = c.id)} data-testid="shock-comp-{c.id}">{c.label}</button>
						{/each}
					</div>
				</div>
				<NumberField id="sp-pre" label={d.pulse.compensation.symmetric ? 'Each shoulder, times the duration' : 'Shoulder before, times the duration'} step={0.5} min={0} bind:value={d.pulse.compensation.pre_frac} errors={errs('pulse.compensation.pre_frac')} disabled={locked} testid="shock-pre" />
				{#if !d.pulse.compensation.symmetric}
					<NumberField id="sp-post" label="Shoulder after, times the duration" step={0.5} min={0} bind:value={d.pulse.compensation.post_frac} errors={errs('pulse.compensation.post_frac')} disabled={locked} testid="shock-post" />
				{/if}
				<div class="sel wide">
					<label class="aq-switch" for="sp-sym">
						<input id="sp-sym" type="checkbox" role="switch" checked={d.pulse.compensation.symmetric} disabled={locked} onchange={(e) => (d.pulse.compensation.symmetric = e.currentTarget.checked)} data-testid="shock-symmetric" />
						<span class="track" aria-hidden="true"><span class="thumb"></span></span>
						<span>The same shoulder before and after</span>
						<span class="word">{d.pulse.compensation.symmetric ? 'On' : 'Off'}</span>
					</label>
				</div>
			</div>
			<Findings items={warns('pulse.compensation').map((message) => ({ level: 'warning' as const, message }))} testid="shock-quiet-warning" />
		</section>
	</div>

	<section class="aq-panel preview" aria-labelledby="sp-prev" data-testid="shock-preview" data-state={shock.previewBusy ? 'busy' : preview ? 'ok' : 'none'}>
		<div class="aq-head"><h2 id="sp-prev">The Pulse at Full Level</h2><span class="aq-sub">Drawn by the control service from the pulse as set.</span></div>
		{#if facts}
			<dl class="facts" data-testid="shock-preview-facts">
				<div><dt>Peak</dt><dd class="num">{formatNumber(facts.reference.peak_g, 4)} g</dd></div>
				<div><dt>Velocity change</dt><dd class="num">{formatNumber(facts.reference.velocity_change_ips, 4)} in/s</dd></div>
				<div><dt>Displacement, peak to peak</dt><dd class="num">{formatNumber(facts.reference.displacement_pk_pk_in, 4)} in</dd></div>
				<div class:warn={!facts.quiet_window.inside}>
					<dt>Quiet window</dt>
					<dd class="num" data-testid="shock-quiet" data-inside={facts.quiet_window.inside}>
						{#if !facts.quiet_window.inside}<svg viewBox="0 0 14 14" width="11" height="11" aria-hidden="true"><path d="M7 1.6l5.6 10H1.4z" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round" /></svg>{/if}
						{formatNumber(facts.quiet_window.worst_frac * 100, 3)}% of the peak, {facts.quiet_window.inside ? 'within' : 'over'} {formatNumber(facts.quiet_window.limit_frac * 100, 3)}%
					</dd>
				</div>
			</dl>
			{#if !facts.quiet_window.inside}
				<p class="aq-notice warn aq-small" role="alert" data-testid="shock-quiet-note">The shoulders leave the standard's quiet window. The test may still run; longer, lower shoulders bring the pulse inside it.</p>
			{/if}
			{#if feas}
				<p class="aq-small kin" data-testid="shock-kinematics">Against the shaker: {feas.message}</p>
			{/if}
			<div class="plots">
				{#each preview?.views ?? [] as v, i (i)}
					<div class="plot" data-testid="shock-preview-view-{i}"><ViewRenderer view={v} minHeight={i === 0 ? 220 : 150} /></div>
				{/each}
			</div>
		{:else if shock.previewBusy}
			<p class="aq-muted aq-small">Drawing the pulse.</p>
		{:else}
			<p class="aq-muted aq-small">{shock.connected ? 'Give the peak, the duration and the shoulders to draw the pulse.' : 'The pulse is drawn by the control service once it is connected.'}</p>
		{/if}
	</section>
</div>

<style>
	.cols {
		display: grid;
		grid-template-columns: minmax(0, 1fr) minmax(0, 1.2fr);
		gap: var(--sp-3);
		align-items: start;
	}

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

	.wide {
		grid-column: 1 / -1;
	}

	.lbl {
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}

	.help {
		margin: 2px 0 0;
		font-size: var(--text-xs);
		color: var(--text-3);
	}

	.facts {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: var(--sp-2) var(--sp-3);
		margin: 0 0 var(--sp-2);
	}

	.facts dt {
		font-size: var(--text-2xs);
		color: var(--text-3);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	.facts dd {
		margin: 0;
		font-size: var(--text-sm);
	}

	.facts .warn dd {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		font-weight: var(--weight-strong);
		color: var(--amber-1);
	}

	.kin {
		margin: 0 0 var(--sp-2);
		color: var(--text-2);
	}

	.plots {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}

	.plot {
		min-width: 0;
	}

	@media (max-width: 1240px) {
		.cols {
			grid-template-columns: minmax(0, 1fr);
		}
	}
</style>
