<script lang="ts">
	/**
	 * Limits, protections and the loop's settings, then the run record. A limit channel is an input
	 * that is not a control, with a PSD ceiling at full level: the floor's reference is notched where
	 * it would pass it. The protections end the run on an envelope over its abort lines, a loop that
	 * diverges, or an input near overload. The loop's caps and ramps start at the service's usual
	 * values. The model of the structure is held from identification: an updated model is not built.
	 */
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import RunRecordEditor from '../RunRecordEditor.svelte';
	import { MIMO_MIXED_MAX_LIMITS } from '$lib/protocol/mimoMixed';
	import { limitRows } from './mmixSetup';
	import type { MimoMixedStore } from './mmixStore.svelte';

	interface Props {
		mm: MimoMixedStore;
	}
	let { mm }: Props = $props();

	const d = $derived(mm.setup);
	const locked = $derived(mm.locked);
	const E = $derived(mm.fieldErrors);
	const free = $derived(limitRows(d, mm.rows).filter((r) => !d.limits.some((l) => l.channel === r.index)));
	let pick = $state<number | null>(null);
	const errs = (k: string) => (E[k] ?? []).map((message) => ({ level: 'error' as const, message }));

	const CONTROL: { key: keyof typeof d.control; label: string; unit?: string; step?: number; integer?: boolean }[] = [
		{ key: 'part_ramp_s', label: 'Part ramp', unit: 's', step: 0.5 },
		{ key: 'floor_settle_s', label: 'Floor settle, at most', unit: 's', step: 5 },
		{ key: 'level_ramp_s', label: 'Level ramp', unit: 's', step: 0.5 },
		{ key: 'shutdown_ramp_s', label: 'Shutdown ramp', unit: 's', step: 0.5 },
		{ key: 'soft_start_db', label: 'Soft start below', unit: 'dB', step: 1 },
		{ key: 'soft_start_relax_db', label: 'Soft start relaxed by', unit: 'dB', step: 1 },
		{ key: 'crest_factor', label: "Floor's crest factor", step: 0.5 },
		{ key: 'cap_up_db', label: 'Step up, at most', unit: 'dB', step: 0.5 },
		{ key: 'cap_down_db', label: 'Step down, at most', unit: 'dB', step: 0.5 }
	];
</script>

<div class="stack">
	<section class="aq-panel" aria-labelledby="mmix-lim" data-testid="mmix-limits">
		<div class="aq-head"><h2 id="mmix-lim">Limit Channels</h2><span class="aq-sub">Inputs that are not controls, with a PSD ceiling at full level. Up to {MIMO_MIXED_MAX_LIMITS}.</span></div>
		{#if d.limits.length}
			<table class="aq-table">
				<thead><tr><th>Channel</th><th class="num">Ceiling</th><th class="num">Abort above it</th><th></th></tr></thead>
				<tbody>
					{#each d.limits as l, i (l.channel)}
						<tr data-testid="mmix-limit-{i}">
							<td>{mm.channelLabel(l.channel) || `Channel ${l.channel}`}</td>
							<td><NumberField compact id="mmix-lim-c-{i}" label="Limit {l.channel} ceiling" unit="g²/Hz" min={0} bind:value={l.ceiling} errors={E[`limits.${i}.ceiling`]} disabled={locked} /></td>
							<td><NumberField compact id="mmix-lim-a-{i}" label="Limit {l.channel} abort margin" unit="dB" min={0} bind:value={l.abort_margin_db} errors={E[`limits.${i}.abort_margin_db`]} disabled={locked} /></td>
							<td><button class="btn ghost sm" disabled={locked} onclick={() => mm.removeLimit(l.channel)}>Remove</button></td>
						</tr>
					{/each}
				</tbody>
			</table>
		{:else}
			<p class="aq-small aq-muted">No limit channels.</p>
		{/if}
		<div class="aq-actions">
			<select class="field sm" bind:value={pick} disabled={locked || !free.length} aria-label="Channel to add as a limit" data-testid="mmix-limit-pick">
				<option value={null}>{free.length ? 'Choose a channel' : 'No free inputs'}</option>
				{#each free as r (r.index)}<option value={r.index}>Channel {r.index}{r.description ? `, ${r.description}` : ''}</option>{/each}
			</select>
			<button class="btn sm" disabled={locked || pick === null || d.limits.length >= MIMO_MIXED_MAX_LIMITS} onclick={() => pick !== null && mm.addLimit(pick) && (pick = null)} data-testid="mmix-limit-add">Add Limit</button>
		</div>
		<Findings items={errs('limits')} />
	</section>

	<section class="aq-panel" aria-labelledby="mmix-prot" data-testid="mmix-protections">
		<div class="aq-head"><h2 id="mmix-prot">Protections</h2><span class="aq-sub">Each ends the run with a ramp down, named in the log.</span></div>
		<div class="aq-fields">
			<NumberField id="mmix-p-env" label="Lines over abort before a stop" integer min={1} bind:value={d.protections.envelope_abort_lines} errors={E['protections.envelope_abort_lines']} disabled={locked} testid="mmix-p-env" />
			<NumberField id="mmix-p-div" label="Diverging loops before a stop" integer min={1} bind:value={d.protections.divergence_loops} errors={E['protections.divergence_loops']} disabled={locked} testid="mmix-p-div" />
			<NumberField id="mmix-p-divdb" label="Divergence counted from" unit="dB" min={0} step={0.5} bind:value={d.protections.divergence_floor_db} errors={E['protections.divergence_floor_db']} disabled={locked} testid="mmix-p-divdb" help="A loop diverges when its error grows past this many dB." />
			<NumberField id="mmix-p-settle" label="Loops to settle before the checks" integer min={0} bind:value={d.protections.settle_loops} errors={E['protections.settle_loops']} disabled={locked} testid="mmix-p-settle" />
			<NumberField id="mmix-p-ovl" label="Input overload at" min={0.1} max={1} step={0.05} bind:value={d.protections.overload_fraction} errors={E['protections.overload_fraction']} disabled={locked} testid="mmix-p-ovl" help="As a share of each input's range." />
		</div>
	</section>

	<section class="aq-panel" aria-labelledby="mmix-loop" data-testid="mmix-loop">
		<div class="aq-head"><h2 id="mmix-loop">The Loop</h2><span class="aq-sub">Every field starts at the service's usual value. The model is held from identification.</span></div>
		<div class="aq-fields">
			{#each CONTROL as c (c.key)}
				<NumberField id="mmix-c-{c.key}" label={c.label} unit={c.unit} step={c.step} integer={c.integer} bind:value={d.control[c.key]} errors={E[`control.${c.key}`]} disabled={locked} testid="mmix-c-{c.key}" />
			{/each}
		</div>
		<Findings items={errs('control.frf')} />
	</section>

	<RunRecordEditor acq={mm} kept="The setup, the channel table, the log, the identification, the spectra, every part's levels and the identification are still kept." />
</div>

<style>
	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
	}
</style>
