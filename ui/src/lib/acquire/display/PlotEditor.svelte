<script lang="ts">
	/**
	 * A plot's settings: what it shows, which channels, how several channels are drawn, and its
	 * axes. Every change applies at once, including during a run; the run itself is never touched,
	 * only what this window asks the service to send.
	 */
	import type { ChannelSelector, LiveQuantity } from '$lib/protocol/types';
	import { hasAxes, hasChannels, QUANTITY_HELP, QUANTITY_ORDER, QUANTITY_WORDS, type PlotSpec, type ScaleChoice } from './layout';

	interface Channel {
		index: number;
		name: string;
	}

	interface Props {
		plot: PlotSpec;
		/** The inputs the operator can pick from: the channel table's enabled inputs. */
		inputs: Channel[];
		quantities: LiveQuantity[];
		onchange: (patch: Partial<Omit<PlotSpec, 'id'>>) => void;
		onclose: () => void;
	}
	let { plot, inputs, quantities, onchange, onclose }: Props = $props();

	const offered = $derived(QUANTITY_ORDER.filter((q) => quantities.includes(q)));
	const mode = $derived(Array.isArray(plot.channels) ? 'pick' : plot.channels);
	const picked = $derived(Array.isArray(plot.channels) ? plot.channels : []);
	const fixed = $derived(plot.yMin !== null && plot.yMax !== null);
	const uid = $derived(`pe-${plot.id}`);

	let yMinText = $state('');
	let yMaxText = $state('');
	$effect(() => {
		yMinText = plot.yMin === null ? '' : String(plot.yMin);
		yMaxText = plot.yMax === null ? '' : String(plot.yMax);
	});

	function setQuantity(q: LiveQuantity) {
		// A new quantity starts on its own scales and range.
		onchange({ quantity: q, x: 'auto', y: 'auto', yMin: null, yMax: null });
	}

	function setMode(m: string) {
		if (m === 'pick') onchange({ channels: picked.length ? picked : inputs.slice(0, 1).map((c) => c.index) });
		else onchange({ channels: m as ChannelSelector });
	}

	function toggle(index: number, on: boolean) {
		const next = on ? [...new Set([...picked, index])].sort((a, b) => a - b) : picked.filter((i) => i !== index);
		onchange({ channels: next });
	}

	function setFixed(on: boolean) {
		if (!on) onchange({ yMin: null, yMax: null });
		else onchange({ yMin: plot.yMin ?? (plot.quantity === 'coherence' ? 0 : plot.y === 'linear' ? -1 : 1e-6), yMax: plot.yMax ?? 1 });
	}

	function commitRange() {
		const lo = Number(yMinText);
		const hi = Number(yMaxText);
		if (yMinText.trim() !== '' && yMaxText.trim() !== '' && Number.isFinite(lo) && Number.isFinite(hi) && hi > lo) onchange({ yMin: lo, yMax: hi });
	}

	const rangeBad = $derived.by(() => {
		if (!fixed) return null;
		const lo = Number(yMinText);
		const hi = Number(yMaxText);
		if (!Number.isFinite(lo) || !Number.isFinite(hi) || yMinText.trim() === '' || yMaxText.trim() === '') return 'Enter both ends of the range.';
		if (!(hi > lo)) return 'The top must be above the bottom.';
		if (plot.y === 'log' && lo <= 0) return 'A log axis needs a bottom above zero.';
		return null;
	});

	function onKey(e: KeyboardEvent) {
		if (e.key === 'Escape') {
			e.stopPropagation();
			onclose();
		}
	}

	const SCALES: [ScaleChoice, string][] = [
		['auto', 'Auto'],
		['log', 'Log'],
		['linear', 'Linear']
	];
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div class="editor" role="group" aria-label="Plot settings" data-testid="plot-editor" onkeydown={onKey}>
	<div class="row">
		<label class="lbl" for="{uid}-q">Shows</label>
		<div class="field-col">
			<select id="{uid}-q" class="field" data-testid="plot-quantity" value={plot.quantity} onchange={(e) => setQuantity(e.currentTarget.value as LiveQuantity)}>
				{#each offered as q (q)}<option value={q}>{QUANTITY_WORDS[q]}</option>{/each}
			</select>
			<p class="help">{QUANTITY_HELP[plot.quantity]}</p>
		</div>
	</div>

	{#if hasChannels(plot.quantity)}
		<div class="row">
			<span class="lbl">Channels</span>
			<div class="field-col">
				<div class="aq-seg sm" role="radiogroup" aria-label="Channels" data-testid="plot-channels">
					{#each [['all', 'All inputs'], ['control', 'Control'], ['limited', 'Limited'], ['pick', 'Chosen']] as [m, label] (m)}
						<button type="button" role="radio" aria-checked={mode === m} data-testid="plot-channels-{m}" onclick={() => setMode(m)}>{label}</button>
					{/each}
				</div>
				{#if mode === 'pick'}
					<ul class="picks" data-testid="plot-picks">
						{#each inputs as c (c.index)}
							<li>
								<label><input type="checkbox" checked={picked.includes(c.index)} onchange={(e) => toggle(c.index, e.currentTarget.checked)} data-testid="plot-pick-{c.index}" /> <span class="num">{c.index}</span> {c.name}</label>
							</li>
						{:else}
							<li class="help">The channel table has no enabled inputs.</li>
						{/each}
					</ul>
				{/if}
			</div>
		</div>
		<div class="row">
			<span class="lbl">Several channels</span>
			<div class="aq-seg sm" role="radiogroup" aria-label="Several channels" data-testid="plot-arrange">
				<button type="button" role="radio" aria-checked={plot.arrange === 'split'} onclick={() => onchange({ arrange: 'split' })}>One pane each</button>
				<button type="button" role="radio" aria-checked={plot.arrange === 'overlay'} onclick={() => onchange({ arrange: 'overlay' })}>All on one pane</button>
			</div>
		</div>
	{/if}

	{#if plot.quantity === 'time'}
		<div class="row">
			<label class="lbl" for="{uid}-pts">Points per channel</label>
			<select id="{uid}-pts" class="field num" value={String(plot.points)} onchange={(e) => onchange({ points: Number(e.currentTarget.value) })}>
				{#each [256, 512, 1024, 2048, 4096, 8192] as n (n)}<option value={String(n)}>{n}</option>{/each}
			</select>
		</div>
	{/if}

	{#if hasAxes(plot.quantity)}
		<div class="row">
			<span class="lbl">X axis</span>
			<div class="aq-seg sm" role="radiogroup" aria-label="X axis scale" data-testid="plot-x">
				{#each SCALES as [v, label] (v)}
					<button type="button" role="radio" aria-checked={plot.x === v} onclick={() => onchange({ x: v })}>{label}</button>
				{/each}
			</div>
		</div>
		<div class="row">
			<span class="lbl">Y axis</span>
			<div class="field-col">
				<div class="aq-seg sm" role="radiogroup" aria-label="Y axis scale" data-testid="plot-y">
					{#each SCALES as [v, label] (v)}
						<button type="button" role="radio" aria-checked={plot.y === v} disabled={v === 'log' && plot.quantity === 'time'} title={v === 'log' && plot.quantity === 'time' ? 'A time history swings below zero, so it has no log scale.' : undefined} onclick={() => onchange({ y: v })}>{label}</button>
					{/each}
				</div>
				<label class="check"><input type="checkbox" checked={fixed} onchange={(e) => setFixed(e.currentTarget.checked)} data-testid="plot-y-fixed" /> Fixed range</label>
				{#if fixed}
					<div class="range">
						<input class="field num" aria-label="Y axis bottom" bind:value={yMinText} onchange={commitRange} data-testid="plot-y-min" />
						<span aria-hidden="true">to</span>
						<input class="field num" aria-label="Y axis top" bind:value={yMaxText} onchange={commitRange} data-testid="plot-y-max" />
					</div>
					{#if rangeBad}<p class="help bad" role="alert">{rangeBad}</p>{/if}
				{/if}
			</div>
		</div>
	{/if}

	<div class="foot">
		<button class="btn sm primary" data-testid="plot-editor-done" onclick={onclose}>Done</button>
	</div>
</div>

<style>
	.editor {
		display: flex;
		flex-direction: column;
		gap: 10px;
		padding: var(--sp-3);
		background: var(--bg-1);
		border: 1px solid var(--line-2);
		border-radius: var(--radius-md);
		box-shadow: var(--shadow-pop);
	}

	.row {
		display: grid;
		grid-template-columns: 9.5em minmax(0, 1fr);
		align-items: start;
		gap: var(--sp-2);
	}

	.lbl {
		padding-top: 5px;
		font-size: var(--text-xs);
		font-weight: var(--weight-strong);
		color: var(--text-2);
	}

	.field-col {
		display: flex;
		flex-direction: column;
		gap: 6px;
		min-width: 0;
	}

	.help {
		font-size: var(--text-xs);
		color: var(--text-3);
	}

	.help.bad {
		color: var(--amber-1);
		font-weight: var(--weight-strong);
	}

	.picks {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(11em, 1fr));
		gap: 2px 10px;
		max-height: 140px;
		margin: 0;
		padding: 0;
		overflow-y: auto;
		list-style: none;
		font-size: var(--text-xs);
	}

	.picks label,
	.check {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		font-size: var(--text-xs);
		color: var(--text-1);
	}

	.range {
		display: flex;
		align-items: center;
		gap: 6px;
	}

	.range input {
		width: 8em;
	}

	.editor select.field {
		width: auto;
		min-width: 14em;
	}

	.foot {
		display: flex;
		justify-content: flex-end;
	}
</style>
