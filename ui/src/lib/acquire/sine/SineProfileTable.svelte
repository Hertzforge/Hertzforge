<script lang="ts">
	/**
	 * A profile's rows, edited in place: each a frequency, what its value is (acceleration in g peak,
	 * velocity in in/s peak, or displacement in in peak to peak), the value in that quantity's own
	 * unit, and optionally alarm and abort bands of its own from it to the next row. Rows are added
	 * after the last and removed one at a time. A finding on a cell shows beside it; the table's own
	 * findings show under it. The control profile and every limit channel's bound use it, each saved
	 * to and opened from its own kind of breakpoint table file.
	 */
	import type { BandDb } from '$lib/protocol/types';
	import type { LoadedTable, TableFileClient } from '$lib/tables/tableFiles';
	import { sineRows, sineTable, type SineTableKind } from '../breakpointFile';
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import TableFileBar from '../TableFileBar.svelte';
	import { newRow, QUANTITIES, type SineRowDraft } from './sineSetup';

	interface Props {
		rows: SineRowDraft[];
		/** The findings' keys for row `i` are `<prefix>.<i>.<column>`; the table's own are `<prefix>`. */
		prefix: string;
		errors: Record<string, string[]>;
		warnings?: Record<string, string[]>;
		locked: boolean;
		testid: string;
		label: string;
		/** The value column's word ("Value", "Bound"). */
		valueWord?: string;
		/** Offer bands on each row (the control profile); a bound has none. */
		bands?: boolean;
		/**
		 * The table as a file: the analysis service, which kind of Sine table it is, its name, and the
		 * bands a row with only some of its own takes for the rest (the profile's).
		 */
		file?: { client: TableFileClient | null; tableKind: SineTableKind; name: string; base?: () => { alarm_db: BandDb; abort_db: BandDb } };
	}
	let { rows, prefix, errors, warnings = {}, locked, testid, label, valueWord = 'Value', bands = true, file }: Props = $props();

	const errs = (k: string) => errors[k] ?? [];
	const tableErrors = $derived(errs(prefix));
	const tableWarnings = $derived(warnings[prefix] ?? []);

	function add() {
		const last = rows.at(-1);
		rows.push(newRow(last ? Math.round(last.freq_hz * 2) : 100, last?.quantity ?? 'accel_g', last?.value ?? 1));
	}

	function setPair(pair: [number, number], i: 0 | 1, v: number | null) {
		pair[i] = v ?? NaN;
	}

	function openFile(tableKind: SineTableKind, loaded: LoadedTable) {
		const opened = sineRows(loaded, { tableKind, bands, base: file?.base?.() });
		rows.splice(0, rows.length, ...opened.rows);
		return opened.notes;
	}
</script>

<div class="ptable" data-testid={testid}>
	<table class="aq-table" aria-label={label}>
		<thead>
			<tr>
				<th class="num">Frequency, Hz</th>
				<th class="num">{valueWord}</th>
				<th>Unit</th>
				{#if bands}<th>Own bands</th>{/if}
				<th aria-label="Remove"></th>
			</tr>
		</thead>
		<tbody>
			{#each rows as row, i (i)}
				<tr data-testid="{testid}-row-{i}">
					<td class="num">
						<NumberField id="{testid}-{i}-f" label="Row {i + 1}, frequency" compact step={10} min={0} value={row.freq_hz} onchange={(v) => (row.freq_hz = v ?? NaN)} errors={errs(`${prefix}.${i}.freq_hz`)} disabled={locked} testid="{testid}-{i}-freq_hz" />
					</td>
					<td class="num">
						<NumberField id="{testid}-{i}-v" label="Row {i + 1}, {valueWord.toLowerCase()}" compact step={row.quantity === 'disp_pp_in' ? 0.001 : 0.1} min={0} value={row.value} onchange={(v) => (row.value = v ?? NaN)} errors={errs(`${prefix}.${i}.value`)} disabled={locked} testid="{testid}-{i}-value" />
					</td>
					<td>
						<select class="field qty" aria-label="Row {i + 1}, quantity" title={QUANTITIES.find((q) => q.id === row.quantity)?.label} bind:value={row.quantity} disabled={locked} data-testid="{testid}-{i}-quantity">
							{#each QUANTITIES as q (q.id)}<option value={q.id}>{q.unit}</option>{/each}
						</select>
					</td>
					{#if bands}
						<td>
							<label class="own">
								<input type="checkbox" checked={row.own_bands} disabled={locked} onchange={(e) => (row.own_bands = e.currentTarget.checked)} data-testid="{testid}-{i}-own" />
								<span class="visually-hidden">Row {i + 1} has bands of its own</span>
							</label>
							{#if row.own_bands}
								<div class="bands">
									<span class="bw">Alarm</span>
									<NumberField id="{testid}-{i}-al" label="Row {i + 1}, alarm below" compact unit="dB" step={0.5} min={0} value={row.alarm_db[0]} onchange={(v) => setPair(row.alarm_db, 0, v)} disabled={locked} />
									<NumberField id="{testid}-{i}-ah" label="Row {i + 1}, alarm above" compact unit="dB" step={0.5} min={0} value={row.alarm_db[1]} onchange={(v) => setPair(row.alarm_db, 1, v)} disabled={locked} />
									<span class="bw">Abort</span>
									<NumberField id="{testid}-{i}-bl" label="Row {i + 1}, abort below" compact unit="dB" step={0.5} min={0} value={row.abort_db[0]} onchange={(v) => setPair(row.abort_db, 0, v)} disabled={locked} />
									<NumberField id="{testid}-{i}-bh" label="Row {i + 1}, abort above" compact unit="dB" step={0.5} min={0} value={row.abort_db[1]} onchange={(v) => setPair(row.abort_db, 1, v)} disabled={locked} />
								</div>
								<Findings items={[...errs(`${prefix}.${i}.alarm_db`), ...errs(`${prefix}.${i}.abort_db`)].map((message) => ({ level: 'error' as const, message }))} />
							{/if}
						</td>
					{/if}
					<td>
						<button class="aq-icon-btn" aria-label="Remove row {i + 1}" disabled={locked || rows.length <= 2} onclick={() => rows.splice(i, 1)}>
							<svg viewBox="0 0 12 12" width="10" height="10" aria-hidden="true"><path d="M3 3l6 6M9 3l-6 6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
						</button>
					</td>
				</tr>
			{/each}
		</tbody>
	</table>
	{#if file}
		{@const tableKind = file.tableKind}
		<TableFileBar client={file.client} {tableKind} name={file.name} table={() => sineTable(rows, { tableKind, bands })} onopen={(loaded) => openFile(tableKind, loaded)} {locked} {testid}>
			<button class="btn sm ghost" disabled={locked} onclick={add} data-testid="{testid}-add">Add Row</button>
		</TableFileBar>
	{:else}
		<button class="btn sm ghost add" disabled={locked} onclick={add} data-testid="{testid}-add">Add Row</button>
	{/if}
	<Findings items={[...tableErrors.map((message) => ({ level: 'error' as const, message })), ...tableWarnings.map((message) => ({ level: 'warning' as const, message }))]} testid="{testid}-findings" />
</div>

<style>
	.ptable {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		min-width: 0;
		overflow-x: auto;
	}

	.qty {
		min-width: 8.5em;
	}

	.own {
		display: inline-flex;
		align-items: center;
	}

	.bands {
		display: grid;
		grid-template-columns: auto auto auto;
		align-items: center;
		gap: 3px 4px;
		margin-top: 4px;
	}

	.bw {
		font-size: var(--text-2xs);
		color: var(--text-3);
	}

	.add {
		align-self: flex-start;
	}

	.ptable :global(.tfb) {
		margin-top: 0;
	}
</style>
