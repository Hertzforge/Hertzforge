<script lang="ts">
	/**
	 * One pulse's judgement, control by control: its verdict by mark and word, why it failed by name,
	 * its signed error (over-test positive), its delay, and how far it stood from its scaled and its
	 * absolute abort line. Above, the pulse as a whole: the joint verdict, the global SRS error, the
	 * arrival spread (a warning when it passes the window), any control over its scaled line, the
	 * ladder's step and the service's other warnings, and the law's call.
	 */
	import type { MimoShockPulseRow } from '$lib/protocol/mimoShock';
	import StatusMark from '$lib/viz/StatusMark.svelte';
	import { formatNumber } from '$lib/viz/format';
	import { db } from '../words';
	import { FAIL_WORDS, marginWords, MS_LAW_MISS_WORDS, otherWarnings, overUnder, scaledCrossingWords, signedDb, spreadWarning, stepNoteWords } from './msWords';

	interface Props {
		row: MimoShockPulseRow | null;
		/** The timing window, seconds, for the arrival spread. */
		windowS: number | null;
		/** The pulse shown is the latest. */
		latest: boolean;
		onlatest?: () => void;
	}
	let { row, windowS, latest, onlatest }: Props = $props();

	const crossing = $derived(row ? scaledCrossingWords(row) : null);
	const spread = $derived(row ? spreadWarning(row, windowS) : null);
	const step = $derived(row ? stepNoteWords(row) : null);
	/** The service's own warnings, less the scaled crossings this card already flags. */
	const others = $derived(otherWarnings(row?.warnings ?? [], { spread: !!spread, crossing: !!crossing }));
</script>

<section class="detail" aria-label="Pulse in detail" data-testid="ms-pulse-detail">
	{#if !row}
		<p class="aq-small aq-muted">The latest pulse's judgement, control by control, appears here once a pulse has fired.</p>
	{:else}
		<div class="head">
			<StatusMark status={row.passed ? 'pass' : 'warn'} word={false} />
			<strong data-testid="ms-detail-title">Pulse {row.index + 1} at {db(row.level_db)}: {row.passed ? 'passed' : 'did not pass'}</strong>
			{#if !latest && onlatest}<button class="btn ghost sm" onclick={onlatest}>Show the Latest</button>{/if}
		</div>
		<dl class="nums">
			<div><dt>Worst over-test</dt><dd class="num">{signedDb(row.over_test_db)}</dd></div>
			{#if row.global_srs_error_db !== null}<div><dt>Global SRS error</dt><dd class="num" data-testid="ms-global-error">{formatNumber(row.global_srs_error_db, 3)} dB</dd></div>{/if}
			<div><dt>Arrival spread</dt><dd class="num">{formatNumber(row.arrival_spread_s * 1000, 3)} ms</dd></div>
			<div><dt>Law</dt><dd>{row.law.used ? `${formatNumber(row.law.call_ms, 3)} ms` : `not used: ${row.law.miss ? MS_LAW_MISS_WORDS[row.law.miss] : 'missed'}; the last drives played again`}</dd></div>
		</dl>
		{#if crossing}
			<p class="flag" role="alert" data-testid="ms-scaled-flag">
				<svg viewBox="0 0 14 14" width="13" height="13" aria-hidden="true"><path d="M2.5 13V1.5M2.5 2h8l-2 3 2 3h-8" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round" /></svg>
				{crossing}
			</p>
		{/if}
		{#if spread}<p class="warn" data-testid="ms-spread-warning"><StatusMark status="warn" word={false} />{spread}</p>{/if}
		{#if step}<p class="aq-small" data-testid="ms-detail-step">{step}</p>{/if}
		{#each others as w, i (i)}<p class="warn"><StatusMark status="warn" word={false} />{w}</p>{/each}

		<div class="scroll">
			<table class="aq-table" data-testid="ms-detail-table">
				<thead>
					<tr><th>Control</th><th>Verdict</th><th>Failed on</th><th class="num">Error</th><th class="num">Delay</th><th class="num">Scaled line</th><th class="num">Absolute line</th></tr>
				</thead>
				<tbody>
					{#each row.controls as c (c.channel)}
						<tr data-testid="ms-detail-{c.channel}" data-verdict={c.passed ? 'pass' : 'fail'}>
							<td>{c.channel}</td>
							<td><StatusMark status={c.passed ? 'pass' : 'fail'} word={false} /> {c.passed ? 'Pass' : 'Fail'}</td>
							<td data-testid="ms-detail-reasons-{c.channel}">{c.reasons.map((r) => FAIL_WORDS[r]?.word ?? r).join(', ')}</td>
							<td class="num" title={overUnder(c.over_test_db)}>{signedDb(c.over_test_db)} <span class="sub">{overUnder(c.over_test_db)}</span></td>
							<td class="num">{formatNumber(c.delay_s * 1000, 3)} ms</td>
							<td class="num" class:over={c.scaled_margin_db > 0} data-testid="ms-detail-scaled-{c.channel}">{marginWords(c.scaled_margin_db)}</td>
							<td class="num" class:over={c.absolute_margin_db > 0} data-testid="ms-detail-absolute-{c.channel}">{marginWords(c.absolute_margin_db)}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
</section>

<style>
	.detail {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		min-width: 0;
	}

	p {
		margin: 0;
	}

	.head {
		display: flex;
		align-items: center;
		gap: 6px;
		font-size: var(--text-sm);
	}

	.head .btn {
		margin-left: auto;
	}

	.nums {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: 4px var(--sp-3);
		margin: 0;
	}

	.nums dt {
		font-size: var(--text-2xs);
		color: var(--text-3);
	}

	.nums dd {
		margin: 0;
		font-size: var(--text-sm);
	}

	.flag,
	.warn {
		display: flex;
		align-items: center;
		gap: 6px;
		font-size: var(--text-sm);
	}

	.flag {
		font-weight: var(--weight-strong);
		color: var(--amber-1);
	}

	.scroll {
		overflow-x: auto;
	}

	.sub {
		font-size: var(--text-2xs);
		color: var(--text-3);
	}

	td.over {
		font-weight: var(--weight-strong);
		text-decoration: underline;
	}
</style>
