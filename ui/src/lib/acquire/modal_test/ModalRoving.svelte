<script lang="ts">
	/**
	 * The roving plan: which response rows rove, and the points they take in turn, typed or pasted
	 * (a node a line, or a node and a direction when one row roves), or filled from a start, a step
	 * and a count. The service makes the plan from them and the table's DOFs, and its check lists
	 * each measurement's DOFs.
	 */
	import NumberField from '../NumberField.svelte';
	import Findings from '../Findings.svelte';
	import { modalRows, parsePoints } from './modalTestSetup';
	import type { ModalTestStore } from './modalTestStore.svelte';

	interface Props {
		modal: ModalTestStore;
	}
	let { modal }: Props = $props();

	const d = $derived(modal.setup);
	const locked = $derived(modal.locked);
	const E = $derived(modal.fieldErrors);
	const W = $derived(modal.fieldWarnings);
	const errs = (k: string) => E[k] ?? [];
	const responses = $derived(modalRows(modal.rows).responses);
	const parsed = $derived(d.roving.how === 'list' ? parsePoints(d.roving.text) : null);
	const c = $derived(modal.checkOf);
	const findings = (k: string) => [...errs(k).map((message) => ({ level: 'error' as const, message })), ...(W[k] ?? []).map((message) => ({ level: 'warning' as const, message }))];

	function toggle(index: number, on: boolean) {
		const set = new Set(d.roving.channels);
		if (on) set.add(index);
		else set.delete(index);
		d.roving.channels = [...set].sort((a, b) => a - b);
	}
</script>

<div class="stack">
	<section class="aq-panel" aria-labelledby="mt-rov" data-testid="mt-roving">
		<div class="aq-head"><h2 id="mt-rov">Roving</h2></div>
		<label class="aq-switch" for="mt-rov-on">
			<input id="mt-rov-on" type="checkbox" role="switch" checked={d.roving.on} disabled={locked} onchange={(e) => (d.roving.on = e.currentTarget.checked)} data-testid="mt-rov-on" />
			<span class="track" aria-hidden="true"><span class="thumb"></span></span>
			<span>Rove responses over points</span>
			<span class="word">{d.roving.on ? 'On' : 'Off'}</span>
		</label>
		<p class="help">Off, the test measures one point with every DOF as the table gives it. On, each roving row takes each point's node in turn; the others stay where they are.</p>
		<Findings items={findings('roving')} />

		{#if d.roving.on}
			<div class="aq-group">
				<h3>Roving Rows</h3>
				{#if responses.length}
					<ul class="rows">
						{#each responses as r (r.index)}
							<li>
								<label><input type="checkbox" checked={d.roving.channels.includes(r.index)} disabled={locked} onchange={(e) => toggle(r.index, e.currentTarget.checked)} data-testid="mt-rov-row" data-row={r.index} /> Row {r.index}, {r.description} ({r.node ?? '-'}{r.direction ?? ''})</label>
							</li>
						{/each}
					</ul>
				{:else}
					<p class="aq-small aq-muted">No response row yet: set the responses in DOFs.</p>
				{/if}
				<Findings items={findings('roving.channels')} />
			</div>

			<div class="aq-group">
				<h3>Points</h3>
				<div class="aq-seg sm" role="radiogroup" aria-label="Points from">
					<button type="button" role="radio" aria-checked={d.roving.how === 'list'} disabled={locked} onclick={() => (d.roving.how = 'list')} data-testid="mt-rov-list">Typed or Pasted</button>
					<button type="button" role="radio" aria-checked={d.roving.how === 'fill'} disabled={locked} onclick={() => (d.roving.how = 'fill')} data-testid="mt-rov-fill">Start, Step, Count</button>
				</div>
				{#if d.roving.how === 'list'}
					<label class="lbl" for="mt-rov-text">Points</label>
					<textarea id="mt-rov-text" class="field mono" rows="6" bind:value={d.roving.text} disabled={locked} placeholder={'2\n3\n4 Z+'} data-testid="mt-rov-text"></textarea>
					<p class="help">A node a line, or separated by spaces or commas; a direction after the node (4 Z+) when one row roves. A column pasted from a sheet reads as it is.</p>
					{#if parsed}<p class="aq-small" data-testid="mt-rov-count">{parsed.points.length} {parsed.points.length === 1 ? 'point' : 'points'} read.</p>{/if}
				{:else}
					<div class="aq-fields">
						<NumberField id="mt-rov-start" label="Start node" integer step={1} min={0} bind:value={d.roving.fill.start} errors={errs('roving.points.start')} disabled={locked} testid="mt-rov-start" />
						<NumberField id="mt-rov-step" label="Step" integer step={1} bind:value={d.roving.fill.step} errors={errs('roving.points.step')} disabled={locked} testid="mt-rov-step" />
						<NumberField id="mt-rov-n" label="Count" integer step={1} min={1} bind:value={d.roving.fill.count} errors={errs('roving.points.count')} disabled={locked} testid="mt-rov-n" />
					</div>
				{/if}
				<Findings items={findings('roving.points')} />
			</div>
		{/if}
	</section>

	{#if c && c.points.length}
		<section class="aq-panel" aria-labelledby="mt-plan" data-testid="mt-plan">
			<div class="aq-head"><h2 id="mt-plan">The Plan</h2><span class="aq-sub">From the control service's check.</span></div>
			<table class="aq-table">
				<thead><tr><th class="num">Point</th><th>References</th><th>Responses</th></tr></thead>
				<tbody>
					{#each c.points as p (p.index)}
						<tr>
							<td class="num">{p.index + 1}</td>
							<td>{p.references.map((r) => `${r.node ?? '-'}${r.direction ?? ''}`).join(', ')}</td>
							<td>{p.responses.map((r) => `${r.node ?? '-'}${r.direction ?? ''}${r.roves ? ' (roves)' : ''}`).join(', ')}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</section>
	{/if}
</div>

<style>
	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		max-width: 860px;
	}

	.help {
		font-size: var(--text-xs);
		color: var(--text-3);
		margin: var(--sp-1) 0 0;
	}

	.rows {
		list-style: none;
		padding: 0;
		margin: 0;
		font-size: var(--text-sm);
	}

	textarea {
		width: 100%;
		max-width: 360px;
		display: block;
		margin-top: var(--sp-1);
	}

	.aq-seg {
		margin-bottom: var(--sp-2);
	}
</style>
