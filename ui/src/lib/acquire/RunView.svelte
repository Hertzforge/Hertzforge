<script lang="ts">
	/**
	 * The run as it happens (the Run view): the page's plots on their grid with the page tabs under
	 * them, and the control dock with the run's cards. Run holds the verbs that apply now, and a
	 * review hold or the run's end as one line with its button (the trip and its axis when a
	 * protection stopped it). Level, Readings (the band's lines), Inputs (every input's rms with its
	 * overload mark) and Log follow. From arm on, the Run card names the folder the run is recorded
	 * in; a finished run's end line offers Open in Analysis and Copy Folder Path. Nothing here
	 * computes a number.
	 */
	import { formatFixed } from '$lib/viz/format';
	import StatusMark from '$lib/viz/StatusMark.svelte';
	import type { DisplayState } from './display/display.svelte';
	import LevelCard from './display/LevelCard.svelte';
	import Dock from './screen/Dock.svelte';
	import PageGrid from './screen/PageGrid.svelte';
	import PageTabs from './screen/PageTabs.svelte';
	import type { PlotPanel } from './screen/screen.svelte';
	import type { CardKind } from './screen/types';
	import RunVerbs from './RunVerbs.svelte';
	import type { AcquireState } from './store.svelte';
	import type { ActionId } from './actions';
	import RunOpenActions from './RunOpenActions.svelte';
	import RunRecordLine from './RunRecordLine.svelte';
	import type { RunOpen } from './runOpen.svelte';
	import { clock, db, LOG_WORDS, OUTCOME_WORDS, outcomeTone, overloadNote, RANGE_MEANING, TRIP_WORDS, tripFacts } from './words';

	interface Props {
		acq: AcquireState;
		display: DisplayState;
		/** Open a page in a window of its own; false when the browser refused. */
		onpopout?: (pageId: string) => boolean;
		/** Open the acknowledgement dialog for arming past a feasibility exceedance. */
		onoverride?: () => void;
		abortKeys: string;
		/** A window on another monitor: the plots and the run's cards, with Stop and Abort the only verbs. */
		compact?: boolean;
		/** A finished run's way to its data on the analysis side; absent in a window that only watches. */
		opener?: RunOpen | null;
	}
	let { acq, display, onpopout, onoverride = () => {}, abortKeys, compact = false, opener = null }: Props = $props();

	const run = $derived(acq.run);
	const f = $derived(acq.frame);
	const end = $derived(run?.state === 'finished' ? run.end : null);
	const tone = $derived(end ? outcomeTone(end.outcome) : null);
	const review = $derived(run?.state === 'review');
	const gate = $derived([...acq.log].reverse().find((e) => e.kind === 'gate') ?? null);
	const channelName = (i: number) => {
		const c = f?.channels.find((x) => x.index === i);
		return c ? `Channel ${i} (${c.description || c.role})` : `Channel ${i}`;
	};
	const logTime = (u: number) => new Date(u * 1000).toLocaleTimeString('en-US', { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false });
	const recent = $derived([...acq.log].reverse().slice(0, 200));
	/** Said only while an input is in overload; otherwise the Range heading explains the word. */
	const overload = $derived(overloadNote(f ? f.channels.filter((c) => c.overload).map((c) => c.index) : []));
	/** The run has streamed: its plots have data or will with the next loop. */
	const streaming = $derived(!!run && (run.state === 'running' || run.state === 'review' || run.state === 'stopping' || (run.state === 'finished' && run.elapsed_s !== null)));
	/** The enabled inputs of the service's channel table, for choosing a plot's channels. */
	const inputs = $derived(
		(acq.server?.channels ?? []).filter((c) => c.enabled && c.role !== 'drive').map((c) => ({ index: c.index, name: c.description || c.role }))
	);
	/** Verbs the Run card draws as its hold and end lines, so not twice. */
	const omit = $derived<ActionId[]>(['apply', ...(review ? (['proceed'] as const) : []), ...(end ? (['reset'] as const) : [])]);
	const reset = $derived(acq.actions.reset);
	const record = $derived(run?.record ?? null);

	function empty(p: PlotPanel): string {
		const data = display.plotData(p.id);
		if (!streaming) return 'Appears when a run starts.';
		if (!data) return 'Waiting for the control service.';
		if (data.channels && data.channels.length === 0) {
			if (p.plot.channels === 'limited') return 'No channel has a limit in this setup.';
			if (p.plot.channels === 'control') return 'The setup has no control channel.';
			return 'None of the chosen channels is in the run.';
		}
		if (p.plot.quantity === 'coherence') return 'Appears when system identification has measured it.';
		return 'Appears with the first control loop.';
	}
</script>

{#snippet card(kind: CardKind)}
	{#if kind === 'run'}
		<div class="runcard">
			<!-- The verbs first: Stop and Abort stay in view however short the card is. -->
			<RunVerbs {acq} {onoverride} {abortKeys} {compact} layout="card" {omit} />
			{#if record}<RunRecordLine {record} ended={!!end} />{/if}
			{#if end}
				<div class="line end {tone}" role={tone === 'fail' ? 'alert' : 'status'} data-testid="run-end" data-outcome={end.outcome}>
					<span class="mk" aria-hidden="true">
						{#if tone === 'pass'}
							<svg viewBox="0 0 14 14" width="16" height="16"><circle cx="7" cy="7" r="6" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M4.2 7.2l1.9 1.9 3.8-4" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
						{:else if tone === 'info'}
							<svg viewBox="0 0 14 14" width="16" height="16"><rect x="2.5" y="2.5" width="9" height="9" rx="1.3" fill="currentColor" /></svg>
						{:else}
							<svg viewBox="0 0 14 14" width="16" height="16"><path d="M4.5 1h5L13 4.5v5L9.5 13h-5L1 9.5v-5z" fill="var(--status-fail-fill)" /><path d="M5 5l4 4M9 5l-4 4" stroke="var(--status-fail-text)" stroke-width="1.6" stroke-linecap="round" /></svg>
						{/if}
					</span>
					<div class="what">
						<p class="head">
							<strong>{end.trip ? TRIP_WORDS[end.trip.kind] : OUTCOME_WORDS[end.outcome]}</strong>
							{#if end.at_s !== null}<span class="when num">at {clock(end.at_s)}{end.level_db !== null ? `, ${db(end.level_db)}` : ''}</span>{/if}
						</p>
						<p class="msg" data-testid="run-end-message">{end.message}</p>
						{#if end.trip}
							<dl class="trip" data-testid="trip">
								{#each tripFacts(end.trip) as t (t.label)}
									<div class:axis={t.label === 'Axis'}><dt>{t.label}</dt><dd class="num">{t.value}</dd></div>
								{/each}
							</dl>
						{/if}
					</div>
					{#if reset.visible && !compact}
						<button class="btn" class:primary={reset.enabled} data-testid="act-reset" aria-disabled={!reset.enabled} class:off={!reset.enabled} title={reset.reason ?? 'A new run from the same setup.'} onclick={() => reset.enabled && acq.reset()}>New Run</button>
					{/if}
					{#if record && opener && !compact}<RunOpenActions {record} {opener} />{/if}
				</div>
			{/if}

			{#if review}
				<div class="line hold" role="status" data-testid="review-hold">
					<span class="mk" aria-hidden="true">
						<svg viewBox="0 0 14 14" width="16" height="16"><rect x="3.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" /><rect x="8.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" /></svg>
					</span>
					<div class="what">
						<p class="head"><strong>System Identification Passed</strong></p>
						<p class="msg">Nothing drives at test level until you proceed.{gate ? ' ' + gate.text.replace(/^./, (c) => c.toUpperCase()) + '.' : ''}</p>
					</div>
					{#if !compact}
						<button class="btn primary" disabled={!acq.actions.proceed.enabled} title={acq.actions.proceed.reason ?? undefined} onclick={() => acq.proceed()} data-testid="review-proceed">Proceed</button>
					{/if}
				</div>
			{/if}

		</div>
	{:else if kind === 'level'}
		<LevelCard {run} frame={f} {channelName} {compact} />
	{:else if kind === 'readings'}
		{#if f}
			<dl class="lines num" data-testid="live-lines">
				<div><dt>In band</dt><dd>{f.lines.in_band}</dd></div>
				<div class:warn={f.lines.alarm > 0}><dt>Outside alarm</dt><dd>{f.lines.alarm}</dd></div>
				<div class:warn={f.lines.abort > 0}><dt>Outside abort</dt><dd>{f.lines.abort}</dd></div>
				<div><dt>Loop</dt><dd>{f.loop}</dd></div>
			</dl>
		{:else}
			<p class="aq-muted">Appears when a run starts.</p>
		{/if}
	{:else if kind === 'inputs'}
		{#if f}
			<div data-testid="live-channels">
				<table class="chans">
					<thead><tr><th>#</th><th>Channel</th><th class="r">rms</th><th><span class="defined" title={RANGE_MEANING} data-testid="range-meaning">Range</span></th></tr></thead>
					<tbody>
						{#each f.channels as c (c.index)}
							<tr class:over={c.overload}>
								<td class="num">{c.index}</td>
								<td>{c.description || c.role}<span class="role">{c.role}</span></td>
								<td class="r num">{c.rms === null ? '-' : `${formatFixed(c.rms, 4)} ${c.unit}`}</td>
								<td>
									{#if c.overload}
										<span class="overload"><StatusMark status="warn" word={false} />Overload</span>
									{:else}
										<span class="aq-muted">OK</span>
									{/if}
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
				{#if overload}
					<p class="aq-small foot overnote" role="status" data-testid="overload-note"><StatusMark status="warn" word={false} />{overload}</p>
				{/if}
			</div>
		{:else}
			<p class="aq-muted">Appears when a run starts.</p>
		{/if}
	{:else if kind === 'log'}
		{#if recent.length}
			<ol class="log" data-testid="live-log">
				{#each recent as e (e.seq)}
					<li class={e.kind}><span class="t num">{logTime(e.at_unix)}</span><span class="k">{LOG_WORDS[e.kind] ?? e.kind}</span><span class="x">{e.text}</span></li>
				{/each}
			</ol>
		{:else}
			<p class="aq-muted">Nothing logged yet.</p>
		{/if}
	{/if}
{/snippet}

<div class="live" class:compact class:bottom={display.dock.side === 'bottom'} data-testid="live-view">
	<div class="area">
		<PageGrid screen={display} {card} channels={inputs} {empty} live={streaming} />
		<PageTabs screen={display} {onpopout} />
	</div>
	<Dock screen={display} {card} />
</div>

<style>
	.live {
		display: grid;
		grid-template-columns: minmax(0, 1fr) auto;
		height: 100%;
		min-height: 0;
	}

	.live.bottom {
		grid-template-columns: minmax(0, 1fr);
		grid-template-rows: minmax(0, 1fr) auto;
	}

	.area {
		display: flex;
		flex-direction: column;
		min-width: 0;
		min-height: 0;
	}

	.runcard {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}

	/* A hold or an end: one line, its mark, its words and its button. */
	.line {
		display: flex;
		align-items: flex-start;
		gap: var(--sp-2);
		padding: var(--sp-2);
		background: var(--bg-2);
		border: 1px solid var(--line-2);
		border-radius: var(--radius-md);
	}

	.line .mk {
		display: inline-flex;
		margin-top: 1px;
	}

	.line .what {
		flex: 1;
		min-width: 0;
	}

	.line .btn {
		flex: none;
	}

	.head {
		display: flex;
		flex-wrap: wrap;
		align-items: baseline;
		gap: 2px 8px;
		font-size: var(--text-sm);
		color: var(--text-1);
	}

	.when {
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	.msg {
		margin-top: 2px;
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	.hold {
		color: var(--blue-1);
		background: var(--blue-weak);
		border-color: var(--blue-2);
	}

	.end.pass {
		color: var(--blue-1);
		border-color: var(--blue-line);
	}

	.end.info .mk {
		color: var(--text-2);
	}

	.end.fail {
		background: var(--amber-weak);
		border: 2px solid var(--amber-2);
	}

	.trip {
		display: flex;
		flex-wrap: wrap;
		gap: 4px var(--sp-4);
		margin: 6px 0 0;
	}

	.trip div {
		display: flex;
		flex-direction: column;
	}

	.trip dt {
		font-size: var(--text-2xs);
		color: var(--text-3);
		text-transform: uppercase;
		letter-spacing: 0.05em;
	}

	.trip dd {
		margin: 0;
		font-size: var(--text-sm);
		color: var(--text-1);
	}

	.trip .axis dd {
		font-weight: var(--weight-strong);
		text-decoration: underline;
		text-underline-offset: 3px;
	}

	.btn.off {
		opacity: 0.45;
		cursor: default;
	}

	.end {
		flex-wrap: wrap;
	}

	.lines {
		display: grid;
		grid-template-columns: repeat(4, minmax(0, 1fr));
		gap: 4px;
		margin: 0;
	}

	.lines div {
		display: flex;
		flex-direction: column;
		padding: 4px 6px;
		border: 1px solid var(--line-1);
		border-radius: var(--radius-sm);
	}

	.lines div.warn {
		border-color: var(--amber-2);
		border-style: dashed;
	}

	.lines dt {
		font-family: var(--font-ui);
		font-size: var(--text-2xs);
		color: var(--text-3);
	}

	.lines dd {
		margin: 0;
		font-size: var(--text-md);
	}

	.chans {
		width: 100%;
		border-collapse: collapse;
		font-size: var(--text-xs);
	}

	.chans th {
		padding: 3px 4px;
		font-size: var(--text-2xs);
		font-weight: var(--weight-strong);
		color: var(--text-3);
		text-align: left;
		text-transform: uppercase;
		letter-spacing: 0.04em;
		border-bottom: 1px solid var(--line-2);
	}

	.chans td {
		padding: 4px;
		border-bottom: 1px solid var(--line-1);
	}

	.chans .r {
		text-align: right;
	}

	.role {
		margin-left: 6px;
		font-size: var(--text-2xs);
		color: var(--text-3);
	}

	.overload {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		font-weight: var(--weight-strong);
		color: var(--amber-1);
	}

	tr.over td {
		background: var(--amber-weak);
	}

	.foot {
		margin-top: 6px;
	}

	.overnote {
		display: flex;
		align-items: flex-start;
		gap: 4px;
		color: var(--amber-1);
	}

	/* A heading with a definition behind it: dotted, and the definition on hover. */
	.defined {
		text-decoration: underline dotted;
		text-underline-offset: 2px;
		cursor: help;
	}

	.log {
		max-height: 220px;
		margin: 0;
		padding: 0;
		overflow-y: auto;
		list-style: none;
		font-size: var(--text-xs);
	}

	.log li {
		display: grid;
		grid-template-columns: auto 5.5em minmax(0, 1fr);
		gap: 6px;
		padding: 2px 0;
		border-bottom: 1px solid var(--line-1);
	}

	.log .t {
		color: var(--text-3);
	}

	.log .k {
		color: var(--text-3);
		font-size: var(--text-2xs);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	.log li.trip .k,
	.log li.alarm .k,
	.log li.overload .k,
	.log li.override .k {
		color: var(--amber-1);
		font-weight: var(--weight-strong);
	}
</style>
