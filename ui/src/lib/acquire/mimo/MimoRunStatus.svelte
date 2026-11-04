<script lang="ts">
	/**
	 * The run in numbers, from the service's status: where it is in its sequence (noise floor,
	 * identification, prediction, the ladder, full level), each control's rms error against the
	 * reference, each drive's rms against its ceiling with the cap the core applied this loop,
	 * and anything that went wrong. The sequence is drawn with shapes and words, never by color.
	 */
	import { formatFixed } from '$lib/viz/format';
	import Findings from '../Findings.svelte';
	import { db, OUTCOME_WORDS } from '../words';
	import type { MimoState } from './mimoStore.svelte';

	interface Props {
		mimo: MimoState;
	}
	let { mimo }: Props = $props();

	const s = $derived(mimo.status);
	const STEPS = [
		{ id: 'noise', label: 'Noise floor', phases: ['sysid_noise'] },
		{ id: 'sysid', label: 'Identification', phases: ['sysid'] },
		{ id: 'review', label: 'Prediction', phases: ['awaiting_review', 'gate_failed'] },
		{ id: 'ladder', label: 'Level schedule', phases: ['engaged'] },
		{ id: 'full', label: 'Full level', phases: ['full_level', 'ramping_down'] }
	];
	const at = $derived(STEPS.findIndex((x) => s && x.phases.includes(s.phase)));
	const ended = $derived(s?.state === 'finished');
	const ceilings = $derived(s?.prediction?.ceiling_vrms ?? []);
</script>

<div class="run" data-testid="mimo-run-status" data-phase={s?.phase ?? ''}>
	<section class="aq-panel">
		<div class="aq-head">
			<h2>Sequence</h2>
			<span class="aq-spacer"></span>
			{#if s}<span class="aq-small aq-muted num">Loop {s.loop}{s.level_db !== null ? `, level ${db(s.level_db)}` : ''}</span>{/if}
		</div>
		<ol class="seq" aria-label="Run sequence">
			{#each STEPS as x, i (x.id)}
				{@const done = ended ? i <= Math.max(at, 0) : at > i}
				{@const now = !ended && at === i}
				<li class:done class:now aria-current={now ? 'step' : undefined}>
					<span class="node" aria-hidden="true">
						{#if done}
							<svg viewBox="0 0 16 16" width="16" height="16"><circle cx="8" cy="8" r="7" fill="currentColor" /><path d="M4.8 8.2l2.1 2.1 4.3-4.6" fill="none" stroke="var(--bg-1)" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" /></svg>
						{:else if now}
							<svg viewBox="0 0 16 16" width="16" height="16"><circle cx="8" cy="8" r="6.5" fill="none" stroke="currentColor" stroke-width="2" /><circle cx="8" cy="8" r="3" fill="currentColor" /></svg>
						{:else}
							<svg viewBox="0 0 16 16" width="16" height="16"><circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" stroke-width="1.3" stroke-dasharray="2.2 2" /></svg>
						{/if}
					</span>
					<span class="lab">{x.label}</span>
				</li>
			{/each}
		</ol>
		{#if !s || s.state === 'configured' || s.state === 'unconfigured'}
			<p class="aq-muted aq-small">Start from the run bar: the noise floor is measured with the drives off, then identification runs at {mimo.setup.identification.level_db} dB, and the service holds for the prediction.</p>
		{/if}
		{#if ended && s?.end}
			<p class="end aq-small" data-testid="mimo-run-end">{OUTCOME_WORDS[s.end.outcome] ?? s.end.outcome}{s.end.message ? `: ${s.end.message}` : ''}</p>
		{/if}
		{#if s?.override}<p class="aq-small aq-muted">Overridden by {s.override.by}: "{s.override.text}"</p>{/if}
		<Findings items={(s?.faults ?? []).map((f) => ({ level: 'error' as const, message: f.message }))} />
	</section>

	<div class="two">
		<section class="aq-panel" aria-labelledby="rs-c">
			<div class="aq-head"><h2 id="rs-c">Controls</h2><span class="aq-sub">RMS error over the band against the reference</span></div>
			{#if s?.control_error_db.length}
				<table class="aq-table">
					<thead><tr><th>{s.shape?.space === 'virtual' ? 'Degree of freedom' : 'Control'}</th><th class="num">Error</th></tr></thead>
					<tbody>
						{#each s.control_error_db as e, i (i)}
							<tr data-testid="run-control-{i}"><td>{s.shape?.labels[i] ?? i + 1}</td><td class="num">{db(e, 2)}</td></tr>
						{/each}
					</tbody>
				</table>
			{:else}
				<p class="aq-muted aq-small">Shown from the first control loop.</p>
			{/if}
		</section>
		<section class="aq-panel" aria-labelledby="rs-d">
			<div class="aq-head"><h2 id="rs-d">Drives</h2><span class="aq-sub">RMS and the core's cap this loop</span></div>
			{#if s?.drive_rms_v.length}
				<table class="aq-table">
					<thead><tr><th>Drive</th><th class="num">RMS</th><th class="num">Ceiling</th><th class="num">Cap</th></tr></thead>
					<tbody>
						{#each s.drive_rms_v as v, k (k)}
							<tr data-testid="run-drive-{k}">
								<td>{s.shape?.drive_labels[k] ?? k + 1}</td>
								<td class="num">{formatFixed(v, 3)} V</td>
								<td class="num">{ceilings[k] !== undefined ? `${formatFixed(ceilings[k], 2)} V` : '-'}</td>
								<td class="num">{s.caps[k] === undefined ? '-' : s.caps[k] >= 0.9995 ? 'None' : `${formatFixed(s.caps[k], 3)}`}</td>
							</tr>
						{/each}
					</tbody>
				</table>
			{:else}
				<p class="aq-muted aq-small">Shown once the drives play.</p>
			{/if}
		</section>
	</div>
</div>

<style>
	.run {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}

	.seq {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-2) var(--sp-5);
		margin: 0 0 var(--sp-2);
		padding: 0;
		list-style: none;
	}

	.seq li {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		font-size: var(--text-sm);
		color: var(--text-3);
	}

	.seq li.done {
		color: var(--text-2);
	}

	.seq li.now {
		font-weight: var(--weight-strong);
		color: var(--text-1);
	}

	.node {
		display: inline-flex;
		color: currentColor;
	}

	.seq li.now .node {
		color: var(--blue-2);
	}

	.end {
		font-weight: var(--weight-strong);
	}

	.two {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: var(--sp-3);
		align-items: start;
	}

	@media (max-width: 1000px) {
		.two {
			grid-template-columns: minmax(0, 1fr);
		}
	}
</style>
