<script lang="ts">
	/**
	 * The prediction in short, once identification is done: whether every drive stays within its
	 * ceiling, the largest error the law would leave, what cannot be reached when the controls
	 * outnumber the drives, and the law it was worked out with. The prediction itself, with its
	 * tables and the way to engage, is one click away.
	 */
	import type { MimoStatus } from '$lib/protocol/mimo';
	import StatusMark from '$lib/viz/StatusMark.svelte';
	import { formatDb } from '$lib/viz/format';

	interface Props {
		status: MimoStatus | null;
		/** Open the prediction; absent in a window on another monitor. */
		onopen?: (() => void) | null;
	}
	let { status, onopen = null }: Props = $props();

	const p = $derived(status?.prediction ?? null);
	const names = $derived(status?.shape?.labels ?? []);
	const over = $derived(p ? p.drive_rms_v.filter((r, k) => p.ceiling_vrms[k] !== undefined && r > p.ceiling_vrms[k]).length : 0);
	const exceeded = $derived(p?.feasibility?.filter((f) => f.verdict === 'exceeded').length ?? 0);
	const largest = (xs: number[] | undefined) => {
		let at = -1;
		(xs ?? []).forEach((e, i) => {
			if (Number.isFinite(e) && (at < 0 || Math.abs(e) > Math.abs(xs![at]))) at = i;
		});
		return at < 0 ? null : { i: at, e: xs![at] };
	};
	const worst = $derived(largest(p?.error_db));
	const unreachable = $derived(p?.has_achievable ? largest(p.unreachable_db) : null);
</script>

<div class="pred" data-testid="mimo-prediction-card">
	{#if p}
		<dl>
			<div>
				<dt>Drives</dt>
				<dd>
					<StatusMark status={over ? 'fail' : 'pass'} word={false} />
					{over ? `${over} of ${p.drive_rms_v.length} over the ceiling` : 'All within their ceilings'}
				</dd>
			</div>
			{#if exceeded}
				<div>
					<dt>Shakers</dt>
					<dd><StatusMark status="warn" word={false} />{exceeded} beyond the shaker's rating</dd>
				</div>
			{/if}
			{#if worst}
				<div>
					<dt>Largest error</dt>
					<dd class="num">{formatDb(worst.e, 2)}{names[worst.i] ? ` at ${names[worst.i]}` : ''}</dd>
				</div>
			{/if}
			{#if unreachable && unreachable.e !== 0}
				<div>
					<dt>Unreachable</dt>
					<dd class="num">{formatDb(unreachable.e, 2)}{names[unreachable.i] ? ` at ${names[unreachable.i]}` : ''}</dd>
				</div>
			{/if}
			<div>
				<dt>Law</dt>
				<dd>{p.law.name} <span class="v">{p.law.version}</span>, iteration {p.iteration}</dd>
			</div>
		</dl>
		{#if onopen}
			<button class="btn sm" data-testid="mimo-open-prediction" onclick={onopen}>Open the Prediction</button>
		{/if}
	{:else}
		<p class="aq-muted">Appears after identification.</p>
	{/if}
</div>

<style>
	.pred {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: var(--sp-2);
	}

	dl {
		display: grid;
		grid-template-columns: auto minmax(0, 1fr);
		gap: 3px var(--sp-3);
		margin: 0;
		font-size: var(--text-xs);
	}

	dl div {
		display: contents;
	}

	dt {
		color: var(--text-3);
	}

	dd {
		margin: 0;
		color: var(--text-1);
	}

	dd :global(.mark) {
		margin-right: 4px;
		vertical-align: -2px;
	}

	.v {
		color: var(--text-3);
	}
</style>
