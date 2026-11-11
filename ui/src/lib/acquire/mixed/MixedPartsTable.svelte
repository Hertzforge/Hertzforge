<script lang="ts">
	/**
	 * One parts table: the tones, or the narrowbands. A row is one part: its id and name, whether it
	 * starts on, how it moves (stands still, sweeps, or follows another part at a ratio) with the
	 * fields of that motion, its amplitude (a tone: constant in g peak, or a Sine profile) or its
	 * level and width (a narrowband), its alarm and abort bands, and its window in parts time. A
	 * row's details open under it for the rest (a dwell's end, a sweep's start, direction and end, a
	 * tone's amplitude profile, a band's own bands).
	 *
	 * Rows are picked by their number as in the channel table: a click picks one, Shift a run, Cmd or
	 * Ctrl one more. With several picked, an edit to any of them is made to all of them (the id and
	 * the name stay each part's own). A finding the editor or the service has on a part shows in one
	 * line under its row, and the cell it names is marked.
	 */
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import SineProfileTable from '../sine/SineProfileTable.svelte';
	import TableFileBar from '../TableFileBar.svelte';
	import type { LoadedTable } from '$lib/tables/tableFiles';
	import { PART_TABLE_KIND, partsRows, partsTable } from './partsFile';
	import { NO_SELECTION, isSelected, pick, pickHow, reach, within, type RowSelection } from '$lib/tables/selection';
	import { copyField, followChoices, type MixedBandDraft, type MixedToneDraft, type ScheduleDraft } from './mixedSetup';
	import type { MixedStore } from './mixedStore.svelte';
	import { MIXED_MAX_BANDS, MIXED_MAX_TONES } from '$lib/protocol/mixed';

	interface Props {
		mixed: MixedStore;
		table: 'tones' | 'bands';
	}
	let { mixed, table }: Props = $props();

	const tones = $derived(table === 'tones');
	const rows = $derived((tones ? mixed.setup.tones : mixed.setup.bands) as (MixedToneDraft | MixedBandDraft)[]);
	const locked = $derived(mixed.locked);
	const E = $derived(mixed.fieldErrors);
	const W = $derived(mixed.fieldWarnings);
	const max = $derived(tones ? MIXED_MAX_TONES : MIXED_MAX_BANDS);
	const word = $derived(tones ? 'tone' : 'narrowband');

	let sel = $state<RowSelection>(NO_SELECTION);
	let open = $state<Record<string, boolean>>({});
	$effect(() => {
		const n = rows.length;
		if (sel.rows.some((r) => r >= n)) sel = within(sel, n);
	});

	const key = (i: number, col: string) => `${table}.${i}.${col}`;
	const bad = (i: number, col: string) => (E[key(i, col)] ?? []).length > 0;
	/** Every finding on the row, as one plain line each, errors first. */
	function rowFindings(i: number) {
		const out: { level: 'error' | 'warning'; message: string }[] = [];
		for (const [k, v] of Object.entries(E)) if (k === `${table}.${i}` || k.startsWith(`${table}.${i}.`)) for (const m of v) out.push({ level: 'error', message: m });
		for (const [k, v] of Object.entries(W)) if (k.startsWith(`${table}.${i}.`)) for (const m of v) out.push({ level: 'warning', message: m });
		return out;
	}

	/** Set a field on row `i`, and on every picked row when `i` is one of several picked. */
	function set(i: number, path: string, apply: (r: MixedToneDraft | MixedBandDraft) => void) {
		if (locked) return;
		apply(rows[i]);
		const targets = reach(sel, i);
		if (targets.length > 1) copyField(rows, targets, i, path);
	}
	const num = (v: number | null) => v ?? NaN;

	function click(i: number, e: MouseEvent) {
		sel = pick(sel, i, pickHow(e));
	}

	function add() {
		if (tones) mixed.addTone();
		else mixed.addBand();
	}

	function removePicked() {
		const list = sel.rows.length ? sel.rows : [];
		if (!list.length) return;
		mixed.removeRows(table, list);
		sel = NO_SELECTION;
	}

	function schedKind(i: number, k: ScheduleDraft['kind']) {
		set(i, 'schedule.kind', (r) => {
			r.schedule.kind = k;
			if (k === 'follow' && !r.schedule.of) r.schedule.of = followChoices(mixed.setup, r.id)[0]?.id ?? '';
		});
	}

	const pickedCount = $derived(sel.rows.length);

	/** A file's parts take the table's place; more than the table holds is refused. */
	function openFile(loaded: LoadedTable) {
		const { rows: read, notes } = partsRows(table, loaded);
		if (read.length > max) throw new Error(`The file has ${read.length} parts; a table holds at most ${max}.`);
		if (tones) mixed.setup.tones = read as MixedToneDraft[];
		else mixed.setup.bands = read as MixedBandDraft[];
		sel = NO_SELECTION;
		open = {};
		return notes;
	}
</script>

<section class="aq-panel parts" aria-labelledby="{table}-title" data-testid="mixed-{table}">
	<div class="aq-head">
		<h2 id="{table}-title">{tones ? 'Tones' : 'Narrowbands'}</h2>
		<span class="aq-sub">{tones ? 'Sines of a stated peak amplitude, each standing still, sweeping, or following another part.' : 'Random of a stated density added over the floor across a width, each standing still, sweeping, or following another part.'}</span>
	</div>

	<Findings items={(E[table] ?? []).map((message) => ({ level: 'error' as const, message }))} />

	{#if pickedCount > 1}
		<div class="strip" role="status" data-testid="mixed-{table}-selection">
			<span>{pickedCount} rows picked: an edit to one is made to all of them.</span>
			<button class="btn sm" disabled={locked} onclick={() => sel.rows.forEach((i) => (rows[i].enabled = true))} data-testid="mixed-{table}-selection-on">On</button>
			<button class="btn sm" disabled={locked} onclick={() => sel.rows.forEach((i) => (rows[i].enabled = false))} data-testid="mixed-{table}-selection-off">Off</button>
			<button class="btn ghost sm" onclick={() => (sel = NO_SELECTION)}>Clear Selection</button>
		</div>
	{/if}

	<div class="aq-scroll">
		<table class="aq-table pt" data-testid="mixed-{table}-table">
			<thead>
				<tr>
					<th class="num">#</th>
					<th>Id</th>
					<th>Name</th>
					<th>On</th>
					<th>Moves</th>
					<th>Frequency</th>
					{#if tones}
						<th>Amplitude</th>
					{:else}
						<th class="num">Level</th>
						<th>Width</th>
					{/if}
					<th>Bands, window</th>
					<th></th>
				</tr>
			</thead>
			<tbody>
				{#each rows as r, i (r.id + ':' + i)}
					{@const s = r.schedule}
					{@const f = rowFindings(i)}
					<tr class:picked={isSelected(sel, i)} data-testid="mixed-{table}-row-{i}" data-id={r.id}>
						<td class="num">
							<button class="rownum" aria-pressed={isSelected(sel, i)} aria-label="Pick row {i + 1}" onclick={(e) => click(i, e)} data-testid="mixed-{table}-pick-{i}">{i + 1}</button>
						</td>
						<td><input class="field sm id" value={r.id} disabled={locked} aria-label="Id, row {i + 1}" aria-invalid={bad(i, 'id')} onchange={(e) => (r.id = e.currentTarget.value.trim())} data-testid="mixed-{table}-{i}-id" /></td>
						<td><input class="field sm name" value={r.name} disabled={locked} aria-label="Name, row {i + 1}" aria-invalid={bad(i, 'name')} onchange={(e) => (r.name = e.currentTarget.value)} data-testid="mixed-{table}-{i}-name" /></td>
						<td><input type="checkbox" checked={r.enabled} disabled={locked} aria-label="Starts on, row {i + 1}" onchange={(e) => set(i, 'enabled', (x) => (x.enabled = e.currentTarget.checked))} data-testid="mixed-{table}-{i}-enabled" /></td>
						<td>
							<select class="field sm" value={s.kind} disabled={locked} aria-label="How it moves, row {i + 1}" aria-invalid={bad(i, 'schedule')} onchange={(e) => schedKind(i, e.currentTarget.value as ScheduleDraft['kind'])} data-testid="mixed-{table}-{i}-kind">
								<option value="fixed">Stands still</option>
								<option value="sweep">Sweeps</option>
								<option value="follow">Follows</option>
							</select>
						</td>
						<td class="freq">
							{#if s.kind === 'fixed'}
								<NumberField compact id="{table}-{i}-f" label="Frequency, row {i + 1}" unit="Hz" value={s.freq_hz} min={0} onchange={(v) => set(i, 'schedule.freq_hz', (x) => (x.schedule.freq_hz = num(v)))} disabled={locked} testid="mixed-{table}-{i}-freq" />
							{:else if s.kind === 'sweep'}
								<NumberField compact id="{table}-{i}-lo" label="From, row {i + 1}" unit="Hz" value={s.band_hz[0]} min={0} onchange={(v) => set(i, 'schedule.band_hz', (x) => (x.schedule.band_hz = [num(v), x.schedule.band_hz[1]]))} disabled={locked} testid="mixed-{table}-{i}-lo" />
								<NumberField compact id="{table}-{i}-hi" label="To, row {i + 1}" unit="Hz" value={s.band_hz[1]} min={0} onchange={(v) => set(i, 'schedule.band_hz', (x) => (x.schedule.band_hz = [x.schedule.band_hz[0], num(v)]))} disabled={locked} testid="mixed-{table}-{i}-hi" />
								<NumberField compact id="{table}-{i}-rate" label="Rate, row {i + 1}" unit={s.law === 'log' ? 'oct/min' : 'Hz/s'} value={s.law === 'log' ? s.oct_per_min : s.hz_per_s} min={0} onchange={(v) => set(i, s.law === 'log' ? 'schedule.oct_per_min' : 'schedule.hz_per_s', (x) => (s.law === 'log' ? (x.schedule.oct_per_min = num(v)) : (x.schedule.hz_per_s = num(v))))} disabled={locked} testid="mixed-{table}-{i}-rate" />
								<select class="field sm law" value={s.law} disabled={locked} aria-label="Sweep law, row {i + 1}" onchange={(e) => set(i, 'schedule.law', (x) => (x.schedule.law = e.currentTarget.value as 'log' | 'linear'))} data-testid="mixed-{table}-{i}-law">
									<option value="log">Log</option>
									<option value="linear">Linear</option>
								</select>
							{:else}
								<select class="field sm of" value={s.of} disabled={locked} aria-label="Follows, row {i + 1}" aria-invalid={bad(i, 'schedule')} onchange={(e) => set(i, 'schedule.of', (x) => (x.schedule.of = e.currentTarget.value))} data-testid="mixed-{table}-{i}-of">
									{#if !followChoices(mixed.setup, r.id).some((c) => c.id === s.of)}<option value={s.of}>{s.of || 'Choose'}</option>{/if}
									{#each followChoices(mixed.setup, r.id) as c (c.id)}<option value={c.id}>{c.name}</option>{/each}
								</select>
								<span class="times" aria-hidden="true">×</span>
								<NumberField compact id="{table}-{i}-ratio" label="Ratio, row {i + 1}" value={s.ratio} min={0} step={0.5} onchange={(v) => set(i, 'schedule.ratio', (x) => (x.schedule.ratio = num(v)))} disabled={locked} testid="mixed-{table}-{i}-ratio" />
							{/if}
						</td>
						{#if 'amplitude' in r}
							{@const t = r as MixedToneDraft}
							<td class="amp">
								{#if t.amplitude.by === 'constant'}
									<NumberField compact id="{table}-{i}-g" label="Amplitude, row {i + 1}" unit="g pk" value={t.amplitude.g} min={0} errors={E[key(i, 'amplitude')] ?? []} onchange={(v) => set(i, 'amplitude.g', (x) => ((x as MixedToneDraft).amplitude.g = num(v)))} disabled={locked} testid="mixed-{table}-{i}-amp" />
								{:else}
									<span class="aq-small">Profile, {t.amplitude.profile.rows.length} rows</span>
								{/if}
								<button class="btn ghost sm" disabled={locked} onclick={() => set(i, 'amplitude.by', (x) => ((x as MixedToneDraft).amplitude.by = t.amplitude.by === 'constant' ? 'profile' : 'constant'))} data-testid="mixed-{table}-{i}-ampby">{t.amplitude.by === 'constant' ? 'Use a Profile' : 'Use a Constant'}</button>
							</td>
						{:else}
							{@const b = r as MixedBandDraft}
							<td><NumberField compact id="{table}-{i}-lv" label="Level, row {i + 1}" unit="g²/Hz" value={b.level_g2_hz} min={0} errors={E[key(i, 'level')] ?? []} onchange={(v) => set(i, 'level_g2_hz', (x) => ((x as MixedBandDraft).level_g2_hz = num(v)))} disabled={locked} testid="mixed-{table}-{i}-level" /></td>
							<td class="width">
								<NumberField compact id="{table}-{i}-w" label="Width, row {i + 1}" unit={b.width.by === 'hz' ? 'Hz' : 'of center'} value={b.width.by === 'hz' ? b.width.hz : b.width.share} min={0} errors={E[key(i, 'width')] ?? []} onchange={(v) => set(i, b.width.by === 'hz' ? 'width.hz' : 'width.share', (x) => ((x as MixedBandDraft).width[b.width.by === 'hz' ? 'hz' : 'share'] = num(v)))} disabled={locked} testid="mixed-{table}-{i}-width" />
								<select class="field sm" value={b.width.by} disabled={locked} aria-label="Width in, row {i + 1}" onchange={(e) => set(i, 'width.by', (x) => ((x as MixedBandDraft).width.by = e.currentTarget.value as 'hz' | 'share'))} data-testid="mixed-{table}-{i}-widthby">
									<option value="hz">Hz</option>
									<option value="share">Share of center</option>
								</select>
							</td>
						{/if}
						<td class="aq-small summary" data-testid="mixed-{table}-{i}-folded">{!tones && !(r as MixedBandDraft).own_bands ? "Floor's bands" : `±${r.alarm_db[1]} / ±${r.abort_db[1]} dB`}; {r.window.on_s} s to {r.window.off_s === null ? 'the end' : `${r.window.off_s} s`}</td>
						<td>
							<button class="btn ghost sm" aria-expanded={!!open[r.id]} onclick={() => (open[r.id] = !open[r.id])} data-testid="mixed-{table}-{i}-more">{open[r.id] ? 'Less' : 'More'}</button>
						</td>
					</tr>
					{#if f.length}
						<tr class="findings"><td></td><td colspan="10"><Findings items={f} /></td></tr>
					{/if}
					{#if open[r.id]}
						<tr class="details" data-testid="mixed-{table}-{i}-details">
							<td></td>
							<td colspan="10">
								<div class="aq-fields">
									{#if s.kind === 'fixed'}
										<div class="sel">
											<span class="lbl">Stands for</span>
											<div class="aq-seg sm" role="radiogroup" aria-label="Stands for">
												{#each [['none', 'Its whole window'], ['seconds', 'Seconds'], ['cycles', 'Cycles']] as [v, w] (v)}
													<button type="button" role="radio" aria-checked={s.dwell.by === v} disabled={locked} onclick={() => set(i, 'schedule.dwell', (x) => (x.schedule.dwell.by = v as ScheduleDraft['dwell']['by']))}>{w}</button>
												{/each}
											</div>
										</div>
										{#if s.dwell.by === 'seconds'}<NumberField id="{table}-{i}-ds" label="Seconds of parts time" unit="s" min={0} value={s.dwell.seconds} onchange={(v) => set(i, 'schedule.dwell', (x) => (x.schedule.dwell.seconds = num(v)))} disabled={locked} />{/if}
										{#if s.dwell.by === 'cycles'}<NumberField id="{table}-{i}-dc" label="Cycles" integer min={1} value={s.dwell.cycles} onchange={(v) => set(i, 'schedule.dwell', (x) => (x.schedule.dwell.cycles = num(v)))} disabled={locked} />{/if}
									{:else if s.kind === 'sweep'}
										<NumberField id="{table}-{i}-st" label="Starts at" unit="Hz" nullable="The low end" fallback={s.band_hz[0]} value={s.start_hz} onchange={(v) => set(i, 'schedule.start_hz', (x) => (x.schedule.start_hz = v))} disabled={locked} testid="mixed-{table}-{i}-start" />
										<div class="sel">
											<span class="lbl">First goes</span>
											<div class="aq-seg sm" role="radiogroup" aria-label="First goes">
												<button type="button" role="radio" aria-checked={s.direction === 'up'} disabled={locked} onclick={() => set(i, 'schedule.direction', (x) => (x.schedule.direction = 'up'))}>Up</button>
												<button type="button" role="radio" aria-checked={s.direction === 'down'} disabled={locked} onclick={() => set(i, 'schedule.direction', (x) => (x.schedule.direction = 'down'))}>Down</button>
											</div>
										</div>
										<div class="sel">
											<span class="lbl">Sweeps until</span>
											<div class="aq-seg sm" role="radiogroup" aria-label="Sweeps until">
												{#each [['none', 'Its window closes'], ['sweeps', 'A count'], ['duration', 'A time'], ['both', 'Whichever first']] as [v, w] (v)}
													<button type="button" role="radio" aria-checked={s.end_by === v} disabled={locked} onclick={() => set(i, 'schedule.end_by', (x) => (x.schedule.end_by = v as ScheduleDraft['end_by']))} data-testid="mixed-{table}-{i}-end-{v}">{w}</button>
												{/each}
											</div>
										</div>
										{#if s.end_by === 'sweeps' || s.end_by === 'both'}<NumberField id="{table}-{i}-sw" label="One-way sweeps" integer min={1} value={s.sweeps} onchange={(v) => set(i, 'schedule.sweeps', (x) => (x.schedule.sweeps = num(v)))} disabled={locked} />{/if}
										{#if s.end_by === 'duration' || s.end_by === 'both'}<NumberField id="{table}-{i}-du" label="Seconds of parts time" unit="s" min={0} value={s.duration_s} onchange={(v) => set(i, 'schedule.duration_s', (x) => (x.schedule.duration_s = num(v)))} disabled={locked} />{/if}
									{:else}
										<p class="aq-small aq-muted">It reads its leader's frequency at the same moment, through the leader's turns and holds; it plays while its own window is open.</p>
									{/if}
									<NumberField id="{table}-{i}-on" label="Comes in at, parts time" unit="s" min={0} value={r.window.on_s} onchange={(v) => set(i, 'window.on_s', (x) => (x.window.on_s = num(v)))} disabled={locked} testid="mixed-{table}-{i}-on" />
									<NumberField id="{table}-{i}-off" label="Goes out at, parts time" unit="s" nullable="The end" fallback={300} min={0} value={r.window.off_s} onchange={(v) => set(i, 'window.off_s', (x) => (x.window.off_s = v))} disabled={locked} testid="mixed-{table}-{i}-off" />
									<NumberField id="{table}-{i}-alb" label="Alarm below" unit="dB" min={0} value={r.alarm_db[0]} onchange={(v) => set(i, 'alarm_db', (x) => (x.alarm_db = [num(v), x.alarm_db[1]]))} disabled={locked || (!tones && !(r as MixedBandDraft).own_bands)} />
									<NumberField id="{table}-{i}-ala" label="Alarm above" unit="dB" min={0} value={r.alarm_db[1]} onchange={(v) => set(i, 'alarm_db', (x) => (x.alarm_db = [x.alarm_db[0], num(v)]))} disabled={locked || (!tones && !(r as MixedBandDraft).own_bands)} />
									<NumberField id="{table}-{i}-abb" label="Abort below" unit="dB" min={0} value={r.abort_db[0]} onchange={(v) => set(i, 'abort_db', (x) => (x.abort_db = [num(v), x.abort_db[1]]))} disabled={locked || (!tones && !(r as MixedBandDraft).own_bands)} />
									<NumberField id="{table}-{i}-aba" label="Abort above" unit="dB" min={0} value={r.abort_db[1]} onchange={(v) => set(i, 'abort_db', (x) => (x.abort_db = [x.abort_db[0], num(v)]))} disabled={locked || (!tones && !(r as MixedBandDraft).own_bands)} />
									{#if !tones}
										<label class="check"><input type="checkbox" checked={(r as MixedBandDraft).own_bands} disabled={locked} onchange={(e) => set(i, 'own_bands', (x) => ((x as MixedBandDraft).own_bands = e.currentTarget.checked))} data-testid="mixed-{table}-{i}-ownbands" /> Bands of its own (else the floor's), widened to its own scatter either way</label>
									{/if}
								</div>
								{#if 'amplitude' in r && (r as MixedToneDraft).amplitude.by === 'profile'}
									<SineProfileTable rows={(r as MixedToneDraft).amplitude.profile.rows} prefix={key(i, 'amplitude')} errors={E} {locked} testid="mixed-{table}-{i}-profile" label="Amplitude against frequency" file={{ client: mixed.tableFiles ?? null, tableKind: 'sine_profile', name: `${r.name} Amplitude` }} />
								{/if}
							</td>
						</tr>
					{/if}
				{/each}
			</tbody>
		</table>
	</div>
	{#if !rows.length}<p class="aq-small aq-muted" data-testid="mixed-{table}-empty">No {word}s.</p>{/if}

	<div class="actions">
		<TableFileBar client={mixed.tableFiles ?? null} tableKind={PART_TABLE_KIND[table]} name={tones ? 'Tones' : 'Narrowbands'} table={() => partsTable(table, rows)} onopen={openFile} {locked} testid="mixed-{table}-file">
			<button class="btn sm" disabled={locked || rows.length >= max} onclick={add} data-testid="mixed-{table}-add">Add {tones ? 'Tone' : 'Narrowband'}</button>
			<button class="btn ghost sm" disabled={locked || !pickedCount} onclick={removePicked} data-testid="mixed-{table}-remove">Remove Picked</button>
			<span class="aq-small aq-muted">{rows.length} of at most {max}</span>
		</TableFileBar>
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

	.freq,
	.amp,
	.width {
		white-space: nowrap;
	}

	.pt :global(.nf.compact) {
		width: 7.5em;
	}

	.pt :global(.nf.compact .field) {
		min-width: 0;
		width: 100%;
	}

	.pt select.field {
		min-width: 8.5em;
	}

	.summary {
		color: var(--text-3);
		white-space: nowrap;
	}

	.pt select.of {
		width: 8em;
		min-width: 0;
	}

	.pt select.law {
		min-width: 5.5em;
	}

	.freq :global(> *),
	.amp :global(> *),
	.width :global(> *) {
		display: inline-flex;
		vertical-align: middle;
	}

	.times {
		margin: 0 2px;
		color: var(--text-3);
	}

	.rownum {
		min-width: 2em;
		padding: 0 4px;
		border: 1px solid transparent;
		border-radius: 3px;
		background: none;
		font: inherit;
		color: var(--text-2);
		cursor: pointer;
	}

	.rownum[aria-pressed='true'] {
		border-color: var(--accent-1, var(--blue-1));
		font-weight: var(--weight-strong);
	}

	tr.picked > td {
		background: var(--bg-2, var(--line-1));
	}

	.strip {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		margin-bottom: var(--sp-2);
		font-size: var(--text-xs);
	}

	.actions {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		margin-top: var(--sp-2);
	}

	.details .aq-fields {
		margin-bottom: var(--sp-2);
	}

	.sel {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.lbl {
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}

	.check {
		display: flex;
		align-items: center;
		gap: 6px;
		font-size: var(--text-xs);
	}
</style>
