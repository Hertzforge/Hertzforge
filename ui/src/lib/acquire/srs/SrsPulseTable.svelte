<script lang="ts">
	/**
	 * Every pulse fired, newest last: the level it was scheduled and commanded at, the drive played,
	 * its worst band and how far off it was, the bands outside the alarm and the abort band, the
	 * verdict, what the limit channels held back, and the law call that gave its gains. A pulse
	 * picked here (its check box, or a click on its row) is laid over the SRS and Pulse pages.
	 */
	import type { SrsPulseRow } from '$lib/protocol/srs';
	import StatusMark from '$lib/viz/StatusMark.svelte';
	import { formatNumber } from '$lib/viz/format';
	import { db } from '../words';
	import { MAX_OVERLAY } from './srsLayout';
	import { pulseNumber, SRS_LAW_FAULT_WORDS } from './srsWords';

	interface Props {
		pulses: SrsPulseRow[];
		/** The picked pulses, by row index. */
		picked: number[];
		ontoggle: (index: number) => void;
	}
	let { pulses, picked, ontoggle }: Props = $props();

	const n = (v: number | null, sig = 4, unit = '') => (v === null ? '-' : `${formatNumber(v, sig)}${unit}`);
	const worst = (p: SrsPulseRow) => (p.worst_err_db === null ? '-' : `${formatNumber(Math.abs(p.worst_err_db), 2)} dB ${p.worst_err_db < 0 ? 'low' : 'high'}`);
	const limited = $derived(pulses.some((p) => p.limited_components !== null));
	let scroller = $state<HTMLElement | null>(null);

	// The newest pulse comes into view as it arrives, unless the operator scrolled up to read.
	$effect(() => {
		void pulses.length;
		const el = scroller;
		if (!el) return;
		if (el.scrollHeight - el.scrollTop - el.clientHeight < 80) el.scrollTop = el.scrollHeight;
	});
</script>

<div class="wrap" data-testid="srs-pulse-table">
	<p class="aq-small aq-muted lead">Pick up to {MAX_OVERLAY} pulses to lay over the SRS and Pulse pages.</p>
	{#if pulses.length}
		<div class="scroll" bind:this={scroller}>
			<table class="aq-table">
				<thead>
					<tr>
						<th scope="col"><span class="visually-hidden">Overlay</span></th>
						<th scope="col">Pulse</th>
						<th scope="col" class="r">Level</th>
						<th scope="col" class="r">Drive</th>
						<th scope="col" class="r">Worst band</th>
						<th scope="col" class="r">Its error</th>
						<th scope="col" class="r">Outside alarm, abort</th>
						<th scope="col">Result</th>
						<th scope="col" class="r">Peak</th>
						{#if limited}<th scope="col" class="r">Held by limits</th>{/if}
						<th scope="col">Law</th>
					</tr>
				</thead>
				<tbody>
					{#each pulses as p (p.index)}
						{@const on = picked.includes(p.index)}
						<tr class:on class:fail={p.passed === false} data-testid="srs-pulse-{p.index}" data-at={p.at_s} onclick={(e) => (e.target as HTMLElement).tagName !== 'INPUT' && ontoggle(p.index)}>
							<td><input type="checkbox" checked={on} aria-label="Lay pulse {pulseNumber(p.index)} over the plots" onchange={() => ontoggle(p.index)} data-testid="srs-overlay-{p.index}" /></td>
							<td class="num">{pulseNumber(p.index)}</td>
							<td class="num r">{db(p.commanded_db)}{#if Math.abs(p.commanded_db - p.scheduled_db) > 0.05}<span class="sub"> of {db(p.scheduled_db)}</span>{/if}</td>
							<td class="num r">{n(p.drive_vpk, 4, ' V pk')}{#if p.limit_scale < 1}<span class="sub"> scaled</span>{/if}</td>
							<td class="num r">{n(p.worst_band_hz, 4, ' Hz')}</td>
							<td class="num r">{worst(p)}</td>
							<td class="num r">{p.alarm_bands === null ? '-' : `${p.alarm_bands}, ${p.abort_bands ?? '-'}`}</td>
							<td>{#if p.passed === null}-{:else}<span class="res"><StatusMark status={p.passed ? 'pass' : 'fail'} word={false} />{p.passed ? 'Pass' : 'Fail'}</span>{/if}</td>
							<td class="num r">{n(p.peak_g, 4, ' g')}</td>
							{#if limited}<td class="num r">{p.limited_components ?? '-'}</td>{/if}
							<td class="law">
								{#if !p.law}-
								{:else if p.law.miss}<span class="miss" title={SRS_LAW_FAULT_WORDS[p.law.miss]}>Missed; gains kept</span>
								{:else}{formatNumber(p.law.call_ms, 3)} ms{/if}
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{:else}
		<p class="aq-muted aq-small">No pulse has fired yet.</p>
	{/if}
</div>

<style>
	.wrap {
		display: flex;
		flex-direction: column;
		min-height: 0;
	}

	.lead {
		margin: 0 0 var(--sp-2);
	}

	.scroll {
		flex: 1;
		min-height: 0;
		overflow: auto;
	}

	.r {
		text-align: right;
	}

	tbody tr {
		cursor: pointer;
	}

	tbody tr:hover td {
		background: var(--bg-2);
	}

	tr.on td {
		background: var(--blue-weak);
	}

	tr.fail td:nth-child(8) {
		font-weight: var(--weight-strong);
	}

	.res {
		display: inline-flex;
		align-items: center;
		gap: 4px;
	}

	.sub {
		color: var(--text-3);
		font-size: var(--text-xs);
	}

	.law {
		white-space: nowrap;
	}

	.miss {
		font-weight: var(--weight-strong);
		color: var(--amber-1);
	}
</style>
