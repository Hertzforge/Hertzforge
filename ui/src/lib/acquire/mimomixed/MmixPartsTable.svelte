<script lang="ts">
	/**
	 * One parts table: the tones, or the narrowbands, on the floor. A row is one part: its id and
	 * name, whether it starts on, how it moves (stands still, sweeps, or follows another part at a
	 * ratio) with that motion's fields, and its window in parts time. Under the row, a line per
	 * control: a tone's peak amplitude and phase at that control; a narrowband's statement there,
	 * a raise of the floor in dB or a density added to it (the added form has one coherence between
	 * the controls). The phases can be left to the tone law. A finding the editor or the service has
	 * on a part shows under its row, named.
	 */
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import { under } from '../setup';
	import { MIMO_MIXED_MAX_BANDS, MIMO_MIXED_MAX_TONES } from '$lib/protocol/mimoMixed';
	import type { ScheduleDraft } from '../mixed/mixedSetup';
	import type { MmixBandDraft, MmixToneDraft } from './mmixSetup';
	import type { MimoMixedStore } from './mmixStore.svelte';

	interface Props {
		mm: MimoMixedStore;
		table: 'tones' | 'bands';
	}
	let { mm, table }: Props = $props();

	const tones = $derived(table === 'tones');
	const d = $derived(mm.setup);
	const rows = $derived((tones ? d.tones : d.bands) as (MmixToneDraft | MmixBandDraft)[]);
	const locked = $derived(mm.locked);
	const E = $derived(mm.fieldErrors);
	const W = $derived(mm.fieldWarnings);
	const labels = $derived(mm.controlLabels);
	const max = $derived(tones ? MIMO_MIXED_MAX_TONES : MIMO_MIXED_MAX_BANDS);
	let open = $state<Record<string, boolean>>({});

	const key = (i: number) => `${table}.${i}`;
	const findings = (i: number) => [...under(E, key(i)).map((message) => ({ level: 'error' as const, message })), ...under(W, key(i)).map((message) => ({ level: 'warning' as const, message }))];
	const leaders = (self: string) => [...d.tones, ...d.bands].filter((p) => p.id !== self);

	function kind(s: ScheduleDraft, k: ScheduleDraft['kind']) {
		s.kind = k;
		if (k === 'sweep' && !(s.band_hz[1] > s.band_hz[0])) s.band_hz = [Math.max(1, s.freq_hz / 2), s.freq_hz * 2];
		if (k === 'follow' && !s.of) s.of = leaders(rows.find((r) => r.schedule === s)?.id ?? '')[0]?.id ?? '';
	}

	function add() {
		if (tones) mm.addTone();
		else mm.addBand();
	}
</script>

<section class="aq-panel parts" aria-labelledby="mmix-{table}-title" data-testid="mmix-{table}">
	<div class="aq-head">
		<h2 id="mmix-{table}-title">{tones ? 'Tones' : 'Narrowbands'}</h2>
		<span class="aq-sub">{tones ? 'Sine tones on the floor, each with a peak amplitude and a phase at every control.' : 'Bands of extra random on the floor, each stated as a raise of the floor or a level added to it, at every control.'}</span>
	</div>
	<Findings items={(E.parts ?? []).map((message) => ({ level: 'error' as const, message }))} />
	{#if rows.length}
		<div class="aq-scroll">
			<table class="aq-table pt" data-testid="mmix-{table}-table">
				<thead>
					<tr>
						<th>Id</th>
						<th>Name</th>
						<th>On</th>
						<th>Moves</th>
						<th>Frequency</th>
						{#if !tones}<th>Width</th>{/if}
						<th>Window</th>
						<th></th>
					</tr>
				</thead>
				<tbody>
					{#each rows as r, i (r.id + ':' + i)}
						{@const s = r.schedule}
						{@const f = findings(i)}
						<tr data-testid="mmix-{table}-row-{i}" data-id={r.id}>
							<td><input class="field sm id" value={r.id} disabled={locked} aria-label="Id, row {i + 1}" aria-invalid={!!E[`${key(i)}.id`]} onchange={(e) => (r.id = e.currentTarget.value.trim())} data-testid="mmix-{table}-{i}-id" /></td>
							<td><input class="field sm name" bind:value={r.name} disabled={locked} aria-label="Name, row {i + 1}" data-testid="mmix-{table}-{i}-name" /></td>
							<td><input type="checkbox" bind:checked={r.enabled} disabled={locked} aria-label="Starts on, row {i + 1}" data-testid="mmix-{table}-{i}-enabled" /></td>
							<td>
								<select class="field sm" value={s.kind} disabled={locked} aria-label="How it moves, row {i + 1}" onchange={(e) => kind(s, e.currentTarget.value as ScheduleDraft['kind'])} data-testid="mmix-{table}-{i}-kind">
									<option value="fixed">Stands still</option>
									<option value="sweep">Sweeps</option>
									<option value="follow">Follows</option>
								</select>
							</td>
							<td class="freq">
								{#if s.kind === 'fixed'}
									<NumberField compact id="mmix-{table}-{i}-f" label="Frequency, row {i + 1}" unit="Hz" min={0} bind:value={s.freq_hz} errors={E[`${key(i)}.schedule.freq_hz`]} disabled={locked} testid="mmix-{table}-{i}-freq" />
								{:else if s.kind === 'sweep'}
									<NumberField compact id="mmix-{table}-{i}-lo" label="From, row {i + 1}" unit="Hz" min={0} bind:value={s.band_hz[0]} disabled={locked} testid="mmix-{table}-{i}-lo" />
									<NumberField compact id="mmix-{table}-{i}-hi" label="To, row {i + 1}" unit="Hz" min={0} bind:value={s.band_hz[1]} disabled={locked} testid="mmix-{table}-{i}-hi" />
									{#if s.law === 'log'}
										<NumberField compact id="mmix-{table}-{i}-rate" label="Rate, row {i + 1}" unit="oct/min" min={0} bind:value={s.oct_per_min} disabled={locked} testid="mmix-{table}-{i}-rate" />
									{:else}
										<NumberField compact id="mmix-{table}-{i}-rate" label="Rate, row {i + 1}" unit="Hz/s" min={0} bind:value={s.hz_per_s} disabled={locked} testid="mmix-{table}-{i}-rate" />
									{/if}
									<select class="field sm law" bind:value={s.law} disabled={locked} aria-label="Sweep law, row {i + 1}" data-testid="mmix-{table}-{i}-law">
										<option value="log">Log</option>
										<option value="linear">Linear</option>
									</select>
								{:else}
									<select class="field sm of" bind:value={s.of} disabled={locked} aria-label="Follows, row {i + 1}" data-testid="mmix-{table}-{i}-of">
										{#if !leaders(r.id).some((p) => p.id === s.of)}<option value={s.of}>{s.of || 'Choose'}</option>{/if}
										{#each leaders(r.id) as p (p.id)}<option value={p.id}>{p.name}</option>{/each}
									</select>
									<span aria-hidden="true">×</span>
									<NumberField compact id="mmix-{table}-{i}-ratio" label="Ratio, row {i + 1}" min={0} step={0.5} bind:value={s.ratio} disabled={locked} testid="mmix-{table}-{i}-ratio" />
								{/if}
							</td>
							{#if 'width' in r}
								<td class="freq">
									{#if r.width.by === 'hz'}
										<NumberField compact id="mmix-{table}-{i}-w" label="Width, row {i + 1}" unit="Hz" min={0} bind:value={r.width.hz} errors={E[`${key(i)}.width`]} disabled={locked} testid="mmix-{table}-{i}-width" />
									{:else}
										<NumberField compact id="mmix-{table}-{i}-w" label="Width, row {i + 1}" min={0} step={0.01} bind:value={r.width.share} errors={E[`${key(i)}.width`]} disabled={locked} testid="mmix-{table}-{i}-width" />
									{/if}
									<select class="field sm" bind:value={r.width.by} disabled={locked} aria-label="Width in, row {i + 1}">
										<option value="hz">Hz</option>
										<option value="share">Share of center</option>
									</select>
								</td>
							{/if}
							<td class="aq-small summary">{r.window.on_s} s to {r.window.off_s === null ? 'the end' : `${r.window.off_s} s`}</td>
							<td class="acts">
								<button class="btn ghost sm" aria-expanded={!!open[r.id]} onclick={() => (open[r.id] = !open[r.id])} data-testid="mmix-{table}-{i}-more">{open[r.id] ? 'Less' : 'More'}</button>
								<button class="btn ghost sm" disabled={locked} onclick={() => mm.removePart(r.id)} aria-label="Remove {r.name}" data-testid="mmix-{table}-{i}-remove">Remove</button>
							</td>
						</tr>
						<tr class="percontrol" data-testid="mmix-{table}-{i}-controls">
							<td></td>
							<td colspan={tones ? 7 : 8}>
								{#if 'amp_g' in r}
									<div class="grid">
										<span class="lbl">Per control</span>
										<label class="aq-check"><input type="checkbox" bind:checked={r.phases_stated} disabled={locked} data-testid="mmix-{table}-{i}-phases" /> Phases stated (else the tone law chooses them)</label>
									</div>
									<table class="aq-table inner">
										<thead><tr><th>Control</th><th class="num">Peak</th><th class="num">Phase</th></tr></thead>
										<tbody>
											{#each labels as l, k (k)}
												<tr>
													<td>{l}</td>
													<td><NumberField compact id="mmix-{table}-{i}-amp-{k}" label="{r.name} at {l}, peak" unit="g pk" min={0} step={0.1} bind:value={r.amp_g[k]} errors={E[`${key(i)}.amp_g.${k}`]} disabled={locked} testid="mmix-{table}-{i}-amp-{k}" /></td>
													<td><NumberField compact id="mmix-{table}-{i}-ph-{k}" label="{r.name} at {l}, phase" unit="deg" step={5} bind:value={r.phase_deg[k]} errors={E[`${key(i)}.phase_deg.${k}`]} disabled={locked || !r.phases_stated || k === 0} testid="mmix-{table}-{i}-ph-{k}" /></td>
												</tr>
											{/each}
										</tbody>
									</table>
								{:else}
									{@const b = r as MmixBandDraft}
									<div class="grid">
										<span class="lbl">Stated as</span>
										<div class="aq-seg sm" role="radiogroup" aria-label="Stated as, row {i + 1}">
											<button type="button" role="radio" aria-checked={b.statement.kind === 'raise'} disabled={locked} onclick={() => (b.statement.kind = 'raise')} data-testid="mmix-{table}-{i}-raise">Raise of the floor</button>
											<button type="button" role="radio" aria-checked={b.statement.kind === 'added'} disabled={locked} onclick={() => (b.statement.kind = 'added')} data-testid="mmix-{table}-{i}-added">Level added</button>
										</div>
										{#if b.statement.kind === 'added'}
											<NumberField compact id="mmix-{table}-{i}-coh" label="Coherence between the controls" step={0.05} min={0} max={1} bind:value={b.statement.coherence} errors={E[`${key(i)}.statement.coherence`]} disabled={locked} testid="mmix-{table}-{i}-coh" />
										{/if}
									</div>
									<table class="aq-table inner">
										<thead><tr><th>Control</th><th class="num">{b.statement.kind === 'raise' ? 'Raise' : 'Added level'}</th></tr></thead>
										<tbody>
											{#each labels as l, k (k)}
												<tr>
													<td>{l}</td>
													<td>
														{#if b.statement.kind === 'raise'}
															<NumberField compact id="mmix-{table}-{i}-db-{k}" label="{b.name} at {l}, raise" unit="dB" step={1} bind:value={b.statement.db[k]} errors={E[`${key(i)}.statement.${k}`]} disabled={locked} testid="mmix-{table}-{i}-db-{k}" />
														{:else}
															<NumberField compact id="mmix-{table}-{i}-lv-{k}" label="{b.name} at {l}, added level" unit="g²/Hz" min={0} bind:value={b.statement.level_g2_hz[k]} errors={E[`${key(i)}.statement.${k}`]} disabled={locked} testid="mmix-{table}-{i}-lv-{k}" />
														{/if}
													</td>
												</tr>
											{/each}
										</tbody>
									</table>
								{/if}
							</td>
						</tr>
						{#if f.length}
							<tr class="findings"><td></td><td colspan="8"><Findings items={f} testid="mmix-{table}-{i}-findings" /></td></tr>
						{/if}
						{#if open[r.id]}
							<tr class="details" data-testid="mmix-{table}-{i}-details">
								<td></td>
								<td colspan="8">
									<div class="aq-fields">
										{#if s.kind === 'sweep'}
											<NumberField id="mmix-{table}-{i}-st" label="Starts at" unit="Hz" nullable="The low end" fallback={s.band_hz[0]} bind:value={s.start_hz} disabled={locked} />
											<div class="sel">
												<span class="lbl">First goes</span>
												<div class="aq-seg sm" role="radiogroup" aria-label="First goes">
													<button type="button" role="radio" aria-checked={s.direction === 'up'} disabled={locked} onclick={() => (s.direction = 'up')}>Up</button>
													<button type="button" role="radio" aria-checked={s.direction === 'down'} disabled={locked} onclick={() => (s.direction = 'down')}>Down</button>
												</div>
											</div>
										{/if}
										<NumberField id="mmix-{table}-{i}-on" label="Comes in at, parts time" unit="s" min={0} bind:value={r.window.on_s} disabled={locked} testid="mmix-{table}-{i}-on" />
										<NumberField id="mmix-{table}-{i}-off" label="Goes out at, parts time" unit="s" nullable="The end" fallback={300} min={0} bind:value={r.window.off_s} disabled={locked} testid="mmix-{table}-{i}-off" />
										{#if 'own_bands' in r}
											<label class="aq-check"><input type="checkbox" bind:checked={r.own_bands} disabled={locked} /> Bands of its own (else the floor's)</label>
										{/if}
										{#if tones || ('own_bands' in r && r.own_bands)}
											<NumberField id="mmix-{table}-{i}-alb" label="Alarm below" unit="dB" min={0} bind:value={r.alarm_db[0]} disabled={locked} />
											<NumberField id="mmix-{table}-{i}-ala" label="Alarm above" unit="dB" min={0} bind:value={r.alarm_db[1]} disabled={locked} />
											<NumberField id="mmix-{table}-{i}-abb" label="Abort below" unit="dB" min={0} bind:value={r.abort_db[0]} disabled={locked} />
											<NumberField id="mmix-{table}-{i}-aba" label="Abort above" unit="dB" min={0} bind:value={r.abort_db[1]} disabled={locked} />
										{/if}
									</div>
								</td>
							</tr>
						{/if}
					{/each}
				</tbody>
			</table>
		</div>
	{:else}
		<p class="aq-small aq-muted" data-testid="mmix-{table}-empty">No {tones ? 'tones' : 'narrowbands'}.</p>
	{/if}
	<div class="aq-actions">
		<button class="btn sm" disabled={locked || rows.length >= max} onclick={add} data-testid="mmix-{table}-add">Add {tones ? 'Tone' : 'Narrowband'}</button>
		<span class="aq-small aq-muted">{rows.length} of at most {max}</span>
	</div>
</section>

<style>
	.parts {
		min-width: 0;
	}

	.pt td {
		vertical-align: middle;
	}

	.pt :global(input.id) {
		width: 4.5em;
	}

	.pt :global(input.name) {
		width: 9em;
	}

	.freq {
		white-space: nowrap;
	}

	.pt :global(.nf.compact) {
		width: 7.5em;
	}

	.summary {
		color: var(--text-3);
		white-space: nowrap;
	}

	.acts {
		white-space: nowrap;
	}

	.percontrol td,
	.findings td,
	.details td {
		border-top: none;
	}

	.grid {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2) var(--sp-3);
		margin-bottom: 4px;
	}

	.inner {
		width: auto;
	}

	.lbl {
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}

	.sel {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}
</style>
