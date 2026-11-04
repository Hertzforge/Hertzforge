<script lang="ts">
	/**
	 * The setup at a glance, beside every section: each section's state in a line, what must
	 * happen before a run can start (each item one click from where it is fixed), the test's
	 * shape as the service checked it, and where the setup stands on the service.
	 */
	import { STATE_WORDS } from '../words';
	import type { MimoSection } from './mimoSetup';
	import type { MimoState } from './mimoStore.svelte';
	import { missingList, summarize } from './summary';

	interface Props {
		mimo: MimoState;
		onopen: (s: MimoSection) => void;
	}
	let { mimo, onopen }: Props = $props();

	const sections = $derived(
		summarize({
			draft: mimo.setup,
			channels: mimo.channels,
			space: mimo.space,
			labels: mimo.labels,
			ref: mimo.ref && mimo.refKey === mimo.refInputKey ? mimo.ref : null,
			refBusy: mimo.refBusy,
			law: mimo.law,
			lawsLoaded: mimo.laws !== null,
			check: mimo.check && mimo.checkedKey === mimo.key ? mimo.check : null,
			counts: mimo.counts,
			station: mimo.acq.selected ? { kind: mimo.acq.selected.kind, dirty: mimo.acq.stationDirty } : null,
			tableErrors: mimo.acq.tableErrorCount,
			setupStatus: mimo.setupStatus
		})
	);
	const missing = $derived(missingList(sections));
	const check = $derived(mimo.check && mimo.checkedKey === mimo.key ? mimo.check : null);
	const status = $derived(mimo.setupStatus);
	const run = $derived(mimo.status);
	const SHAPE: Record<string, string> = { square: 'Square', more_controls_than_drives: 'More controls than drives', fewer_controls_than_drives: 'Fewer controls than drives' };
</script>

<aside class="aq-panel summary" aria-labelledby="ms-sum-title" data-testid="mimo-summary">
	<div class="aq-head">
		<h2 id="ms-sum-title">Setup</h2>
		<span class="aq-spacer"></span>
		<span class="check aq-small" aria-live="polite" data-testid="mimo-ready" data-ready={missing.length === 0}>
			{#if mimo.checking}
				Checking
			{:else if missing.length === 0}
				<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><circle cx="7" cy="7" r="6" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M4.2 7.2l1.9 1.9 3.8-4" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" /></svg>
				Ready to run
			{:else}
				<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><path d="M7 1.6l5.6 10H1.4z" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round" /></svg>
				{missing.length} {missing.length === 1 ? 'thing' : 'things'} before a run
			{/if}
		</span>
	</div>

	<ol class="sections">
		{#each sections as s (s.section)}
			<li class:current={mimo.section === s.section && mimo.step === 'setup'}>
				<button class="row" onclick={() => onopen(s.section)} data-testid="summary-{s.section}" data-missing={s.missing.length}>
					<span class="mark" aria-hidden="true">
						{#if s.errors > 0 || s.missing.length}
							<svg viewBox="0 0 14 14" width="13" height="13"><circle cx="7" cy="7" r="5.5" fill="none" stroke="currentColor" stroke-width="1.5" stroke-dasharray="2.4 1.8" /></svg>
						{:else}
							<svg viewBox="0 0 14 14" width="13" height="13"><circle cx="7" cy="7" r="6" fill="currentColor" /><path d="M4.2 7.2l1.9 1.9 3.8-4" fill="none" stroke="var(--bg-1)" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
						{/if}
					</span>
					<span class="t">{s.title}<span class="visually-hidden">{s.missing.length ? ', needs attention' : ', complete'}</span></span>
					<span class="s">{s.state}</span>
				</button>
				{#if s.missing.length}
					<ul class="miss">
						{#each s.missing as m, i (i)}<li>{m}</li>{/each}
					</ul>
				{/if}
			</li>
		{/each}
	</ol>

	{#if check}
		<dl class="facts" data-testid="summary-shape">
			<div><dt>Shape</dt><dd>{SHAPE[check.shape.shape]}</dd></div>
			<div><dt>Controlled</dt><dd class="num">{check.shape.dofs ?? check.shape.controls} by <span>{check.shape.drives}</span></dd></div>
		</dl>
	{/if}

	<div class="onservice aq-small" data-testid="mimo-setup-status" data-status={status}>
		<h3>On the Service</h3>
		{#if status === 'none'}
			Not applied yet. Apply Setup sends it; the service checks it again and holds it for the run.
		{:else if status === 'applied'}
			Applied{run?.run_id ? ` to run ${run.run_id}` : ''}: {run ? STATE_WORDS[run.state].toLowerCase() : ''}.
		{:else if status === 'changed'}
			Changed on this screen since the service took it. Apply it again to run it.
		{:else}
			The service holds a setup{run?.title ? ` ("${run.title}")` : ''} from another screen or an earlier session. Start runs that one; Apply Setup replaces it with this one.
		{/if}
	</div>
</aside>

<style>
	.summary {
		position: sticky;
		top: 0;
	}

	.check {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}

	.check[data-ready='false'] {
		color: var(--amber-1);
	}

	.sections {
		display: flex;
		flex-direction: column;
		gap: 2px;
		margin: 0;
		padding: 0;
		list-style: none;
	}

	.row {
		display: grid;
		grid-template-columns: 18px minmax(0, 1fr);
		grid-template-areas: 'mark t' '. s';
		width: 100%;
		gap: 0 var(--sp-2);
		padding: 6px 8px;
		text-align: left;
		color: var(--text-1);
		background: transparent;
		border: 1px solid transparent;
		border-radius: var(--radius-md);
	}

	.row:hover {
		background: var(--bg-2);
	}

	.current .row {
		background: var(--blue-weak);
		border-color: var(--blue-line);
	}

	.mark {
		grid-area: mark;
		display: inline-flex;
		align-items: center;
		color: var(--text-2);
	}

	.t {
		grid-area: t;
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
	}

	.s {
		grid-area: s;
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	.miss {
		margin: 2px 0 6px 34px;
		padding: 0;
		list-style: none;
		font-size: var(--text-xs);
		color: var(--amber-1);
	}

	.miss li::before {
		content: '';
		display: inline-block;
		width: 5px;
		height: 5px;
		margin: 0 6px 1px 0;
		border: 1px solid currentColor;
		transform: rotate(45deg);
	}

	.facts {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: var(--sp-2);
		margin: var(--sp-3) 0 0;
		padding-top: var(--sp-3);
		border-top: 1px solid var(--line-1);
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

	.onservice {
		margin-top: var(--sp-3);
		padding-top: var(--sp-3);
		color: var(--text-2);
		border-top: 1px solid var(--line-1);
	}

	.onservice h3 {
		margin-bottom: 4px;
		font-size: var(--text-xs);
		font-weight: var(--weight-strong);
		color: var(--text-1);
	}
</style>
