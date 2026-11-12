<script lang="ts">
	/**
	 * The abort lines and the kinematic limits. Each control has two abort lines from one margin
	 * over its target's peak: the absolute line, at full level, always ends the run (it cannot be
	 * turned off); the scaled line, at the level played, ends the run while scaled aborts are on.
	 * With them off, a pulse over its scaled line is flagged and judged failing, and the run goes
	 * on without climbing. The switch is locked while a run is live. The drives' ceilings and
	 * aborts and the limit channels are with the channels.
	 */
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import type { MimoShockStore } from './msStore.svelte';

	interface Props {
		ms: MimoShockStore;
	}
	let { ms }: Props = $props();

	const d = $derived(ms.setup);
	const c = $derived(d.control);
	const locked = $derived(ms.locked);
	const E = $derived(ms.fieldErrors);
	const W = $derived(ms.fieldWarnings);
	const errs = (k: string) => E[k] ?? [];
	const usualAbort = $derived(d.kind === 'srs' ? '6 dB' : "twice the standard's amplitude tolerance");
</script>

<div class="stack">
	<section class="aq-panel" aria-labelledby="ms-ab-title" data-testid="ms-aborts">
		<div class="aq-head"><h2 id="ms-ab-title">Abort Lines</h2><span class="aq-sub">One margin over each control's target peak gives two lines.</span></div>
		<div class="aq-fields">
			<NumberField id="ms-abort-db" label="Abort margin over the target's peak" unit="dB" nullable="Usual" fallback={d.kind === 'srs' ? 6 : 2.28} step={0.5} min={0} max={40} bind:value={c.abort_db} errors={errs('control.abort_db')} disabled={locked} testid="ms-abort-db" help="Usual: {usualAbort}." />
		</div>

		<div class="lines">
			<div class="line" data-testid="ms-absolute-line">
				<span class="mk" aria-hidden="true"><svg viewBox="0 0 14 14" width="14" height="14"><rect x="3" y="6.5" width="8" height="6" rx="1" fill="currentColor" /><path d="M4.8 6.5V4.8a2.2 2.2 0 014.4 0v1.7" fill="none" stroke="currentColor" stroke-width="1.4" /></svg></span>
				<div>
					<p class="head"><strong>Absolute lines: always on</strong></p>
					<p class="aq-small">The margin over each control's target at full level. A measured peak over it ends the run at any level, whatever the setting below.</p>
				</div>
			</div>
			<div class="line" data-testid="ms-scaled-line">
				<label class="aq-switch">
					<input type="checkbox" role="switch" aria-checked={c.scaled_aborts} bind:checked={c.scaled_aborts} disabled={locked} data-testid="ms-scaled-aborts" aria-label="Scaled abort lines" />
					<span class="track" aria-hidden="true"><span class="thumb"></span></span>
					<span class="word">{c.scaled_aborts ? 'Scaled lines: on' : 'Scaled lines: off'}</span>
				</label>
				<div>
					<p class="aq-small" data-testid="ms-scaled-words">
						{#if c.scaled_aborts}
							On: a pulse over its abort line scaled to the level played stops the run.
						{:else}
							Off: a pulse over its abort line scaled to the level played is flagged and judged failing, and the run goes on without climbing.
						{/if}
					</p>
					{#if locked}<p class="aq-small aq-muted" data-testid="ms-scaled-locked">Locked while the run is live.</p>{/if}
				</div>
			</div>
		</div>
		<Findings items={(W['control.scaled_aborts'] ?? []).map((message) => ({ level: 'warning' as const, message }))} />
	</section>

	<section class="aq-panel" aria-labelledby="ms-kin-title">
		<div class="aq-head"><h2 id="ms-kin-title">Kinematic Limits</h2><span class="aq-sub">At every control, predicted or measured.</span></div>
		<div class="aq-fields">
			<NumberField id="ms-vel" label="Peak velocity" unit="in/s" nullable="None" fallback={100} step={10} min={0} bind:value={c.max_velocity_ips} errors={errs('control.max_velocity_ips')} disabled={locked} help="Over it, the run ends." />
			<NumberField id="ms-disp" label="Displacement, peak to peak" unit="in" nullable="None" fallback={1} step={0.1} min={0} bind:value={c.max_displacement_in} errors={errs('control.max_displacement_in')} disabled={locked} help="Over it, the run ends." />
		</div>
	</section>
</div>

<style>
	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
	}

	.lines {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		margin-top: var(--sp-3);
	}

	.line {
		display: grid;
		grid-template-columns: auto minmax(0, 1fr);
		gap: var(--sp-3);
		align-items: start;
	}

	.line p {
		margin: 0;
	}

	.mk {
		color: var(--text-2);
		padding-top: 2px;
	}

	.head {
		font-size: var(--text-sm);
	}
</style>
