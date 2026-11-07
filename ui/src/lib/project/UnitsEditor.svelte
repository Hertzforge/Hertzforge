<script lang="ts">
	/**
	 * Declare the units a file did not state, by channel, beside the data rather than over it. Each row is a channel, or the object itself; an FRF's response
	 * and reference channels are two tables. The choices are a shortlist: any unit Hertzforge reads
	 * can be typed.
	 */
	import type { ProjectState } from '$lib/app/state.svelte';
	import type { UnitsTables } from '$lib/protocol/types';

	interface Props {
		project: ProjectState;
		tables: UnitsTables;
	}
	let { project, tables }: Props = $props();

	const HEADINGS = { object: '', channels: 'Channels', response: 'Response channels', reference: 'Reference channels' } as const;

	/** What is typed in each row, by side and key; a row left as it was is not sent. */
	let typed = $state<Record<string, string>>({});
	$effect(() => {
		const next: Record<string, string> = {};
		for (const t of tables.tables) for (const r of t.rows) next[`${t.side}/${r.key}`] = r.unit ?? '';
		typed = next;
	});

	const changes = $derived(
		tables.tables.flatMap((t) =>
			t.rows
				.filter((r) => {
					const now = (typed[`${t.side}/${r.key}`] ?? '').trim();
					return now !== '' && now !== (r.unit ?? '');
				})
				.map((r) => ({ side: t.side, key: r.key, unit: typed[`${t.side}/${r.key}`].trim() }))
		)
	);

	async function apply(e: SubmitEvent) {
		e.preventDefault();
		if (changes.length) await project.defineUnits(tables.name, changes);
	}
</script>

<form class="units" onsubmit={apply} data-testid="units-editor" aria-label="Units of {tables.name}">
	<div class="uhead">
		<span class="utitle">Imported units of {tables.name}</span>
		<button type="button" class="btn ghost sm" onclick={() => (project.units = null)}>Close</button>
	</div>
	{#each tables.tables as t (t.side)}
		{#if HEADINGS[t.side]}<p class="side">{HEADINGS[t.side]}</p>{/if}
		<table>
			<tbody>
				{#each t.rows as r (r.key)}
					{@const id = `units-${t.side}-${r.key}`}
					<tr class:open={r.unit === null}>
						<th scope="row"><label for={id}>{r.label}</label></th>
						<td class="q">{r.quantity ?? ''}</td>
						<td>
							<input
								{id}
								class="field mono"
								list="{id}-choices"
								placeholder="unit"
								spellcheck="false"
								autocomplete="off"
								data-testid="unit-{t.side}-{r.key}"
								bind:value={typed[`${t.side}/${r.key}`]}
							/>
							<datalist id="{id}-choices">
								{#each r.choices as c (c)}<option value={c}></option>{/each}
							</datalist>
						</td>
					</tr>
				{/each}
			</tbody>
		</table>
	{/each}
	<div class="uactions">
		<span class="hint">A record converts once each of its channels has a unit.</span>
		<button class="btn primary sm" type="submit" data-testid="units-apply" disabled={!changes.length || project.busy !== null}>Declare</button>
	</div>
</form>

<style>
	.units {
		margin-top: var(--sp-2);
		padding: var(--sp-2);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-sm);
		font-size: var(--text-xs);
	}

	.uhead,
	.uactions {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-2);
	}

	.utitle {
		color: var(--text-1);
	}

	.side {
		margin: var(--sp-2) 0 2px;
		color: var(--text-3);
	}

	table {
		width: 100%;
		border-collapse: collapse;
	}

	th {
		font-weight: normal;
		text-align: left;
		color: var(--text-2);
		white-space: nowrap;
	}

	tr.open th {
		color: var(--amber-2);
	}

	.q {
		padding: 0 6px;
		color: var(--text-4);
	}

	input {
		width: 100%;
		min-width: 4em;
		font-size: var(--text-xs);
	}

	.hint {
		color: var(--text-4);
	}

	.uactions {
		margin-top: var(--sp-2);
	}
</style>
