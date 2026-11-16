<script lang="ts">
	/**
	 * The Playback setup in a few lines, beside every section: the drive folder and the rows it
	 * plays on, the level, the ceiling and the rolls; what the control service made of it (the
	 * file's rate and length, the gain, the run's length, the stop) and its check's verdict; and
	 * what is left to fix, each a click from its section.
	 */
	import { formatNumber } from '$lib/viz/format';
	import { PLAYBACK_SECTIONS, playbackRows, type PlaybackSection } from './playbackSetup';
	import type { PlaybackStore } from './playbackStore.svelte';
	import { folderName } from '../words';

	interface Props {
		playback: PlaybackStore;
		onopen: (s: PlaybackSection) => void;
	}
	let { playback, onopen }: Props = $props();

	const d = $derived(playback.setup);
	const dry = $derived(playback.current);
	const dv = $derived(dry?.derived ?? null);
	const report = $derived(playback.checkReport);
	const todo = $derived(PLAYBACK_SECTIONS.filter((s) => playback.counts[s.id].errors > 0));
	const ch = $derived(playbackRows(playback.rows));
	const n = (v: number, sig = 4) => formatNumber(v, sig);
	const num = (v: number, unit: string, sig = 4) => (Number.isFinite(v) ? `${n(v, sig)} ${unit}` : '-');
</script>

<aside class="aq-panel summary" aria-label="Setup summary" data-testid="playback-summary">
	<h2>Summary</h2>
	<dl>
		<div><dt>Drive</dt><dd data-testid="playback-summary-drive">{d.drive.folder ? folderName(d.drive.folder) : 'None yet'}{d.map.length ? `, ${d.map.length} ${d.map.length === 1 ? 'channel' : 'channels'} on row${d.map.length === 1 ? '' : 's'} ${d.map.map((m) => m ?? '-').join(', ')}` : ''}</dd></div>
		<div><dt>Level and ceiling</dt><dd>{num(d.level_db, 'dB', 3)}, ceiling {num(d.ceiling_v, 'V')}</dd></div>
		<div><dt>Rolls</dt><dd>{num(d.pre_roll_s, 's')} before, {num(d.post_roll_s, 's')} after</dd></div>
		<div><dt>Rows</dt><dd data-testid="playback-summary-rows">Records {ch.inputs.length ? ch.inputs.map((r) => r.index).join(', ') : 'no input'}; drives {ch.drives.length ? ch.drives.map((r) => r.index).join(', ') : 'none'}</dd></div>
	</dl>

	{#if dv}
		<h3>From the Control Service</h3>
		<dl data-testid="playback-summary-derived">
			<div><dt>Rate and length</dt><dd class="num" data-testid="playback-derived-rate">{n(dv.fs_hz, 6)} Hz, {dv.samples} samples, {n(dv.duration_s, 4)} s</dd></div>
			<div><dt>Gain</dt><dd class="num">{n(dv.gain, 4)}</dd></div>
			<div><dt>Run</dt><dd class="num">{n(dv.total_s, 4)} s with the rolls</dd></div>
		</dl>
	{/if}
	{#if report}
		<div><dt class="lab">Drive check</dt><dd class="chk {report.pass ? 'pass' : 'fail'}" data-testid="playback-summary-check">{report.pass ? 'Passes' : `Refused: ${report.refusals.length} ${report.refusals.length === 1 ? 'finding' : 'findings'}`}</dd></div>
	{:else if playback.checking}
		<p class="aq-muted aq-small">Checking the setup.</p>
	{/if}

	<div class="ready" data-testid="playback-ready" data-ready={todo.length === 0 && !!dry && !!report?.pass}>
		{#if todo.length}
			<h3>Left to Fix</h3>
			<ul>
				{#each todo as s (s.id)}
					<li><button class="btn ghost sm" onclick={() => onopen(s.id)} data-testid="playback-todo-{s.id}">{s.label}: {playback.counts[s.id].errors} {playback.counts[s.id].errors === 1 ? 'error' : 'errors'}</button></li>
				{/each}
			</ul>
		{:else if dry && report?.pass}
			<p class="aq-small ok">Nothing is missing: the setup checks clean.</p>
		{/if}
	</div>
</aside>

<style>
	.summary {
		position: sticky;
		top: 0;
	}

	h2 {
		margin-bottom: var(--sp-2);
		font-size: var(--text-md);
		font-weight: var(--weight-strong);
	}

	h3 {
		margin: var(--sp-3) 0 var(--sp-1);
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
	}

	dl {
		display: flex;
		flex-direction: column;
		gap: 6px;
		margin: 0;
	}

	dt,
	.lab {
		margin-top: var(--sp-2);
		font-size: var(--text-2xs);
		color: var(--text-3);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	dd {
		margin: 0;
		font-size: var(--text-sm);
		color: var(--text-1);
	}

	.chk.pass {
		color: var(--blue-1);
	}

	ul {
		margin: 0;
		padding: 0;
		list-style: none;
	}

	.ok {
		margin: var(--sp-3) 0 0;
		color: var(--blue-1);
	}
</style>
