<script lang="ts">
	/**
	 * A bar a row (a channel's RMS error, its kurtosis, its signal to noise), the rows named down
	 * the side so a long column of channels stays legible, and the chart as tall as its rows.
	 * The service has judged every bar: one past a threshold is drawn in the fail color and says
	 * which side it is out on in words, so nothing rests on color alone. The ground past each
	 * threshold is shaded. A row with no number draws no bar and keeps its name and its note.
	 */
	import type { BarsView } from '$lib/protocol/types';
	import { barSpan, barsAxis, barText, place } from './bars';

	interface Props {
		view: BarsView;
		showTitle?: boolean;
	}

	let { view, showTitle = true }: Props = $props();

	const ROW = 18;
	const TOP = 6;
	const BOTTOM = 34;
	const RIGHT = 52;

	let width = $state(0);
	const axis = $derived(barsAxis(view));
	const base = $derived(view.baseline ?? 0);
	/** Room for the longest name, within reason. */
	const left = $derived(Math.min(170, Math.max(56, 12 + 6.6 * Math.max(0, ...view.labels.map((l) => l.length)))));
	const plotW = $derived(Math.max(40, width - left - RIGHT));
	const height = $derived(TOP + BOTTOM + view.labels.length * ROW);
	const x = (v: number) => place(axis, left, plotW, v);
	const unit = $derived(view.value.unit ? ` (${view.value.unit})` : '');
	const outCount = $derived(view.states.filter((s) => s !== null).length);
</script>

<div class="bars" data-testid="view-bars">
	{#if showTitle}
		<h3 class="title">{view.title}</h3>
	{/if}
	<p class="summary" aria-live="polite">
		{outCount === 0 ? `All ${view.labels.length} within limits` : `${outCount} of ${view.labels.length} outside limits`}
	</p>
	<div class="plot" bind:clientWidth={width}>
		{#if width > 0}
			<svg {width} {height} role="img" aria-label={`${view.title}: ${view.value.label}${unit}, one bar a row`}>
				{#if view.over !== null && view.over !== undefined && view.over < axis.hi}
					<rect class="beyond" x={x(view.over)} y={TOP} width={left + plotW - x(view.over)} height={view.labels.length * ROW} />
				{/if}
				{#if view.under !== null && view.under !== undefined && view.under > axis.lo}
					<rect class="beyond" x={left} y={TOP} width={x(view.under) - left} height={view.labels.length * ROW} />
				{/if}
				{#each axis.ticks as t (t)}
					<line class="grid" x1={x(t)} x2={x(t)} y1={TOP} y2={TOP + view.labels.length * ROW} />
					<text class="tick" x={x(t)} y={TOP + view.labels.length * ROW + 14} text-anchor="middle">{t.toFixed(axis.decimals)}</text>
				{/each}
				<line class="base" x1={x(base)} x2={x(base)} y1={TOP} y2={TOP + view.labels.length * ROW} />
				{#each view.labels as label, k (k)}
					{@const y = TOP + k * ROW}
					{@const span = barSpan(axis, left, plotW, base, view.values[k])}
					{@const state = view.states[k]}
					<g class="row" class:out={state !== null}>
						<title>{barText(view, k)}</title>
						<text class="label" x={left - 6} y={y + ROW * 0.68} text-anchor="end">{label}</text>
						{#if span}
							<rect class="bar" x={span[0]} y={y + ROW * 0.16} width={Math.max(1, span[1] - span[0])} height={ROW * 0.68} />
						{/if}
						{#if view.notes?.[k]}
							<text class="note" x={x(base) + 4} y={y + ROW * 0.68}>{view.notes[k]}</text>
						{/if}
						{#if state}
							<text class="state" x={left + plotW + 6} y={y + ROW * 0.68}>{state}</text>
						{/if}
					</g>
				{/each}
				<text class="axis-label" x={left + plotW / 2} y={height - 4} text-anchor="middle">{view.value.label}{unit}</text>
			</svg>
		{/if}
	</div>
</div>

<style>
	.bars {
		display: flex;
		flex-direction: column;
		min-width: 0;
		gap: 2px;
	}

	.title {
		margin: 0;
		font-size: var(--text-sm);
		font-weight: 600;
	}

	.summary {
		margin: 0;
		font-size: var(--text-2xs, 11px);
		color: var(--text-2);
	}

	.plot {
		width: 100%;
		min-width: 0;
	}

	svg {
		display: block;
		font-family: var(--font-ui);
		font-size: 10px;
	}

	.beyond {
		fill: var(--amber-weak);
	}

	.grid {
		stroke: var(--line-1);
		stroke-width: 1;
	}

	.base {
		stroke: var(--text-3);
		stroke-width: 1;
	}

	.tick,
	.axis-label {
		fill: var(--text-2);
	}

	.label {
		fill: var(--text-1);
		font-family: var(--font-mono);
	}

	.bar {
		fill: var(--text-3);
		opacity: 0.75;
	}

	.out .bar {
		fill: var(--status-fail-fill);
		opacity: 1;
	}

	.note {
		fill: var(--text-2);
		font-style: italic;
	}

	.state {
		fill: var(--status-fail);
		font-weight: 600;
	}
</style>
