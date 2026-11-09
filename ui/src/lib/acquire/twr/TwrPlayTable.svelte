<script lang="ts">
	/**
	 * Every play after identification, newest last: its kind, the level and rung it played at and its
	 * iteration there, the service's judgement (rms, peak and spectral error, and the gate), the drive
	 * and response peaks, its alignment drift, the law call that made its drive and the values the law
	 * reported. A play picked here (its Records button, or a click on its row) opens its target,
	 * response, error and drive records below the table; the service holds the most recent plays'
	 * records, and an older play keeps its row.
	 */
	import type { LawEntry } from '$lib/protocol/mimo';
	import type { TwrPlayRow } from '$lib/protocol/twr';
	import type { View } from '$lib/protocol/types';
	import StatusMark from '$lib/viz/StatusMark.svelte';
	import ViewRenderer from '$lib/viz/ViewRenderer.svelte';
	import { formatNumber } from '$lib/viz/format';
	import { db } from '../words';
	import { driftWords, lawCallWords, PLAY_KIND_WORDS, playNumber } from './twrWords';

	interface Props {
		plays: TwrPlayRow[];
		/** The run's law, for its declared values' labels and units. */
		law: LawEntry | null;
		/** The play whose records are open, and its views. */
		record: { index: number; views: View[] } | null;
		/** The play whose records are being fetched, or null. */
		busy: number | null;
		ontoggle: (index: number) => void;
	}
	let { plays, law, record, busy, ontoggle }: Props = $props();

	const values = $derived(law?.diagnostics ?? []);
	const n = (v: number | null | undefined, sig = 4, unit = '') => (v === null || v === undefined || !Number.isFinite(v) ? '-' : `${formatNumber(v, sig)}${unit}`);
	let scroller = $state<HTMLElement | null>(null);

	// The newest play comes into view as it arrives, unless the operator scrolled up to read.
	$effect(() => {
		void plays.length;
		const el = scroller;
		if (!el) return;
		if (el.scrollHeight - el.scrollTop - el.clientHeight < 80) el.scrollTop = el.scrollHeight;
	});
</script>

<div class="wrap" data-testid="twr-play-table" data-rows={plays.length}>
	{#if plays.length}
		<div class="scroll" bind:this={scroller}>
			<table class="aq-table">
				<thead>
					<tr>
						<th scope="col">Play</th>
						<th scope="col">Kind</th>
						<th scope="col" class="r">Level</th>
						<th scope="col" class="r">Iteration</th>
						<th scope="col" class="r">RMS error</th>
						<th scope="col" class="r">Peak error</th>
						<th scope="col" class="r">Spectral error</th>
						<th scope="col">Gate</th>
						<th scope="col" class="r">Drive</th>
						<th scope="col" class="r">Response</th>
						<th scope="col" class="r">Lag drift</th>
						<th scope="col">Law</th>
						{#each values as d (d.key)}<th scope="col" class="r" title={d.help}>{d.label}{d.unit ? `, ${d.unit}` : ''}</th>{/each}
						<th scope="col"><span class="visually-hidden">Records</span></th>
					</tr>
				</thead>
				<tbody>
					{#each plays as p (p.index)}
						{@const open = record?.index === p.index}
						<tr class:on={open} class:miss={!!p.law && !p.law.used} data-testid="twr-play-{p.index}" onclick={(e) => (e.target as HTMLElement).tagName !== 'BUTTON' && ontoggle(p.index)}>
							<td class="num">{playNumber(p.index)}</td>
							<td>{PLAY_KIND_WORDS[p.kind]}</td>
							<td class="num r">{db(p.level_db)}{#if p.rung !== null}<span class="sub">, rung {p.rung + 1}</span>{/if}</td>
							<td class="num r">{p.iteration ?? '-'}</td>
							<td class="num r">{n(p.rms_error_pct, 3, '%')}</td>
							<td class="num r">{n(p.peak_error_pct, 3, '%')}</td>
							<td class="num r">{n(p.spectral_error_db, 3, ' dB')}</td>
							<td><span class="res"><StatusMark status={p.gated ? 'pass' : 'warn'} word={false} />{p.gated ? 'Met' : 'Not met'}</span></td>
							<td class="num r">{n(p.drive_vpk, 4, ' V pk')}{#if p.clipped_samples > 0}<span class="sub">, {p.clipped_samples} clipped</span>{/if}</td>
							<td class="num r">{n(p.response_peak, 4)}</td>
							<td class="num r" class:unmeasured={p.drift_frames === null}>{driftWords(p)}</td>
							<td class="law">{lawCallWords(p)}{#if p.clamped_bins > 0}<span class="sub">, {p.clamped_bins} bins held</span>{/if}</td>
							{#each values as d (d.key)}<td class="num r">{n(p.law?.values?.[d.key])}</td>{/each}
							<td><button class="btn ghost sm" aria-pressed={open} data-testid="twr-play-records-{p.index}" disabled={busy === p.index} onclick={() => ontoggle(p.index)}>{open ? 'Close' : 'Records'}</button></td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{:else}
		<p class="aq-muted aq-small">No play has been judged yet.</p>
	{/if}
	{#if record}
		<div class="records" data-testid="twr-play-record" data-index={record.index}>
			<p class="aq-small aq-muted">Play {playNumber(record.index)}'s records, from the service.</p>
			{#each record.views as v, i (i)}<div class="plot"><ViewRenderer view={v} minHeight={170} /></div>{/each}
		</div>
	{/if}
</div>

<style>
	.wrap {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		min-height: 0;
	}

	.scroll {
		flex: 1;
		min-height: 0;
		max-height: 420px;
		overflow: auto;
	}

	.r {
		text-align: right;
	}

	tbody tr {
		cursor: pointer;
	}

	th,
	td {
		white-space: nowrap;
	}

	tbody tr:hover td {
		background: var(--bg-2);
	}

	tr.on td {
		background: var(--blue-weak);
	}

	tr.miss td.law {
		font-weight: var(--weight-strong);
		color: var(--amber-1);
	}

	.res {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		white-space: nowrap;
	}

	.sub {
		color: var(--text-3);
		font-size: var(--text-xs);
	}

	.law {
		white-space: nowrap;
	}

	.unmeasured {
		color: var(--text-3);
	}

	.records {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}

	.plot {
		min-width: 0;
	}
</style>
