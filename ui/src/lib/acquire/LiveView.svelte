<script lang="ts">
	/**
	 * The run as it happens, from `random.live`: control against reference on the run's own
	 * analysis lines, drawn as steps across each bin, with the alarm (dotted) and abort
	 * (dash-dot) lines; the drive; the numbers; every input's rms with its overload mark; the
	 * alarms holding now; and how the run ended, with the trip and its axis when a protection
	 * stopped it. Nothing here computes a number.
	 */
	import { formatFixed } from '$lib/viz/format';
	import StatusMark from '$lib/viz/StatusMark.svelte';
	import ViewRenderer from '$lib/viz/ViewRenderer.svelte';
	import LevelLadder from './LevelLadder.svelte';
	import type { AcquireState } from './store.svelte';
	import { alarmText, clock, db, LOG_WORDS, OUTCOME_WORDS, outcomeTone, TRIP_WORDS, tripFacts } from './words';

	interface Props {
		acq: AcquireState;
	}
	let { acq }: Props = $props();

	const run = $derived(acq.run);
	const f = $derived(acq.frame);
	const end = $derived(run?.state === 'finished' ? run.end : null);
	const tone = $derived(end ? outcomeTone(end.outcome) : null);
	const gate = $derived([...acq.log].reverse().find((e) => e.kind === 'gate') ?? null);
	const channelName = (i: number) => {
		const c = f?.channels.find((x) => x.index === i);
		return c ? `Channel ${i} (${c.description || c.role})` : `Channel ${i}`;
	};
	const logTime = (u: number) => new Date(u * 1000).toLocaleTimeString('en-US', { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false });
	const recent = $derived([...acq.log].reverse().slice(0, 200));
</script>

<div class="live" data-testid="live-view">
	{#if end}
		<section class="end {tone}" role={tone === 'fail' ? 'alert' : 'status'} data-testid="run-end" data-outcome={end.outcome}>
			<div class="endmark" aria-hidden="true">
				{#if tone === 'pass'}
					<svg viewBox="0 0 14 14" width="22" height="22"><circle cx="7" cy="7" r="6" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M4.2 7.2l1.9 1.9 3.8-4" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
				{:else if tone === 'info'}
					<svg viewBox="0 0 14 14" width="22" height="22"><rect x="2.5" y="2.5" width="9" height="9" rx="1.3" fill="currentColor" /></svg>
				{:else}
					<svg viewBox="0 0 14 14" width="22" height="22"><path d="M4.5 1h5L13 4.5v5L9.5 13h-5L1 9.5v-5z" fill="var(--status-fail-fill)" /><path d="M5 5l4 4M9 5l-4 4" stroke="var(--status-fail-text)" stroke-width="1.6" stroke-linecap="round" /></svg>
				{/if}
			</div>
			<div class="endbody">
				<h2>
					{end.trip ? TRIP_WORDS[end.trip.kind] : OUTCOME_WORDS[end.outcome]}
					{#if end.at_s !== null}<span class="when num">at {clock(end.at_s)}{end.level_db !== null ? `, ${db(end.level_db)}` : ''}</span>{/if}
				</h2>
				<p class="msg" data-testid="run-end-message">{end.message}</p>
				{#if end.trip}
					<dl class="trip" data-testid="trip">
						{#each tripFacts(end.trip) as t (t.label)}
							<div class:axis={t.label === 'Axis'}><dt>{t.label}</dt><dd class="num">{t.value}</dd></div>
						{/each}
					</dl>
				{/if}
			</div>
			{#if acq.actions.reset.visible}
				<button class="btn" class:primary={acq.actions.reset.enabled} disabled={!acq.actions.reset.enabled} title={acq.actions.reset.reason ?? 'A new run from the same setup.'} onclick={() => acq.reset()}>New Run</button>
			{/if}
		</section>
	{/if}

	{#if run?.state === 'review'}
		<section class="review" role="status" data-testid="review-hold">
			<svg viewBox="0 0 14 14" width="20" height="20" aria-hidden="true"><rect x="3.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" /><rect x="8.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" /></svg>
			<div class="grow">
				<h2>System Identification Passed</h2>
				<p>Nothing drives at test level until you proceed. {gate ? gate.text.replace(/^./, (c) => c.toUpperCase()) + '.' : ''}</p>
			</div>
			<button class="btn primary" disabled={!acq.actions.proceed.enabled} title={acq.actions.proceed.reason ?? undefined} onclick={() => acq.proceed()} data-testid="review-proceed">Proceed</button>
		</section>
	{/if}

	{#if !f}
		<div class="empty">
			<p>
				{#if run?.state === 'running'}
					Waiting for the first frame from the control service.
				{:else}
					Nothing is running. Apply the setup, then Start: the control spectrum against its reference, the drive and the run's numbers appear here.
				{/if}
			</p>
		</div>
	{:else}
		<div class="grid">
			<div class="charts">
				<section class="pane control" aria-label="Control against reference" data-testid="live-control">
					<ViewRenderer view={f.views[0]} live minHeight={260} />
				</section>
				<section class="pane drive" aria-label="Drive" data-testid="live-drive">
					{#if f.views[1].series.length}
						<ViewRenderer view={f.views[1]} live minHeight={130} />
					{:else}
						<h3 class="ptitle">Drive</h3>
						<p class="aq-muted">The drive appears with the first control loop.</p>
					{/if}
				</section>
			</div>
			<aside class="side" aria-label="Run numbers">
				{#if run}<section class="card"><LevelLadder {run} frame={f} /></section>{/if}

				<section class="card" aria-label="Lines">
					<h3>Lines in the Band</h3>
					<dl class="lines num" data-testid="live-lines">
						<div><dt>In band</dt><dd>{f.lines.in_band}</dd></div>
						<div class:warn={f.lines.alarm > 0}><dt>Outside alarm</dt><dd>{f.lines.alarm}</dd></div>
						<div class:warn={f.lines.abort > 0}><dt>Outside abort</dt><dd>{f.lines.abort}</dd></div>
						<div><dt>Loop</dt><dd>{f.loop}</dd></div>
					</dl>
				</section>

				<section class="card" aria-label="Alarms" data-testid="live-alarms">
					<h3>Alarms</h3>
					{#if f.alarms.length}
						<ul class="alarms">
							{#each f.alarms as a, i (i)}
								<li><StatusMark status="warn" word={false} /> {alarmText(a, channelName)}</li>
							{/each}
						</ul>
					{:else}
						<p class="none"><StatusMark status="pass" word={false} /> None holding</p>
					{/if}
				</section>

				<section class="card" aria-label="Run readings">
					<ViewRenderer view={f.views[2]} layout="list" />
				</section>

				<section class="card" aria-label="Inputs" data-testid="live-channels">
					<h3>Inputs</h3>
					<table class="chans">
						<thead><tr><th>#</th><th>Channel</th><th class="r">rms</th><th>Range</th></tr></thead>
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
					<p class="aq-small aq-muted foot">Overload: the input reached 90 % of its range during this run.</p>
				</section>

				<section class="card" aria-label="Run log" data-testid="live-log">
					<h3>Log</h3>
					<ol class="log">
						{#each recent as e (e.seq)}
							<li class={e.kind}><span class="t num">{logTime(e.at_unix)}</span><span class="k">{LOG_WORDS[e.kind] ?? e.kind}</span><span class="x">{e.text}</span></li>
						{/each}
					</ol>
				</section>
			</aside>
		</div>
	{/if}
</div>

<style>
	.live {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-height: 0;
	}

	.end {
		display: flex;
		align-items: flex-start;
		gap: var(--sp-3);
		padding: var(--sp-3) var(--sp-4);
		background: var(--bg-1);
		border: 1px solid var(--line-2);
		border-radius: var(--radius-lg);
	}

	.end.pass {
		border-color: var(--blue-line);
		color: var(--blue-1);
	}

	.end.fail {
		background: var(--amber-weak);
		border: 2px solid var(--amber-2);
	}

	.endmark {
		display: inline-flex;
		margin-top: 2px;
	}

	.end.info .endmark {
		color: var(--text-2);
	}

	.endbody {
		flex: 1;
		min-width: 0;
		color: var(--text-1);
	}

	.end h2 {
		display: flex;
		flex-wrap: wrap;
		align-items: baseline;
		gap: 4px 10px;
		font-size: var(--text-lg);
		font-weight: var(--weight-strong);
	}

	.when {
		font-size: var(--text-sm);
		font-weight: var(--weight-regular);
		color: var(--text-2);
	}

	.msg {
		margin-top: 2px;
		font-size: var(--text-sm);
	}

	.trip {
		display: flex;
		flex-wrap: wrap;
		gap: 6px var(--sp-5);
		margin: var(--sp-2) 0 0;
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
		font-size: var(--text-md);
	}

	.trip .axis dd {
		font-weight: var(--weight-strong);
		text-decoration: underline;
		text-underline-offset: 3px;
	}

	.review {
		display: flex;
		align-items: center;
		gap: var(--sp-3);
		padding: var(--sp-3) var(--sp-4);
		color: var(--blue-1);
		background: var(--blue-weak);
		border: 1.5px solid var(--blue-2);
		border-radius: var(--radius-lg);
	}

	.review h2 {
		font-size: var(--text-md);
		font-weight: var(--weight-strong);
		color: var(--text-1);
	}

	.review p {
		font-size: var(--text-sm);
		color: var(--text-2);
	}

	.grow {
		flex: 1;
	}

	.empty {
		display: grid;
		place-items: center;
		min-height: 280px;
		padding: var(--sp-6);
		color: var(--text-3);
		text-align: center;
		background: var(--bg-plot);
		border: 1px dashed var(--line-2);
		border-radius: var(--radius-lg);
	}

	.empty p {
		max-width: 52ch;
	}

	.grid {
		display: grid;
		grid-template-columns: minmax(0, 1fr) 340px;
		gap: var(--sp-3);
		min-height: 0;
	}

	.charts {
		display: grid;
		grid-template-rows: minmax(320px, 2fr) minmax(200px, 1fr);
		gap: var(--sp-3);
		min-height: 0;
	}

	.pane,
	.card {
		min-height: 0;
		padding: var(--sp-3);
		background: var(--bg-1);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-lg);
	}

	.ptitle,
	.card h3 {
		margin-bottom: var(--sp-2);
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
	}

	.side {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
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

	.alarms {
		margin: 0;
		padding: 0;
		list-style: none;
		font-size: var(--text-sm);
	}

	.alarms li,
	.none {
		display: flex;
		align-items: center;
		gap: 6px;
		padding: 2px 0;
		font-size: var(--text-sm);
	}

	.none {
		color: var(--text-2);
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

	@media (min-height: 700px) and (min-width: 1101px) {
		.live {
			flex: 1;
		}
		.grid {
			flex: 1;
		}
		.charts {
			grid-template-rows: minmax(240px, 2fr) minmax(150px, 1fr);
		}
		.side {
			overflow-y: auto;
			padding-right: 2px;
		}
	}

	@media (max-width: 1100px) {
		.grid {
			grid-template-columns: 1fr;
		}
		.side {
			display: grid;
			grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
			align-items: start;
		}
	}
</style>
