<script lang="ts">
	/**
	 * The verdict grid: controls down, pulses across, one cell per control per pulse. A passing cell
	 * holds a check and the word Pass; a failing one an octagon and the reasons by name (Tol, Time,
	 * Pol, Dur); a cell over its scaled abort line also carries a raised bar. Mark and word say it,
	 * never color alone. A column is picked to show that pulse's judgement in detail.
	 */
	import type { MimoShockPulseRow } from '$lib/protocol/mimoShock';
	import { db } from '../words';
	import { verdictGrid } from './msGrid';

	interface Props {
		rows: MimoShockPulseRow[];
		controls: number[];
		/** The pulse shown in detail. */
		shown: number | null;
		onpick: (index: number | null) => void;
	}
	let { rows, controls, shown, onpick }: Props = $props();

	const grid = $derived(verdictGrid(rows, controls));
</script>

<div class="vg" data-testid="ms-verdict-grid">
	{#if !grid.columns.length}
		<p class="aq-small aq-muted">Each control's verdict on each pulse appears here once a pulse has fired.</p>
	{:else}
		{#if grid.hidden}<p class="aq-small aq-muted">The latest {grid.columns.length} pulses; {grid.hidden} earlier.</p>{/if}
		<div class="scroll">
			<table aria-label="Verdicts by control and pulse">
				<thead>
					<tr>
						<th scope="col">Control</th>
						{#each grid.columns as c (c.index)}
							<th scope="col" class:picked={shown === c.index}>
								<button class="colbtn" aria-pressed={shown === c.index} title="Pulse {c.index + 1} at {db(c.level_db)}: {c.passed ? 'passed' : 'did not pass'}. Show it in detail." onclick={() => onpick(shown === c.index ? null : c.index)} data-testid="ms-grid-pulse-{c.index}">
									<span class="pn num">{c.index + 1}</span>
									<span class="lv num">{db(c.level_db, 1)}</span>
									<span class="jv" data-joint={c.passed ? 'pass' : 'fail'}>{c.passed ? 'Pass' : 'Fail'}</span>
								</button>
							</th>
						{/each}
					</tr>
				</thead>
				<tbody>
					{#each grid.controls as ch, r (ch)}
						<tr>
							<th scope="row">{ch}</th>
							{#each grid.cells[r] as cell, k (k)}
								<td class:picked={shown === grid.columns[k].index}>
									{#if cell}
										<span class="cell" class:pass={cell.passed} class:fail={!cell.passed} title={cell.label} aria-label={cell.label} data-testid="ms-cell-{ch}-{cell.pulse}" data-verdict={cell.passed ? 'pass' : 'fail'}>
											<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true">
												{#if cell.passed}
													<path d="M2.5 7.3l2.8 2.8 6-6.3" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" />
												{:else}
													<path d="M4.5 1h5L13 4.5v5L9.5 13h-5L1 9.5v-5z" fill="currentColor" /><path d="M5 5l4 4M9 5l-4 4" stroke="var(--status-fail-text)" stroke-width="1.6" stroke-linecap="round" />
												{/if}
											</svg>
											<span class="t">{cell.text}</span>
											{#if cell.scaledOver}<span class="over" title="Over its scaled abort line">Over line</span>{/if}
										</span>
									{:else}
										<span class="aq-muted">-</span>
									{/if}
								</td>
							{/each}
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
		<p class="key aq-small aq-muted">Fail reasons: Tol, outside tolerance; Time, outside the timing window; Pol, upside down; Dur, duration out of range.</p>
	{/if}
</div>

<style>
	.vg {
		min-width: 0;
	}

	.scroll {
		overflow-x: auto;
	}

	table {
		border-collapse: collapse;
		font-size: var(--text-xs);
	}

	th,
	td {
		padding: 3px 5px;
		border-bottom: 1px solid var(--line-1);
		text-align: left;
		white-space: nowrap;
	}

	th[scope='row'] {
		font-weight: var(--weight-strong);
	}

	.picked {
		background: var(--bg-2);
		box-shadow: inset 2px 0 0 var(--blue-2), inset -2px 0 0 var(--blue-2);
	}

	.colbtn {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		padding: 0;
		font-size: var(--text-2xs);
		color: var(--text-2);
		background: none;
		border: 0;
	}

	.pn {
		font-weight: var(--weight-strong);
		color: var(--text-1);
	}

	.jv[data-joint='fail'] {
		text-decoration: underline dotted;
	}

	.cell {
		display: inline-flex;
		align-items: center;
		gap: 3px;
	}

	.cell.pass {
		color: var(--blue-1);
	}

	.cell.fail {
		color: var(--amber-1);
		font-weight: var(--weight-strong);
	}

	.over {
		padding: 0 3px;
		font-size: var(--text-2xs);
		border: 1px solid currentColor;
		border-radius: 3px;
	}

	.key {
		margin: var(--sp-1) 0 0;
	}
</style>
