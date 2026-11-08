<script lang="ts" generics="R extends object">
	/**
	 * A table of points, a frequency and a value in each row (a target's breakpoints, a limit
	 * channel's gain), edited in place: rows are added after the last and removed one at a time.
	 * A finding on a cell shows beside it; the table's own findings show under it.
	 */
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';

	interface Column {
		key: keyof R & string;
		label: string;
		unit: string;
		step: number;
	}

	interface Props {
		rows: R[];
		columns: [Column, Column];
		/** The findings' keys for row `i` are `<prefix>.<i>.<column>`; the table's own are `<prefix>`. */
		prefix: string;
		errors: Record<string, string[]>;
		locked: boolean;
		/** A new row after the last. */
		make: (last: R | undefined) => R;
		/** At least this many rows stay. */
		min?: number;
		testid: string;
		label: string;
	}
	let { rows, columns, prefix, errors, locked, make, min = 0, testid, label }: Props = $props();

	const errs = (k: string) => errors[k] ?? [];
	const tableErrors = $derived(errs(prefix));
</script>

<div class="ptable" data-testid={testid}>
<table class="aq-table pts" aria-label={label}>
	<thead>
		<tr>
			<th class="r">{columns[0].label}, {columns[0].unit}</th>
			<th class="r">{columns[1].label}, {columns[1].unit}</th>
			<th aria-label="Remove"></th>
		</tr>
	</thead>
	<tbody>
		{#each rows as row, i (i)}
			<tr>
				{#each columns as c (c.key)}
					<td class="num">
						<NumberField
							id="{testid}-{i}-{c.key}"
							label="Row {i + 1}, {c.label.toLowerCase()}"
							compact
							step={c.step}
							value={row[c.key] as unknown as number}
							onchange={(v) => ((row as unknown as Record<string, number>)[c.key] = v ?? NaN)}
							errors={errs(`${prefix}.${i}.${c.key}`)}
							disabled={locked}
							testid="{testid}-{i}-{c.key}"
						/>
					</td>
				{/each}
				<td>
					<button class="aq-icon-btn" aria-label="Remove row {i + 1}" disabled={locked || rows.length <= min} onclick={() => rows.splice(i, 1)}>
						<svg viewBox="0 0 12 12" width="10" height="10" aria-hidden="true"><path d="M3 3l6 6M9 3l-6 6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
					</button>
				</td>
			</tr>
		{/each}
	</tbody>
</table>
<button class="btn sm ghost add" disabled={locked} onclick={() => rows.push(make(rows.at(-1)))} data-testid="{testid}-add">Add Row</button>
<Findings items={tableErrors.map((message) => ({ level: 'error' as const, message }))} />
</div>

<style>
	.pts {
		margin-top: var(--sp-2);
	}

	.r {
		text-align: right;
	}

	.add {
		margin-top: var(--sp-2);
	}
</style>
