<script lang="ts">
	/**
	 * The cross terms of the reference: for each pair of quantities, the coherence (0 to 1) and
	 * the phase (degrees), each a constant or a table of points against frequency. Pairs not
	 * listed take the default the operator chooses: independent (coherence 0) or fully coherent
	 * (coherence 1, phase 0). One pair at a time is drawn in the preview.
	 */
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import { livePairs, type PairDraft, type PairValueDraft } from './mimoSetup';
	import type { MimoState } from './mimoStore.svelte';

	interface Props {
		mimo: MimoState;
		labels: string[];
	}
	let { mimo, labels }: Props = $props();

	const r = $derived(mimo.setup.reference);
	const locked = $derived(mimo.locked);
	const E = $derived(mimo.fieldErrors);
	const n = $derived(labels.length);
	const live = $derived(new Set(livePairs(r.pairs, n)));

	function addPair() {
		const taken = new Set(r.pairs.map((p) => `${Math.min(p.row, p.col)}-${Math.max(p.row, p.col)}`));
		for (let i = 0; i < n; i++)
			for (let j = i + 1; j < n; j++)
				if (!taken.has(`${i}-${j}`)) {
					r.pairs.push({ row: i, col: j, coherence: { kind: 'constant', value: 0.5 }, phase: { kind: 'constant', value: 0 } });
					r.previewPair = [i, j];
					return;
				}
	}

	function setKind(v: PairValueDraft, kind: 'constant' | 'table'): PairValueDraft {
		if (v.kind === kind) return v;
		if (kind === 'table') return { kind: 'table', points: [{ freq_hz: 20, value: v.kind === 'constant' ? v.value : 0 }, { freq_hz: 2000, value: v.kind === 'constant' ? v.value : 0 }] };
		return { kind: 'constant', value: v.kind === 'table' ? (v.points[0]?.value ?? 0) : 0 };
	}

	const isPreview = (p: PairDraft) => (r.previewPair[0] === p.row && r.previewPair[1] === p.col) || (r.previewPair[0] === p.col && r.previewPair[1] === p.row);
	const errorsOf = (i: number, what: string) => E[`reference.pairs.${i}.${what}`] ?? [];
	const liveIndex = (p: PairDraft) => [...live].indexOf(p);
</script>

<section class="aq-panel" aria-labelledby="pt-title" data-testid="pair-table">
	<div class="aq-head">
		<h2 id="pt-title">Coherence and Phase</h2>
		<span class="aq-sub">Squared coherence from 0 to 1; phase of the first against the second, in degrees.</span>
	</div>

	<div class="default">
		<span class="lbl" id="pt-default">Pairs not listed</span>
		<div class="aq-seg sm" role="radiogroup" aria-labelledby="pt-default">
			<button type="button" role="radio" aria-checked={r.unspecified === 'independent'} disabled={locked} onclick={() => (r.unspecified = 'independent')} data-testid="pairs-independent">Independent</button>
			<button type="button" role="radio" aria-checked={r.unspecified === 'fully_coherent'} disabled={locked} onclick={() => (r.unspecified = 'fully_coherent')} data-testid="pairs-coherent">Fully coherent</button>
		</div>
		<span class="aq-muted aq-small">{r.unspecified === 'independent' ? 'Coherence 0: the quantities move independently.' : 'Coherence 1 and phase 0: the quantities move together.'}</span>
	</div>

	{#if n < 2}
		<p class="aq-muted aq-small">Pairs need at least two controlled quantities.</p>
	{:else}
		{#if r.pairs.length}
			<div class="aq-scroll">
				<table class="aq-table pairs">
					<thead>
						<tr>
							<th>Pair</th>
							<th>Coherence</th>
							<th>Phase</th>
							<th>Preview</th>
							<th><span class="visually-hidden">Remove</span></th>
						</tr>
					</thead>
					<tbody>
						{#each r.pairs as p, i (p)}
							{@const ok = live.has(p)}
							{@const li = liveIndex(p)}
							<tr data-testid="pair-{i}">
								<td>
									<div class="who">
										<select class="field" aria-label="Pair {i + 1}, first" value={p.row} disabled={locked} onchange={(e) => (p.row = Number(e.currentTarget.value))}>
											{#each labels as l, k (k)}<option value={k}>{l}</option>{/each}
										</select>
										<span class="aq-muted">with</span>
										<select class="field" aria-label="Pair {i + 1}, second" value={p.col} disabled={locked} onchange={(e) => (p.col = Number(e.currentTarget.value))}>
											{#each labels as l, k (k)}<option value={k}>{l}</option>{/each}
										</select>
									</div>
								</td>
								{#each [['coherence', 'Coherence', ''], ['phase', 'Phase', 'deg']] as [what, word, unit] (what)}
									{@const v = what === 'coherence' ? p.coherence : p.phase}
									<td>
										<div class="val">
											<div class="aq-seg sm" role="radiogroup" aria-label="Pair {i + 1} {word.toLowerCase()} given as">
												<button type="button" role="radio" aria-checked={v.kind === 'constant'} disabled={locked} onclick={() => (what === 'coherence' ? (p.coherence = setKind(p.coherence, 'constant')) : (p.phase = setKind(p.phase, 'constant')))}>Constant</button>
												<button type="button" role="radio" aria-checked={v.kind === 'table'} disabled={locked} onclick={() => (what === 'coherence' ? (p.coherence = setKind(p.coherence, 'table')) : (p.phase = setKind(p.phase, 'table')))}>Table</button>
											</div>
											{#if v.kind === 'constant'}
												<NumberField id="pr-{i}-{what}" label="Pair {i + 1} {word.toLowerCase()}" compact {unit} bind:value={v.value} errors={li >= 0 ? errorsOf(li, what) : []} disabled={locked} testid="pair-{i}-{what}" />
											{:else}
												<div class="points">
													{#each v.points as pt, k (pt)}
														<div class="pt">
															<NumberField id="pr-{i}-{what}-{k}-f" label="Pair {i + 1} {word.toLowerCase()} point {k + 1} frequency" compact unit="Hz" bind:value={pt.freq_hz} disabled={locked} />
															<NumberField id="pr-{i}-{what}-{k}-v" label="Pair {i + 1} {word.toLowerCase()} point {k + 1} value" compact {unit} bind:value={pt.value} disabled={locked} />
															<button class="aq-icon-btn" aria-label="Remove point {k + 1}" disabled={locked || v.points.length <= 1} onclick={() => v.points.splice(k, 1)}>
																<svg viewBox="0 0 12 12" width="10" height="10" aria-hidden="true"><path d="M3 3l6 6M9 3l-6 6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
															</button>
														</div>
													{/each}
													<button class="btn sm ghost" disabled={locked} onclick={() => v.points.push({ freq_hz: (v.points[v.points.length - 1]?.freq_hz ?? 100) * 2, value: v.points[v.points.length - 1]?.value ?? 0 })}>Add Point</button>
													<Findings items={(li >= 0 ? errorsOf(li, what) : []).map((message) => ({ level: 'error' as const, message }))} />
												</div>
											{/if}
										</div>
									</td>
								{/each}
								<td>
									<button class="btn sm" class:on={isPreview(p)} aria-pressed={isPreview(p)} disabled={!ok} onclick={() => (r.previewPair = [p.row, p.col])} data-testid="pair-{i}-preview">
										{isPreview(p) ? 'Shown' : 'Show'}
									</button>
								</td>
								<td class="acts">
									<button class="aq-icon-btn" aria-label="Remove pair {i + 1}" disabled={locked} onclick={() => r.pairs.splice(i, 1)}>
										<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M3 3l6 6M9 3l-6 6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
									</button>
								</td>
							</tr>
							{#if !ok}
								<tr class="aq-findrow"><td colspan="5"><Findings items={[{ level: 'warning', message: p.row === p.col ? 'A pair names two different quantities.' : 'This pair is listed already, or names a quantity the test no longer has; it is left out.' }]} /></td></tr>
							{/if}
						{/each}
					</tbody>
				</table>
			</div>
		{/if}
		<div class="aq-actions add">
			<button class="btn sm" disabled={locked || r.pairs.length >= (n * (n - 1)) / 2} onclick={addPair} data-testid="pair-add">Add Pair</button>
			<span class="aq-muted aq-small num">{live.size} of {(n * (n - 1)) / 2} pairs given</span>
		</div>
	{/if}
</section>

<style>
	.default {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 6px var(--sp-3);
		margin-bottom: var(--sp-3);
	}

	.lbl {
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}

	.who,
	.val {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 4px 6px;
	}

	.who .field {
		max-width: 150px;
	}

	.val :global(.nf) {
		min-width: 100px;
	}

	.points {
		display: flex;
		flex-direction: column;
		gap: 3px;
	}

	.pt {
		display: flex;
		align-items: center;
		gap: 4px;
	}

	.pt :global(.nf) {
		min-width: 92px;
	}

	.btn.on {
		font-weight: var(--weight-strong);
		border-color: var(--blue-2);
	}

	.acts {
		width: 1%;
	}

	.add {
		margin-top: var(--sp-2);
		align-items: center;
	}
</style>
