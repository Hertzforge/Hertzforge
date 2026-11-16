<script lang="ts">
	/**
	 * The drive file: a file chosen by its full path on the computer that runs the test, or time
	 * histories of the open project; what it holds, read by the analysis service; the signals to
	 * play, in drive order; the rate when the file states none; Convert, which writes a new drive
	 * folder the control service reads; and the folder the setup plays, each of its channels mapped
	 * to a drive row of the channel table. The control service checks the folder (the Check
	 * section) and nothing plays that it has not checked.
	 */
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import { playbackRows } from './playbackSetup';
	import type { PlaybackStore } from './playbackStore.svelte';
	import { formatNumber } from '$lib/viz/format';

	interface Props {
		playback: PlaybackStore;
	}
	let { playback }: Props = $props();

	const d = $derived(playback.setup);
	const src = $derived(playback.driveSource);
	const locked = $derived(playback.locked);
	const E = $derived(playback.fieldErrors);
	const errs = (k: string) => E[k] ?? [];
	const drives = $derived(playbackRows(playback.rows).drives);
	const objects = $derived(playback.timeObjects);
	const sidecar = $derived(playback.converted && playback.converted.folder === d.drive.folder ? playback.converted.drive : null);
	const report = $derived(playback.checkReport);
	const names = $derived(sidecar?.channels.map((c) => c.name) ?? report?.channels.map((c) => c.name) ?? []);

	let from = $state<'path' | 'project'>('path');
	let path = $state('');
	let picked = $state<string[]>([]);

	const signalOf = (key: string) => src.inspected?.signals.find((s) => s.key === key) ?? null;
	const n = (v: number, sig = 6) => formatNumber(v, sig);

	function readIt() {
		if (from === 'path') {
			if (path.trim()) void playback.inspect({ path: path.trim() });
		} else if (picked.length) void playback.inspect({ objects: [...picked] });
	}
</script>

<div class="stack" data-testid="playback-drive">
	<section class="aq-panel" aria-labelledby="pb-file">
		<div class="aq-head"><h2 id="pb-file">Drive File</h2></div>
		<p class="aq-lead aq-small lead">A drive is volts at each drive output, one signal a drive, of any length, made by you. The file is read on the computer that runs the test, never uploaded, and written to a drive folder the control service plays. CSV and other delimited text, MATLAB, NumPy, HDF5, Hertzforge projects, universal files (dataset 58), Excel and WAV are read.</p>

		<div class="aq-group">
			<h3>Read a File</h3>
			<div class="aq-seg" role="radiogroup" aria-label="Read the drive from">
				<button type="button" role="radio" aria-checked={from === 'path'} disabled={locked} onclick={() => (from = 'path')} data-testid="pb-from-path">A file</button>
				<button type="button" role="radio" aria-checked={from === 'project'} disabled={locked} onclick={() => (from = 'project')} data-testid="pb-from-project">The project's time histories</button>
			</div>
			{#if from === 'path'}
				<div class="sel spaced">
					<label for="pb-path" class="lbl">File</label>
					<input id="pb-path" class="field mono" placeholder="/full/path/to/drive.csv" bind:value={path} disabled={locked} data-testid="pb-path" onkeydown={(e) => e.key === 'Enter' && readIt()} />
					<p class="help">By its full path on the computer that runs the test.</p>
				</div>
			{:else if objects.length}
				<fieldset class="objs spaced" data-testid="pb-objects">
					<legend class="lbl">Time histories in the open project</legend>
					{#each objects as o (o.name)}
						<label class="aq-check"><input type="checkbox" checked={picked.includes(o.name)} disabled={locked} onchange={(e) => (picked = e.currentTarget.checked ? [...picked, o.name] : picked.filter((x) => x !== o.name))} /> {o.name}{o.records ? ` (${o.records} ${o.records === 1 ? 'record' : 'records'})` : ''}</label>
					{/each}
				</fieldset>
			{:else}
				<p class="aq-small aq-muted spaced">The open project has no time histories. Open or import one in the analysis workspaces, or read a file.</p>
			{/if}
			<div class="row">
				<button class="btn" disabled={locked || !!playback.working || (from === 'path' ? !path.trim() : !picked.length)} onclick={readIt} data-testid="pb-read">{playback.working === 'Read the File' ? 'Reading' : 'Read'}</button>
			</div>
			{#if playback.fileError}<Findings items={[{ level: 'error', message: playback.fileError }]} />{/if}
		</div>

		{#if src.inspected}
			{@const ins = src.inspected}
			<div class="aq-group" data-testid="pb-signals" data-count={ins.signals.length}>
				<h3>Signals</h3>
				<p class="aq-small aq-muted">{ins.source.format === 'project' ? 'From the project' : (ins.source.path ?? '')}, read as {ins.source.format}. Tick the signals to play; they play in the order of the chosen list below, one a drive.</p>
				<table class="aq-table">
					<thead><tr><th>Play</th><th>Signal</th><th>Where</th><th class="r">Samples</th><th class="r">Rate</th><th>Unit</th><th></th></tr></thead>
					<tbody>
						{#each ins.signals as s (s.key)}
							<tr data-testid="pb-signal-{s.key}">
								<td><input type="checkbox" aria-label="Play {s.name}" checked={src.chosen.includes(s.key)} disabled={locked || !!s.problem} onchange={(e) => playback.toggle(s.key, e.currentTarget.checked)} /></td>
								<td>{s.name}</td>
								<td class="aq-muted">{s.where}</td>
								<td class="r num">{s.samples}</td>
								<td class="r num">{s.rate_hz === null ? '-' : `${n(s.rate_hz)} Hz`}</td>
								<td>{s.unit ?? 'not stated (taken as V)'}</td>
								<td class="aq-small">{s.problem ?? ''}</td>
							</tr>
						{/each}
					</tbody>
				</table>
				{#if src.chosen.length}
					<h3 class="spaced">Drive Order</h3>
					<ol class="order" data-testid="pb-order">
						{#each src.chosen as key, i (key)}
							<li>
								<span class="num">{i}</span>
								<span class="nm">{signalOf(key)?.name ?? key}</span>
								<button class="aq-icon-btn" aria-label="Earlier" disabled={locked || i === 0} onclick={() => playback.move(key, -1)}>&#8593;</button>
								<button class="aq-icon-btn" aria-label="Later" disabled={locked || i === src.chosen.length - 1} onclick={() => playback.move(key, 1)}>&#8595;</button>
							</li>
						{/each}
					</ol>
				{/if}
				{#if playback.rateNeeded}
					<div class="aq-fields spaced">
						<NumberField id="pb-rate" label="Rate" unit="Hz" step={1} min={0} bind:value={src.rate_hz} disabled={locked} testid="pb-rate" help="The file states no rate: the rate the drive was made at. The station runs at this rate, and the drive is never resampled." />
					</div>
				{:else if ins.rate_hz !== null}
					<p class="aq-small spaced">The file states {n(ins.rate_hz)} samples a second; the station runs at that rate, and the drive is never resampled.</p>
				{/if}
				{#each ins.notes as note, i (i)}<p class="aq-small aq-muted">{note}</p>{/each}
				<div class="row">
					<button class="btn primary" aria-disabled={!!playback.convertWhy || !!playback.working} class:off={!!playback.convertWhy} title={playback.convertWhy ?? 'Write the chosen signals to a new drive folder.'} onclick={() => void playback.convert()} data-testid="pb-convert">{playback.working === 'Convert' ? 'Converting' : 'Convert'}</button>
					{#if playback.convertWhy}<span class="aq-small aq-muted">{playback.convertWhy}</span>{/if}
				</div>
			</div>
		{/if}
	</section>

	<section class="aq-panel" aria-labelledby="pb-folder" data-testid="pb-folder" data-folder={d.drive.folder} data-sha={d.drive.sha256}>
		<div class="aq-head"><h2 id="pb-folder">Drive to Play</h2></div>
		{#if d.drive.folder}
			<dl class="nums">
				<div class="wide"><dt>Folder</dt><dd class="mono" data-testid="pb-folder-path">{d.drive.folder}</dd></div>
				<div class="wide"><dt>SHA-256 of its samples</dt><dd class="mono sha">{d.drive.sha256}</dd></div>
				{#if sidecar ?? report}
					{@const facts = sidecar ? { rate: sidecar.rate_hz, samples: sidecar.samples } : { rate: report!.rate_hz, samples: report!.samples }}
					<div><dt>Rate</dt><dd class="num">{n(facts.rate)} Hz</dd></div>
					<div><dt>Length</dt><dd class="num">{facts.samples} samples, {n(facts.samples / facts.rate, 4)} s</dd></div>
				{/if}
			</dl>
			{#if sidecar?.fixes.length}<p class="aq-small">Its ends were fixed from another folder; the fix is kept in its sidecar.</p>{/if}
			{#if playback.converted && playback.converted.folder === d.drive.folder}
				{#each playback.converted.notes as note, i (i)}<p class="aq-small aq-muted">{note}</p>{/each}
			{/if}
			<div class="aq-group">
				<h3>Each Channel's Drive Row</h3>
				<table class="aq-table" data-testid="pb-map">
					<thead><tr><th>File channel</th><th>Plays on</th></tr></thead>
					<tbody>
						{#each d.map as m, k (k)}
							<tr>
								<td>{k}{names[k] ? `, ${names[k]}` : ''}</td>
								<td>
									<select class="field" aria-label="Drive row for file channel {k}" disabled={locked} value={m === null ? '' : String(m)} onchange={(e) => (d.map[k] = e.currentTarget.value === '' ? null : Number(e.currentTarget.value))} data-testid="pb-map-{k}">
										<option value="">Choose</option>
										{#each drives as r (r.index)}<option value={String(r.index)}>Row {r.index}{r.description ? `, ${r.description}` : ''} (range {r.range_max} V)</option>{/each}
									</select>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
				<p class="aq-small aq-muted">An enabled drive row no channel plays on is driven at 0 V. Each row once.</p>
				<Findings items={[...errs('map'), ...errs('drive')].map((message) => ({ level: 'error' as const, message }))} />
			</div>
		{:else}
			<p class="aq-small aq-muted">None yet. Read a file and convert it, or open a setup that names a drive folder.</p>
			<Findings items={errs('drive').map((message) => ({ level: 'error' as const, message }))} />
		{/if}
	</section>
</div>

<style>
	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		max-width: 920px;
	}

	.lead {
		margin: 0 0 var(--sp-2);
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

	.help {
		margin: 2px 0 0;
		font-size: var(--text-xs);
		color: var(--text-3);
	}

	.mono {
		font-family: var(--font-mono);
	}

	.sha {
		font-size: var(--text-2xs);
		word-break: break-all;
	}

	.spaced {
		margin-top: var(--sp-3);
	}

	.row {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		margin-top: var(--sp-3);
	}

	.objs {
		display: flex;
		flex-direction: column;
		gap: 4px;
		border: 0;
		padding: 0;
	}

	.r {
		text-align: right;
	}

	.order {
		margin: 0;
		padding: 0;
		list-style: none;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.order li {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		font-size: var(--text-sm);
	}

	.order .nm {
		flex: 1;
	}

	.nums {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: 6px var(--sp-3);
		margin: 0;
	}

	.nums .wide {
		grid-column: 1 / -1;
	}

	.nums dt {
		font-size: var(--text-2xs);
		color: var(--text-3);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	.nums dd {
		margin: 0;
		font-size: var(--text-sm);
	}

	.btn.off {
		opacity: 0.45;
		cursor: default;
	}
</style>
