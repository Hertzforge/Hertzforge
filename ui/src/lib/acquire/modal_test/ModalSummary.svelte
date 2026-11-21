<script lang="ts">
	/**
	 * The Modal Test setup in a few lines, beside every section: the mode, the DOFs, the plan and the
	 * excitation; the control service's check of it before anything is armed (the rate, the band,
	 * the estimator, the references and responses, the points); every warning and refusal in the
	 * service's own words; and what is left to fix, each a click from its section.
	 */
	import { modalRows, modalSections, type ModalSection } from './modalTestSetup';
	import type { ModalTestStore } from './modalTestStore.svelte';
	import { num, SOURCE_WORDS } from './modalTestWords';

	interface Props {
		modal: ModalTestStore;
		onopen: (s: ModalSection) => void;
	}
	let { modal, onopen }: Props = $props();

	const d = $derived(modal.setup);
	const dry = $derived(modal.current);
	const c = $derived(modal.checkOf);
	const ch = $derived(modalRows(modal.rows));
	const sections = $derived(modalSections(d.mode));
	const todo = $derived(sections.filter((s) => modal.counts[s.id].errors > 0));
	const dofName = (r: { node?: number | null; direction?: string | null }) => `${r.node ?? '-'}${r.direction ?? ''}`;
</script>

<aside class="aq-panel summary" aria-label="Setup summary" data-testid="mt-summary">
	<h2>Summary</h2>
	<dl>
		<div><dt>Mode</dt><dd data-testid="mt-summary-mode">{d.mode === 'impact' ? `Impact, ${d.impact.accept === 'auto' ? 'automatic' : 'manual'} accept` : `Shaker, ${SOURCE_WORDS[d.shaker.source]}, ${d.shaker.drives.length} ${d.shaker.drives.length === 1 ? 'shaker' : 'shakers'}`}</dd></div>
		<div><dt>DOFs</dt><dd data-testid="mt-summary-dofs">{ch.references.length ? `References ${ch.references.map(dofName).join(', ')}` : 'No reference'}; {ch.responses.length ? `responses ${ch.responses.map(dofName).join(', ')}` : 'no response'}</dd></div>
		<div><dt>Averages</dt><dd class="num">{d.averages} {d.mode === 'impact' ? 'hits' : 'frames'} a point</dd></div>
		<div><dt>Roving</dt><dd>{d.roving.on ? `Rows ${d.roving.channels.join(', ') || '-'} rove` : 'Off: one point'}</dd></div>
	</dl>

	{#if c}
		<h3>From the Control Service</h3>
		<dl data-testid="mt-summary-check">
			<div><dt>Acquisition</dt><dd class="num">{num(c.fs_hz, 'Hz')}, {c.block} samples, {num(c.df_hz, 'Hz')} lines</dd></div>
			<div><dt>Band</dt><dd class="num">{num(c.band.lo_hz, 'Hz')} to {num(c.band.hi_hz, 'Hz')}, {c.band.lines} lines</dd></div>
			<div><dt>Estimator</dt><dd>{c.estimator}</dd></div>
			<div><dt>Plan</dt><dd class="num" data-testid="mt-summary-points">{c.points.length} {c.points.length === 1 ? 'point' : 'points'}, {c.references.length} {c.references.length === 1 ? 'reference' : 'references'}, {c.responses.length} {c.responses.length === 1 ? 'response' : 'responses'}</dd></div>
			{#if c.impact}
				<div><dt>Trigger</dt><dd class="num">{num(c.impact.trigger.level, c.impact.trigger.unit)}, {c.impact.trigger.pretrigger_frames} samples before</dd></div>
				<div><dt>Checks</dt><dd>Overload {c.impact.checks.overload}; double hit {c.impact.checks.double_hit}; tip over {num(c.impact.checks.tip_warn_db, 'dB')}{c.impact.checks.target ? `; target ${num(c.impact.checks.target.peak, c.impact.checks.target.unit)} within ${num(c.impact.checks.target.tolerance_pct)}%, never rejects` : ''}</dd></div>
				{#if c.impact.exponential_window}<div><dt>Exponential window</dt><dd class="num">tau {num(c.impact.exponential_window.tau_s, 's')}</dd></div>{/if}
			{/if}
			{#if c.shaker}
				<div><dt>Drives</dt><dd class="num">{c.shaker.drives.map((x) => `row ${x.row} ${num(x.level_vrms, 'Vrms')}`).join(', ')}; ceiling {num(c.shaker.ceiling_v, 'V')}</dd></div>
			{/if}
		</dl>
	{:else if modal.checking}
		<p class="aq-muted aq-small">Checking the setup.</p>
	{:else if modal.acq.tableDirty}
		<p class="aq-muted aq-small" data-testid="mt-summary-table">The table has edits: the service checks the DOFs once it is applied. Apply Setup sends both.</p>
	{/if}

	{#if dry && (dry.errors.length || dry.warnings.length)}
		<h3>The Service Says</h3>
		<ul class="said" data-testid="mt-said">
			{#each dry.errors as e, k (k)}<li class="error" data-reason={e.reason}><strong>Refused.</strong> {e.message}</li>{/each}
			{#each dry.warnings as w, k (k)}<li class="warning" data-reason={w.reason}>{w.message}</li>{/each}
		</ul>
	{/if}

	<div class="ready" data-testid="mt-ready" data-ready={todo.length === 0 && !!dry?.check}>
		{#if todo.length}
			<h3>Left to Fix</h3>
			<ul>
				{#each todo as s (s.id)}
					<li><button class="btn ghost sm" onclick={() => onopen(s.id)} data-testid="mt-todo-{s.id}">{s.label}: {modal.counts[s.id].errors} {modal.counts[s.id].errors === 1 ? 'error' : 'errors'}</button></li>
				{/each}
			</ul>
		{:else if dry?.check}
			<p class="aq-small ok">Nothing is missing: the setup checks clean.</p>
		{/if}
	</div>
</aside>

<style>
	.summary {
		position: sticky;
		top: 0;
	}

	h2 {
		margin-bottom: var(--sp-2);
		font-size: var(--text-md);
		font-weight: var(--weight-strong);
	}

	h3 {
		margin: var(--sp-3) 0 var(--sp-1);
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
	}

	dl {
		display: flex;
		flex-direction: column;
		gap: 6px;
		margin: 0;
	}

	dt {
		margin-top: var(--sp-2);
		font-size: var(--text-2xs);
		color: var(--text-3);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	dd {
		margin: 0;
		font-size: var(--text-sm);
		color: var(--text-1);
	}

	ul {
		margin: 0;
		padding: 0;
		list-style: none;
	}

	.said li {
		font-size: var(--text-xs);
		margin-top: var(--sp-1);
		padding-left: 8px;
		border-left: 3px solid var(--status-warn);
		color: var(--text-2);
	}

	.said li.error {
		border-left-color: var(--status-fail);
		color: var(--text-1);
	}

	.ok {
		margin: var(--sp-3) 0 0;
		color: var(--blue-1);
	}
</style>
