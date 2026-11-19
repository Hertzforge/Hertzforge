<script lang="ts">
	/**
	 * A transformation matrix as a labeled grid. Cells take numbers as typed; a block copied from a
	 * spreadsheet (tab-separated) pastes into the grid from the cell it lands on; a CSV file fills
	 * it from the top left. Below the grid, the service's rank and condition readout. A matrix a
	 * reference brings is shown read-only.
	 */
	import Findings from '../Findings.svelte';
	import { formatNumber } from '$lib/viz/format';
	import type { MimoTransformCheck } from '$lib/protocol/mimo';
	import { identityMatrix, matrixFromCsv, parseGrid, pasteInto, type MatrixDraft } from './mimoSetup';

	interface Props {
		id: string;
		title: string;
		/** One line on what the matrix maps. */
		sub: string;
		matrix: MatrixDraft;
		rowLabels: string[];
		colLabels: string[];
		/** The service's readout, or null before the dry run. */
		check: MimoTransformCheck | null;
		/** The rank the matrix must reach, and in which words. */
		needs: { rank: number; words: string };
		errors?: string[];
		warnings?: string[];
		readonly?: boolean;
		disabled?: boolean;
		onchange: (m: MatrixDraft) => void;
	}
	let { id, title, sub, matrix, rowLabels, colLabels, check, needs, errors = [], warnings = [], readonly = false, disabled = false, onchange }: Props = $props();

	let note = $state<{ level: 'error' | 'warning' | 'note'; message: string } | null>(null);
	let fileInput = $state<HTMLInputElement | null>(null);
	let drafts = $state<Record<number, string>>({});

	const cell = (r: number, c: number) => matrix.values[r * matrix.cols + c];
	const text = (k: number) => drafts[k] ?? (Number.isFinite(matrix.values[k]) ? String(matrix.values[k]) : '');

	function set(k: number, raw: string) {
		drafts[k] = raw;
		const v = raw.trim() === '' ? NaN : Number(raw);
		const values = [...matrix.values];
		values[k] = v;
		onchange({ ...matrix, values, source: matrix.source === 'Pasted' || matrix.source === null ? matrix.source : 'Edited' });
	}

	function blur(k: number) {
		delete drafts[k];
	}

	function onPaste(e: ClipboardEvent, r: number, c: number) {
		const raw = e.clipboardData?.getData('text/plain') ?? '';
		if (!/[\t\n]/.test(raw.trim())) return;
		e.preventDefault();
		const { cells, bad } = parseGrid(raw);
		const { matrix: next, outside } = pasteInto(matrix, cells, r, c);
		drafts = {};
		onchange({ ...next, source: 'Pasted' });
		const words: string[] = [];
		if (bad.length) words.push(`${bad.length} ${bad.length === 1 ? 'cell does' : 'cells do'} not read as a number and ${bad.length === 1 ? 'is' : 'are'} left empty`);
		if (outside) words.push(`${outside} ${outside === 1 ? 'value falls' : 'values fall'} outside the ${matrix.rows} by ${matrix.cols} grid and ${outside === 1 ? 'was' : 'were'} left out`);
		note = words.length ? { level: 'warning', message: `Pasted. ${words.join('; ')}.` } : { level: 'note', message: `Pasted ${cells.length} ${cells.length === 1 ? 'row' : 'rows'}.` };
	}

	async function onFile(e: Event) {
		const input = e.currentTarget as HTMLInputElement;
		const file = input.files?.[0];
		input.value = '';
		if (!file) return;
		const parsed = matrixFromCsv(await file.text());
		if (parsed.error) {
			note = { level: 'error', message: `${file.name}: ${parsed.error}` };
			return;
		}
		const rows = parsed.values.length;
		const cols = parsed.values[0]?.length ?? 0;
		if (rows !== matrix.rows || cols !== matrix.cols) {
			note = { level: 'error', message: `${file.name} holds a ${rows} by ${cols} matrix; this one is ${matrix.rows} by ${matrix.cols} (${needs.words}). Check the file against the channels.` };
			return;
		}
		drafts = {};
		const { matrix: next } = pasteInto(matrix, parsed.values);
		onchange({ ...next, source: file.name });
		note = { level: 'note', message: `Read ${file.name}.` };
	}

	function identity() {
		drafts = {};
		onchange({ ...identityMatrix(matrix.rows, matrix.cols), source: 'Identity' });
		note = null;
	}

	const rankShort = $derived(check ? check.rank < needs.rank : false);
</script>

<div class="matrix" data-testid="{id}-matrix">
	<div class="mhead">
		<h3>{title}</h3>
		<span class="aq-muted aq-small">{sub}</span>
		<span class="aq-spacer"></span>
		{#if !readonly}
			<div class="aq-actions">
				<button class="btn sm" {disabled} onclick={() => fileInput?.click()} data-testid="{id}-import">Import CSV</button>
				<button class="btn sm ghost" {disabled} onclick={identity}>Start From Identity</button>
			</div>
			<input type="file" accept=".csv,.tsv,.txt,text/csv" class="visually-hidden" bind:this={fileInput} onchange={onFile} tabindex="-1" aria-hidden="true" />
		{/if}
	</div>

	{#if matrix.rows === 0 || matrix.cols === 0}
		<p class="aq-muted aq-small empty">The grid appears once it has rows and columns: {needs.words}.</p>
	{:else}
		<div class="aq-scroll grid-wrap">
			<table class="grid" aria-label={title}>
				<thead>
					<tr>
						<th scope="col"><span class="visually-hidden">Row</span></th>
						{#each colLabels as c, j (j)}<th scope="col" title={c}>{c}</th>{/each}
					</tr>
				</thead>
				<tbody>
					{#each { length: matrix.rows } as _, r (r)}
						<tr>
							<th scope="row" title={rowLabels[r]}>{rowLabels[r] ?? r + 1}</th>
							{#each { length: matrix.cols } as _, c (c)}
								{@const k = r * matrix.cols + c}
								<td>
									{#if readonly}
										<span class="num ro">{formatNumber(cell(r, c), 4)}</span>
									{:else}
										<input
											class="num"
											class:bad={!Number.isFinite(cell(r, c))}
											inputmode="decimal"
											aria-label="{title}, {rowLabels[r] ?? r + 1}, {colLabels[c] ?? c + 1}"
											value={text(k)}
											{disabled}
											data-testid="{id}-{r}-{c}"
											oninput={(e) => set(k, e.currentTarget.value)}
											onblur={() => blur(k)}
											onpaste={(e) => onPaste(e, r, c)}
										/>
									{/if}
								</td>
							{/each}
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}

	<div class="readout" data-testid="{id}-readout" aria-live="polite">
		{#if check}
			<span class="fact" class:short={rankShort}>
				{#if rankShort}
					<svg viewBox="0 0 14 14" width="11" height="11" aria-hidden="true"><path d="M4.5 1h5L13 4.5v5L9.5 13h-5L1 9.5v-5z" fill="currentColor" /></svg>
				{:else}
					<svg viewBox="0 0 14 14" width="11" height="11" aria-hidden="true"><path d="M2.5 7.3l2.8 2.8 6-6.3" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" /></svg>
				{/if}
				Rank <span class="num">{check.rank}</span> of <span class="num">{needs.rank}</span> needed
			</span>
			<span class="fact">Condition <span class="num">{check.condition === null ? 'not defined (rank short)' : formatNumber(check.condition, 4)}</span></span>
		{:else}
			<span class="aq-muted">Rank and condition come from the control service once the setup is checked.</span>
		{/if}
		{#if matrix.source}<span class="aq-muted src">From: {matrix.source}</span>{/if}
	</div>
	<Findings items={[...errors.map((message) => ({ level: 'error' as const, message })), ...warnings.map((message) => ({ level: 'warning' as const, message })), ...(note ? [note] : [])]} />
	{#if !readonly}<p class="aq-muted aq-small tip">Copy a block of cells in a spreadsheet and paste it into any cell; it fills from there.</p>{/if}
</div>

<style>
	.matrix + :global(.matrix) {
		margin-top: var(--sp-4);
		padding-top: var(--sp-4);
		border-top: 1px solid var(--line-1);
	}

	.mhead {
		display: flex;
		flex-wrap: wrap;
		align-items: baseline;
		gap: 4px var(--sp-3);
		margin-bottom: var(--sp-2);
	}

	h3 {
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
	}

	.aq-spacer {
		flex: 1;
	}

	.grid-wrap {
		max-height: 340px;
		border: 1px solid var(--line-1);
		border-radius: var(--radius-md);
	}

	.grid {
		border-collapse: separate;
		border-spacing: 0;
		font-size: var(--text-xs);
	}

	.grid th {
		position: sticky;
		top: 0;
		z-index: 1;
		max-width: 96px;
		padding: 4px 6px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-weight: var(--weight-medium);
		color: var(--text-2);
		background: var(--bg-2);
		border-bottom: 1px solid var(--line-1);
	}

	.grid tbody th {
		left: 0;
		text-align: left;
		border-right: 1px solid var(--line-1);
	}

	.grid td {
		padding: 1px;
		border-bottom: 1px solid var(--line-1);
	}

	.grid input {
		width: 76px;
		height: 24px;
		padding: 0 6px;
		font-size: var(--text-xs);
		text-align: right;
		color: var(--text-1);
		background: var(--bg-0);
		border: 1px solid transparent;
		border-radius: var(--radius-sm);
	}

	.grid input:focus {
		outline: none;
		border-color: var(--blue-2);
	}

	.grid input.bad {
		border: 1.5px dashed var(--amber-2);
	}

	.ro {
		display: inline-block;
		min-width: 64px;
		padding: 0 6px;
		text-align: right;
	}

	.readout {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 4px var(--sp-4);
		margin-top: var(--sp-2);
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	.fact {
		display: inline-flex;
		align-items: center;
		gap: 5px;
	}

	.fact.short {
		font-weight: var(--weight-strong);
		color: var(--amber-1);
	}

	.empty {
		padding: var(--sp-3);
		background: var(--bg-plot);
		border: 1px dashed var(--line-2);
		border-radius: var(--radius-md);
	}

	.tip {
		margin-top: 4px;
	}
</style>
