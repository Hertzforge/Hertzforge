<script lang="ts">
	/**
	 * The drives at a glance: each drive's rms against its ceiling as a bar with the ceiling marked,
	 * the numbers, and this loop's cap factor. A drive over its ceiling is marked and its row set in
	 * bold; a capped drive carries a mark. A drive's name shows it on the page's drive spectrum.
	 */
	import type { MimoStatus } from '$lib/protocol/mimo';
	import StatusMark from '$lib/viz/StatusMark.svelte';
	import { formatFixed } from '$lib/viz/format';

	interface Props {
		status: MimoStatus | null;
		/** Each drive's voltage ceiling (V rms), in the setup's drive order. */
		ceilings?: number[] | null;
		/** Show drive `k` on the page's drive spectrum; false when the page has none showing one drive. */
		onpick?: ((k: number) => boolean) | null;
	}
	let { status, ceilings = null, onpick = null }: Props = $props();

	const drives = $derived(status?.shape?.drive_labels ?? []);
	const rms = $derived(status?.drive_rms_v ?? []);
	const ceil = $derived(ceilings ?? status?.prediction?.ceiling_vrms ?? []);
	const caps = $derived(status?.caps ?? []);
	const biggest = $derived(Math.max(1e-12, ...ceil.filter((x) => Number.isFinite(x)), ...rms.filter((x) => Number.isFinite(x))));
	let picked = $state<number | null>(null);
</script>

<div class="drives" data-testid="mimo-drives">
	{#if drives.length}
		<table aria-label="Drive rms against its ceiling" data-testid="plant-drive-rms">
			<thead><tr><th>Drive</th><th class="bar">Against ceiling</th><th class="r">V rms</th><th class="r">Ceiling</th><th class="r">Cap</th></tr></thead>
			<tbody>
				{#each drives as d, k (k)}
					{@const r = rms[k]}
					{@const c = ceil[k]}
					{@const cap = caps[k]}
					{@const over = r !== undefined && c !== undefined && r > c}
					<tr class:over class:sel={picked === k}>
						<td>
							{#if onpick}
								<button class="name" title="Show {d} on this page's drive spectrum" onclick={() => (picked = onpick(k) ? k : null)}>{d}</button>
							{:else}
								{d}
							{/if}
							{#if over}<StatusMark status="fail" word={false} />{/if}
						</td>
						<td class="bar">
							<span class="track">
								<span class="fill" style:width="{r === undefined ? 0 : Math.min(100, (r / biggest) * 100)}%"></span>
								{#if c !== undefined}<span class="ceil" style:left="{Math.min(100, (c / biggest) * 100)}%" title="Ceiling"></span>{/if}
							</span>
						</td>
						<td class="r num">{r === undefined ? '-' : formatFixed(r, 3)}</td>
						<td class="r num">{c === undefined ? '-' : formatFixed(c, 3)}</td>
						<td class="r num">
							{#if cap !== undefined && cap < 1}<StatusMark status="warn" word={false} />{/if}
							{cap === undefined ? '-' : formatFixed(cap, 3)}
						</td>
					</tr>
				{/each}
			</tbody>
		</table>
	{:else}
		<p class="aq-muted">Appears when a run starts.</p>
	{/if}
</div>

<style>
	.drives {
		max-height: 320px;
		overflow-y: auto;
	}

	table {
		width: 100%;
		border-collapse: collapse;
		font-size: var(--text-xs);
	}

	th {
		padding: 3px 4px;
		font-size: var(--text-2xs);
		font-weight: var(--weight-strong);
		color: var(--text-3);
		text-align: left;
		text-transform: uppercase;
		letter-spacing: 0.04em;
		border-bottom: 1px solid var(--line-2);
	}

	td {
		padding: 3px 4px;
		border-bottom: 1px solid var(--line-1);
		white-space: nowrap;
	}

	.r {
		text-align: right;
	}

	.bar {
		width: 35%;
	}

	tr.over td {
		font-weight: var(--weight-strong);
	}

	tr.sel td {
		background: var(--accent-weak);
	}

	.name {
		padding: 0;
		font: inherit;
		color: inherit;
		background: none;
		border: 0;
		text-decoration: underline dotted;
		text-underline-offset: 2px;
		cursor: pointer;
	}

	.track {
		position: relative;
		display: block;
		height: 8px;
		background: var(--bg-3);
		border-radius: 2px;
	}

	.fill {
		position: absolute;
		inset: 0 auto 0 0;
		background: var(--series-drive);
		border-radius: 2px;
	}

	.over .fill {
		background: var(--status-fail);
	}

	.ceil {
		position: absolute;
		top: -3px;
		bottom: -3px;
		width: 2px;
		margin-left: -1px;
		background: var(--text-1);
	}
</style>
