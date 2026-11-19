<script lang="ts">
	/**
	 * Every pulse fired, newest last: where it came from, the level it was scheduled and commanded
	 * at, the drive played, the measured peak and its error, the velocity change, the three
	 * verdicts, the SRS bands within their band, and the law call that made its drive. A pulse
	 * picked here (its check box, or a click on its row) is laid over the Pulse and SRS pages.
	 */
	import type { ShockPulseRow } from '$lib/protocol/shock';
	import StatusMark from '$lib/viz/StatusMark.svelte';
	import { formatNumber } from '$lib/viz/format';
	import { db } from '../words';
	import { MAX_OVERLAY } from './shockLayout';
	import { pulseNumber, SHOCK_LAW_FAULT_WORDS, verdictWord } from './shockWords';

	interface Props {
		pulses: ShockPulseRow[];
		/** The picked pulses, by row index. */
		picked: number[];
		ontoggle: (index: number) => void;
	}
	let { pulses, picked, ontoggle }: Props = $props();

	const ORIGIN: Record<ShockPulseRow['origin'], string> = { ladder: 'Schedule', train: 'Train', gunfire: 'Gunfire' };
	const n = (v: number | null, sig = 4, unit = '') => (v === null ? '-' : `${formatNumber(v, sig)}${unit}`);
	let scroller = $state<HTMLElement | null>(null);

	// The newest pulse comes into view as it arrives, unless the operator scrolled up to read.
	$effect(() => {
		void pulses.length;
		const el = scroller;
		if (!el) return;
		if (el.scrollHeight - el.scrollTop - el.clientHeight < 80) el.scrollTop = el.scrollHeight;
	});
</script>

<div class="wrap" data-testid="shock-pulse-table">
	<p class="aq-small aq-muted lead">Pick up to {MAX_OVERLAY} pulses to lay over the Pulse and SRS pages.</p>
	{#if pulses.length}
		<div class="scroll" bind:this={scroller}>
			<table class="aq-table">
				<thead>
					<tr>
						<th scope="col"><span class="visually-hidden">Overlay</span></th>
						<th scope="col">Pulse</th>
						<th scope="col">From</th>
						<th scope="col" class="r">Level</th>
						<th scope="col" class="r">Drive</th>
						<th scope="col" class="r">Peak</th>
						<th scope="col" class="r">Error</th>
						<th scope="col" class="r">Velocity change</th>
						<th scope="col">Envelope, velocity, substitution</th>
						<th scope="col">Result</th>
						<th scope="col" class="r">SRS bands</th>
						<th scope="col">Law</th>
					</tr>
				</thead>
				<tbody>
					{#each pulses as p (p.index)}
						{@const on = picked.includes(p.index)}
						<tr class:on class:fail={p.passed === false} data-testid="shock-pulse-{p.index}" onclick={(e) => (e.target as HTMLElement).tagName !== 'INPUT' && ontoggle(p.index)}>
							<td><input type="checkbox" checked={on} aria-label="Lay pulse {pulseNumber(p.index)} over the plots" onchange={() => ontoggle(p.index)} data-testid="shock-overlay-{p.index}" /></td>
							<td class="num">{pulseNumber(p.index)}</td>
							<td>{ORIGIN[p.origin]}</td>
							<td class="num r">{db(p.commanded_db)}{#if Math.abs(p.commanded_db - p.scheduled_db) > 0.05}<span class="sub"> of {db(p.scheduled_db)}</span>{/if}</td>
							<td class="num r">{n(p.drive_vpk, 4, ' V pk')}{#if p.limit_scale < 1}<span class="sub"> scaled</span>{/if}</td>
							<td class="num r">{n(p.peak_g, 4, ' g')}</td>
							<td class="num r">{n(p.error_pct, 3, '%')}</td>
							<td class="num r">{n(p.velocity_change_ips, 4, ' in/s')}</td>
							<td class="verd">
								{#if p.verdicts}
									{#each [p.verdicts.envelope, p.verdicts.velocity, p.verdicts.substitution] as ok, k (k)}<span class="v" title={verdictWord(ok)}><StatusMark status={ok ? 'pass' : 'fail'} word={false} /></span>{/each}
								{:else}-{/if}
							</td>
							<td>{#if p.passed === null}-{:else}<span class="res"><StatusMark status={p.passed ? 'pass' : 'fail'} word={false} />{p.passed ? 'Pass' : 'Fail'}</span>{/if}</td>
							<td class="num r">{p.srs ? `${p.srs.passed_bands} of ${p.srs.bands}` : '-'}</td>
							<td class="law">
								{#if !p.law}In force
								{:else if p.law.miss}<span class="miss" title={SHOCK_LAW_FAULT_WORDS[p.law.miss]}>Missed</span>
								{:else}{formatNumber(p.law.call_ms, 3)} ms{#if p.law.capped_bins > 0 || p.law.peak_capped}<span class="sub">, capped</span>{/if}{/if}
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

	tr.fail td:nth-child(10) {
		font-weight: var(--weight-strong);
	}

	.verd {
		white-space: nowrap;
	}

	.v + .v {
		margin-left: 4px;
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
