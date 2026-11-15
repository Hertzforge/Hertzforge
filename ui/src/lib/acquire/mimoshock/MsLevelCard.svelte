<script lang="ts">
	/**
	 * The MIMO Shock run at a glance: what it is doing in plain words, the level in force on the
	 * ladder (with its half step before full level, and any step the control service halved, with
	 * its note), each control's reachable floor before the first pulse, the verdict mode, and once
	 * the run ends, how and why in plain words, naming the abort line when one ended it. Every
	 * number is the service's; shape and word carry each state, color only follows.
	 */
	import type { MimoShockPulseRow, MimoShockState } from '$lib/protocol/mimoShock';
	import StatusMark from '$lib/viz/StatusMark.svelte';
	import { formatNumber } from '$lib/viz/format';
	import { clock, db } from '../words';
	import { abortLineOf, endWords, ladderRungs, lastPulseWords, msLevelStatus, msOutcomeTone, MS_OUTCOME_WORDS, rungWord, signedDb, stepNoteWords, VERDICT_WORDS } from './msWords';

	interface Props {
		status: MimoShockState | null;
		rows: MimoShockPulseRow[];
		compact?: boolean;
	}
	let { status, rows, compact = false }: Props = $props();

	const st = $derived(msLevelStatus(status));
	const last = $derived(rows.at(-1) ?? status?.last_pulse ?? null);
	const lastWords = $derived(lastPulseWords(last));
	const rungs = $derived(ladderRungs(status?.levels_db ?? null, rows, status?.level_db ?? null));
	const notes = $derived(rows.map((r) => stepNoteWords(r)).filter((x): x is string => !!x));
	const review = $derived(status?.review ?? null);
	const beforeFirst = $derived(!!review && (status?.pulses.fired ?? 0) === 0);
	const end = $derived(status?.state === 'finished' ? status.end : null);
	const line = $derived(abortLineOf(end));
	const next = $derived(status?.state === 'running' && status.phase === 'waiting' ? (status.next_pulse_s !== null && status.next_pulse_s > 0.05 ? `Next pulse in ${clock(status.next_pulse_s)}` : status.mode === 'manual' ? 'Ready to fire' : null) : null);
</script>

<section class="levelcard" class:compact aria-label="Level" data-testid="ms-level-card">
	<div class="statusline {st.tone}" role="status" data-testid="ms-level-status">
		<StatusMark status={st.tone} word={false} />
		<span class="words">{st.text}</span>
		{#if status?.mode}<span class="mode">{status.mode === 'auto' ? 'Automatic' : 'On Fire'}</span>{/if}
	</div>
	{#if status?.state === 'running' && status.level_db !== null}
		<p class="coming" data-testid="ms-level-now">At {db(status.level_db)}: pulse {status.pulses.at_level + (status.phase === 'firing' ? 0 : 1)} of at most {status.pulses.of_level}</p>
	{/if}
	{#if next}<p class="next">{next}</p>{/if}
	{#if lastWords}
		<p class="last {lastWords.tone}" data-testid="ms-last-pulse"><StatusMark status={lastWords.tone} word={false} />{lastWords.text}</p>
	{/if}

	{#if end}
		<div class="end {msOutcomeTone(end.outcome)}" role={msOutcomeTone(end.outcome) === 'fail' ? 'alert' : 'status'} data-testid="ms-end" data-reason={end.reason ?? ''} data-line={line ?? ''}>
			<p class="head"><StatusMark status={msOutcomeTone(end.outcome)} word={false} /><strong>{MS_OUTCOME_WORDS[end.outcome]}</strong></p>
			<p class="why" data-testid="ms-end-words">{endWords(end)}</p>
			{#if line}<p class="aq-small" data-testid="ms-end-line">Abort line: {line === 'scaled' ? 'scaled to the level played' : 'absolute, at full level'}.</p>{/if}
		</div>
	{/if}

	{#if rungs.length}
		<div class="ladder" data-testid="ms-ladder-card">
			<h3>Ladder</h3>
			<ol>
				{#each rungs as r (r.db)}
					{@const word = rungWord(r, status?.levels_db ?? null)}
					<li class:now={r.now} data-testid="ms-rung" data-db={r.db} data-now={r.now}>
						<span class="arrow" aria-hidden="true">{r.now ? '▶' : ''}</span>
						<span class="db num">{db(r.db)}</span>
						<span class="fired num">{r.fired} fired, {r.passed} passed</span>
						{#if word}<span class="tag" data-testid="ms-rung-word">{word}</span>{/if}
						{#if r.now}<span class="tag strong">In force</span>{/if}
					</li>
				{/each}
			</ol>
			{#each notes as n, i (i)}<p class="aq-small note" data-testid="ms-step-note">{n}</p>{/each}
		</div>
	{/if}

	{#if beforeFirst && review}
		<div class="floors" data-testid="ms-floors">
			<h3>Reachable Before the First Pulse</h3>
			<table class="aq-table">
				<thead><tr><th>Control</th><th class="num">Reachable</th><th class="num">Floor</th><th>Within tolerance</th>{#if review.controls.some((c) => c.predicted_peak_ratio !== null)}<th class="num">First peak</th>{/if}</tr></thead>
				<tbody>
					{#each review.controls as c (c.channel)}
						<tr data-testid="ms-floor-{c.channel}" data-ok={c.reachable_ok}>
							<td>{c.channel}</td>
							<td class="num">{formatNumber(c.reachable * 100, 4)}%</td>
							<td class="num">{signedDb(c.floor_db)}</td>
							<td><StatusMark status={c.reachable_ok ? 'pass' : 'fail'} word={false} /> {c.reachable_ok ? 'Yes' : 'No'}</td>
							{#if review.controls.some((x) => x.predicted_peak_ratio !== null)}<td class="num">{c.predicted_peak_ratio === null ? '-' : `${formatNumber(c.predicted_peak_ratio, 3)} times`}</td>{/if}
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}

	{#if status?.verdict}
		<p class="aq-small aq-muted" data-testid="ms-verdict-mode">Verdict: {VERDICT_WORDS[status.verdict].label.toLowerCase()}. {VERDICT_WORDS[status.verdict].help}</p>
	{/if}
</section>

<style>
	.levelcard {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}

	.statusline {
		display: flex;
		align-items: center;
		gap: 8px;
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
	}

	.words {
		flex: 1;
	}

	.mode {
		font-size: var(--text-2xs);
		color: var(--text-3);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	p {
		margin: 0;
	}

	.coming {
		font-size: var(--text-md, var(--text-sm));
		font-weight: var(--weight-strong);
	}

	.next {
		font-size: var(--text-sm);
		color: var(--text-2);
	}

	.last {
		display: flex;
		align-items: center;
		gap: 6px;
		font-size: var(--text-sm);
	}

	.last.warn {
		font-weight: var(--weight-strong);
	}

	.end {
		padding: var(--sp-2);
		border: 1px solid var(--line-2);
		border-radius: var(--radius-md);
	}

	.end.fail {
		border-width: 2px;
	}

	.end .head {
		display: flex;
		align-items: center;
		gap: 6px;
	}

	.why {
		font-size: var(--text-sm);
	}

	h3 {
		margin: 0 0 4px;
		font-size: var(--text-xs);
		font-weight: var(--weight-strong);
		color: var(--text-2);
	}

	ol {
		margin: 0;
		padding: 0;
		list-style: none;
		font-size: var(--text-xs);
	}

	li {
		display: grid;
		grid-template-columns: 14px 60px minmax(0, 1fr) auto auto;
		gap: 6px;
		align-items: center;
		padding: 2px 0;
	}

	li.now {
		font-weight: var(--weight-strong);
	}

	.tag {
		padding: 0 4px;
		font-size: var(--text-2xs);
		border: 1px solid var(--line-2);
		border-radius: 3px;
	}

	.tag.strong {
		border-color: currentColor;
	}

	.note {
		margin-top: 4px;
	}
</style>
