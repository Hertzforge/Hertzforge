<script lang="ts">
	/**
	 * The Measure screen, read at arm's length with a hammer in one hand: the point list with the
	 * selected point marked and Previous and Next; the state in large words; the counter of accepted
	 * hits against the averages asked, the rejected beside it, and the dataset's totals; a tile for
	 * every hit at this point; the latest hit's own block, force and response, with the force window
	 * and the exponential window it was taken with over them and a second impact marked; the force target's High or Low flag with the service's percent and
	 * a force-level bar with the target band; the force spectrum with the tip's roll-off; the FRF magnitude, phase and
	 * coherence as the average builds; large Accept and Reject buttons with their keys; and Clear Point,
	 * which asks first. In
	 * shaker mode: Start and Stop Driving, the traces, the reference autospectra, the FRF and the
	 * frame counter. Every count, flag and percent is the control service's; this screen draws them.
	 *
	 * The keys (Enter or A, Backspace or R, U, the Left and Right arrows, Space) work wherever the
	 * focus is, never while a text field has it, and in both accept modes.
	 */
	import type { Snippet } from 'svelte';
	import SpectrumPane from '$lib/viz/SpectrumPane.svelte';
	import { MODAL_HIT_KINDS, type ModalHitKind } from '$lib/protocol/modal_test';
	import RunBar from '../RunBar.svelte';
	import RunOpenActions from '../RunOpenActions.svelte';
	import RunRecordLine from '../RunRecordLine.svelte';
	import type { RunOpen } from '../runOpen.svelte';
	import { outcomeTone } from '../words';
	import { MODAL_KEY_WORDS } from './modalTestKeys';
	import type { ModalTestStore } from './modalTestStore.svelte';
	import type { ModalVerb } from './modalTestVerbs';
	import { forceSpectrumView, frfViews, hitViews, referenceSpectraView, roleView, dofName, secondImpactWords } from './modalTestViews';
	import { counterWords, MODAL_DRIVE_PHASE_WORDS, MODAL_OUTCOME_HELP, MODAL_OUTCOME_WORDS, refusalWords, modalStateHint, modalStateWords, num, reportLines, targetFlag, tileWords, verdictWords } from './modalTestWords';

	interface Props {
		modal: ModalTestStore;
		abortKeys: string;
		opener?: RunOpen | null;
		station?: Snippet;
	}
	let { modal, abortKeys, opener = null }: Props = $props();

	const s = $derived(modal.now);
	const check = $derived(modal.runCheck);
	const mode = $derived(s?.mode ?? check?.mode ?? modal.setup.mode);
	const impact = $derived(mode === 'impact');
	const live = $derived(modal.live);
	const verbs = $derived(modal.verbs);
	const dofs = $derived(s?.point_dofs ?? { references: check?.references ?? [], responses: check?.responses ?? [] });
	const latest = $derived(s?.latest && s.latest.point === s.point ? s.latest : null);
	const counter = $derived(counterWords(s?.counts));
	const unit = $derived(dofs.references[0]?.unit ?? '');
	const accept = $derived(check?.impact?.accept ?? modal.setup.impact.accept);
	const end = $derived(s?.state === 'finished' ? s.end : null);
	const record = $derived(modal.record);

	const hit = $derived(impact ? hitViews(live, latest, dofs, modal.shown.response) : null);
	const second = $derived(secondImpactWords(latest?.report?.double_hit, live?.time.rate_hz ?? 0, unit));
	const tipDrop = $derived(live?.hit?.force_spectrum.tip_drop_db ?? latest?.report?.tip_drop_db ?? null);
	const frf = $derived(frfViews(live, dofs, modal.shown.response, modal.shown.reference));
	const fspec = $derived(impact ? forceSpectrumView(live) : null);
	const refSpectra = $derived(!impact ? referenceSpectraView(live, dofs) : null);
	const refTraces = $derived(!impact ? roleView(live, 'reference', dofs, 'Force') : null);
	const respTraces = $derived(!impact ? roleView(live, 'response', dofs, 'Response') : null);
	const driveTraces = $derived(!impact ? roleView(live, 'drive', dofs, 'Drive') : null);

	/** The target the latest hit was judged against, as the service reports it. */
	const target = $derived.by(() => {
		const r = latest?.report;
		if (r && r.target !== undefined) return { peak: r.target, tol: r.target_tolerance_pct ?? 0 };
		const t = check?.impact?.checks.target;
		return t ? { peak: t.peak, tol: t.tolerance_pct } : null;
	});
	const flag = $derived(latest ? targetFlag(latest.report?.target_verdict ?? latest.target_verdict, latest.report?.target_diff_pct ?? latest.target_diff_pct) : null);
	/** The force bar's scale: the band and the peak drawn on one span, from zero. */
	const bar = $derived.by(() => {
		const peak = latest?.report?.peak ?? latest?.peak;
		if (!target || peak === undefined) return null;
		const lo = target.peak * (1 - target.tol / 100);
		const hi = target.peak * (1 + target.tol / 100);
		const top = Math.max(hi, peak) * 1.25;
		const at = (v: number) => `${Math.max(0, Math.min(100, (v / top) * 100))}%`;
		return { lo: at(lo), width: `${((hi - lo) / top) * 100}%`, peak: at(peak), target: at(target.peak), value: peak };
	});

	/** Each point of the plan named by its roving DOFs. */
	const points = $derived.by(() => {
		const plan = check?.points ?? [];
		return (s?.counts.points ?? []).map((p) => {
			const roving = plan[p.index]?.responses.filter((r) => r.roves) ?? [];
			const name = roving.length ? roving.map((r) => `${r.node ?? '-'}${r.direction ?? ''}`).join(', ') : plan.length > 1 ? '' : 'Fixed DOFs';
			return { ...p, name };
		});
	});

	let list = $state<HTMLElement | null>(null);
	$effect(() => {
		const i = s?.point;
		if (i === undefined) return;
		list?.querySelector<HTMLElement>(`[data-point="${i}"]`)?.scrollIntoView({ block: 'nearest' });
	});

	/** A verb from a button: then the button lets the focus go, so the keys stay the screen's. */
	function press(v: ModalVerb, e?: Event) {
		(e?.currentTarget as HTMLElement | null)?.blur();
		void modal.verb(v);
	}

	function onkeydown(e: KeyboardEvent) {
		if (modal.onKey(e)) e.preventDefault();
	}

	/** Clear Point asks first: it takes every accepted average of the point out. */
	let clearing = $state(false);
	$effect(() => {
		// a new point, or nothing left to clear, closes the question
		void s?.point;
		if (!verbs.clear.enabled) clearing = false;
	});
	function confirmClear(e: Event) {
		(e.currentTarget as HTMLElement | null)?.blur();
		clearing = false;
		void modal.verb('clear');
	}

	const SIM_WORDS: Record<ModalHitKind, string> = { clean: 'Clean', double: 'Double', overload: 'Overload' };
	const armVerb = $derived<ModalVerb>(s?.state === 'armed' || s?.state === 'arming' ? 'disarm' : 'arm');
</script>

<svelte:window {onkeydown} />

{#snippet verbButton(v: ModalVerb, label: string, keys: string | null, cls = '')}
	{@const g = verbs[v]}
	{#if g.visible}
		<button class="btn {cls}" class:off={!g.enabled} aria-disabled={!g.enabled} title={g.reason ?? undefined} onclick={(e) => g.enabled && press(v, e)} data-testid="mt-{v}">
			<span class="word">{label}</span>
			{#if keys}<span class="keys">{keys}</span>{/if}
		</button>
	{/if}
{/snippet}

{#snippet clearPoint()}
	{@const g = verbs.clear}
	{#if g.visible}
		{#if clearing}
			<div class="ask" role="alertdialog" aria-labelledby="mt-clear-q" data-testid="mt-clear-ask">
				<p id="mt-clear-q">Take all {s?.counts.point.accepted ?? 0} accepted {impact ? (s?.counts.point.accepted === 1 ? 'hit' : 'hits') : s?.counts.point.accepted === 1 ? 'frame' : 'frames'} at point {(s?.point ?? 0) + 1} out? The point opens again; its tiles stay, marked undone.</p>
				<div class="row2">
					<button class="btn danger" onclick={confirmClear} data-testid="mt-clear-yes">Clear Point</button>
					<button class="btn ghost" onclick={(e) => { (e.currentTarget as HTMLElement).blur(); clearing = false; }} data-testid="mt-clear-no">Keep Them</button>
				</div>
			</div>
		{:else}
			<button class="btn ghost clear" class:off={!g.enabled} aria-disabled={!g.enabled} title={g.reason ?? 'Take every accepted average of this point out'} onclick={(e) => { (e.currentTarget as HTMLElement).blur(); if (g.enabled) clearing = true; }} data-testid="mt-clear">Clear Point</button>
		{/if}
	{/if}
{/snippet}

{#snippet barNums()}
	<div>
		<dt>Point</dt>
		<dd class="num">{s ? `${s.point + 1} of ${s.counts.totals.points_planned}` : '-'}</dd>
	</div>
	<div>
		<dt>Measured</dt>
		<dd class="num">{s ? `${s.counts.totals.points_measured} of ${s.counts.totals.points_planned}` : '-'}</dd>
	</div>
{/snippet}

<div class="measure" class:shaker={!impact} data-testid="mt-measure" data-mode={mode} data-state={s?.state ?? 'none'}>
	<div class="bar"><RunBar acq={modal} onoverride={() => {}} {abortKeys} nums={barNums} /></div>

	<aside class="left">
		<section class="panel state" data-testid="mt-state">
			<p class="big" data-testid="mt-state-words">{modalStateWords(s?.state, mode)}</p>
			<p class="hint">{modalStateHint(s?.state, mode, accept)}</p>
			{#if !impact && s}<p class="hint" data-testid="mt-drive-phase">Drive: {MODAL_DRIVE_PHASE_WORDS[s.drive_phase]}</p>{/if}
		</section>

		<section class="panel counter" data-testid="mt-counter" aria-label="Counts">
			{#if counter && s}
				<p class="count" style:font-size={counter.point.length <= 6 ? null : counter.point.length <= 8 ? '40px' : '30px'} data-testid="mt-count"><span class="num">{counter.point}</span></p>
				<p class="unit">{impact ? 'accepted hits' : 'frames'} at point {s.point + 1}</p>
				{#if impact}<p class="rej" class:some={s.counts.point.rejected > 0} data-testid="mt-rejected"><span class="num">{counter.rejected}</span></p>{/if}
				<p class="tot" data-testid="mt-dataset">{counter.dataset}</p>
				<p class="tot" data-testid="mt-points">{counter.points}</p>
			{:else}
				<p class="hint">The counts appear when the test starts.</p>
			{/if}
		</section>

		<section class="panel points" aria-labelledby="mt-pts">
			<div class="head">
				<h2 id="mt-pts">Points</h2>
				<div class="nav">
					{@render verbButton('previous', 'Previous', '←', 'sm')}
					{@render verbButton('next', 'Next', '→', 'sm')}
				</div>
			</div>
			<ol class="plist" bind:this={list} data-testid="mt-points-list">
				{#each points as p (p.index)}
					<li>
						<button class="pt" class:on={p.index === s?.point} class:done={p.measured} aria-current={p.index === s?.point ? 'step' : undefined} data-point={p.index} disabled={!modal.isController || !modal.isLive || s?.state === 'measuring'} onclick={(e) => { (e.currentTarget as HTMLElement).blur(); void modal.select(p.index); }}>
							<span class="n num">{p.index + 1}</span>
							<span class="nm">{p.name}</span>
							<span class="c num">{p.accepted}/{s?.counts.point.asked ?? ''}</span>
							<span class="w">{p.measured ? 'Measured' : p.taken ? 'Started' : ''}</span>
						</button>
					</li>
				{/each}
			</ol>
		</section>

		<section class="panel keys" aria-labelledby="mt-keys">
			<h2 id="mt-keys">Keys</h2>
			<dl>
				{#if impact}
					<div><dt>{MODAL_KEY_WORDS.accept}</dt><dd>Accept the hit</dd></div>
					<div><dt>{MODAL_KEY_WORDS.reject}</dt><dd>Reject the hit</dd></div>
				{/if}
				<div><dt>{MODAL_KEY_WORDS.undo}</dt><dd>Undo the last accepted</dd></div>
				<div><dt>{MODAL_KEY_WORDS.previous}, {MODAL_KEY_WORDS.next}</dt><dd>Change point</dd></div>
				{#if impact}<div><dt>{MODAL_KEY_WORDS.arm}</dt><dd>Arm or disarm</dd></div>{/if}
			</dl>
			<p class="hint">Not while a text field has the focus.</p>
		</section>

		<div class="endtest">{@render verbButton('end', 'End Test', null)}</div>
	</aside>

	<main class="center">
		{#if impact}
			<section class="tiles" aria-label="Hits at this point" data-testid="mt-tiles">
				{#each s?.hits ?? [] as t (t.seq)}
					{@const w = tileWords(t)}
					<div class="tile {w.tone}" class:latest={t.seq === latest?.seq} data-testid="mt-tile" data-status={t.status}>
						<p class="tn">Hit <span class="num">{t.n}</span></p>
						<p class="ts">{w.status}</p>
						{#if w.reason && !w.flags.some((f) => f.word === w.reason)}<p class="tr">{w.reason}</p>{/if}
						{#each w.flags as f (f.kind)}<span class="flag {f.kind}" data-flag={f.kind}>{f.word}</span>{/each}
					</div>
				{:else}
					<p class="hint">No hit at this point yet.</p>
				{/each}
			</section>
		{/if}

		<div class="plots" class:impact>
			{#if impact}
				<div class="plot" data-testid="mt-force-plot">
					{#if hit?.force}
						<SpectrumPane view={hit.force} live minHeight={150} readout={false} copyButton={false} regions={hit.forceRegions} marker={hit.second} />
					{:else}<p class="empty">The force is drawn once a hit lands.</p>{/if}
					{#if latest?.double_hit}<span class="pflag double" data-testid="mt-double-flag">Double Hit{second ? `: ${second}` : ''}</span>{/if}
					{#if latest?.overload}<span class="pflag overload" data-testid="mt-overload-flag">Overload</span>{/if}
				</div>
				<div class="plot" data-testid="mt-response-plot">
					{#if hit?.response}<SpectrumPane view={hit.response} live minHeight={150} readout={false} copyButton={false} regions={hit.responseRegions} marker={hit.second} />{:else}<p class="empty">The response is drawn once a hit lands.</p>{/if}
				</div>
				<div class="plot" data-testid="mt-fspec-plot">
					{#if fspec}<SpectrumPane view={fspec} live minHeight={150} readout={false} copyButton={false} />{:else}<p class="empty">The force spectrum is drawn once a hit lands.</p>{/if}
					{#if latest?.tip_warning}<span class="pflag tip" data-testid="mt-tip-flag">Tip: the force falls {num(tipDrop)} dB across the band. A harder tip reaches higher.</span>{/if}
				</div>
			{:else}
				<div class="plot">{#if refTraces}<SpectrumPane view={refTraces} live minHeight={150} readout={false} copyButton={false} />{:else}<p class="empty">The forces are drawn while the shakers drive.</p>{/if}</div>
				<div class="plot">{#if respTraces}<SpectrumPane view={respTraces} live minHeight={150} readout={false} copyButton={false} />{:else}<p class="empty">The responses are drawn while the shakers drive.</p>{/if}</div>
				<div class="plot">{#if driveTraces}<SpectrumPane view={driveTraces} live minHeight={150} readout={false} copyButton={false} />{:else}<p class="empty">The drives are drawn while the shakers drive.</p>{/if}</div>
				<div class="plot" data-testid="mt-refspec-plot">{#if refSpectra}<SpectrumPane view={refSpectra} live minHeight={150} readout={false} copyButton={false} />{:else}<p class="empty">The reference autospectra are drawn as the average builds.</p>{/if}</div>
			{/if}
			<div class="plot" data-testid="mt-frf-plot">{#if frf.magnitude}<SpectrumPane view={frf.magnitude} live minHeight={150} readout={false} copyButton={false} />{:else}<p class="empty">The FRF is drawn after the first accepted {impact ? 'hit' : 'frame'}.</p>{/if}</div>
			<div class="plot">{#if frf.phase}<SpectrumPane view={frf.phase} live minHeight={150} readout={false} copyButton={false} yFixed={[-180, 180]} />{:else}<p class="empty">The phase is drawn with the FRF.</p>{/if}</div>
			<div class="plot" data-testid="mt-coh-plot">{#if frf.coherence}<SpectrumPane view={frf.coherence} live minHeight={150} readout={false} copyButton={false} yFixed={[0, 1.05]} />{:else}<p class="empty">The coherence is drawn with the FRF.</p>{/if}</div>
		</div>
		<div class="pick">
			<label>Response
				<select class="field" bind:value={modal.shown.response} data-testid="mt-show-response">
					{#each dofs.responses as r, i (i)}<option value={i}>{dofName(r, `Response ${i + 1}`)}</option>{/each}
				</select>
			</label>
			{#if dofs.references.length > 1}
				<label>Reference
					<select class="field" bind:value={modal.shown.reference} data-testid="mt-show-reference">
						{#each dofs.references as r, i (i)}<option value={i}>{dofName(r, `Reference ${i + 1}`)}</option>{/each}
					</select>
				</label>
			{/if}
			<span class="aq-small aq-muted num">{frf.averages} {frf.averages === 1 ? 'average' : 'averages'} in the FRF{check ? `, ${check.estimator}` : ''}</span>
		</div>
	</main>

	<aside class="right">
		{#if end}
			<section class="panel endline {outcomeTone(end.outcome)}" data-testid="mt-end" data-outcome={end.outcome}>
				<h2 class="outcome">{MODAL_OUTCOME_WORDS[end.outcome] ?? end.outcome}</h2>
				{#if MODAL_OUTCOME_HELP[end.outcome]}<p class="help" data-testid="mt-end-help">{MODAL_OUTCOME_HELP[end.outcome]}</p>{/if}
				<p class="hint">{end.message}</p>
				{#if record}<RunRecordLine {record} ended />{/if}
				{#if record && opener}<RunOpenActions {record} {opener} />{/if}
			</section>
		{/if}
		{#if impact}
			<section class="panel judge" aria-label="The latest hit">
				<p class="verdict" data-testid="mt-verdict" data-status={latest?.status ?? ''}>{verdictWords(latest, s?.counts.unit) ?? 'No hit yet'}</p>
				<div class="ar">
					{@render verbButton('accept', 'ACCEPT', MODAL_KEY_WORDS.accept, 'accept')}
					{@render verbButton('reject', 'REJECT', MODAL_KEY_WORDS.reject, 'reject')}
				</div>
				<div class="row2">
					{@render verbButton('undo', 'Undo', MODAL_KEY_WORDS.undo)}
					{@render verbButton(armVerb, armVerb === 'arm' ? 'Arm' : 'Disarm', MODAL_KEY_WORDS.arm)}
				</div>
				{@render clearPoint()}
				{#if modal.verbRefusal}<p class="refused" role="alert" data-testid="mt-refusal" data-reason={modal.verbRefusal.reason}>{refusalWords(modal.verbRefusal)}</p>{/if}
				<ul class="report">
					{#each reportLines(latest?.report, unit) as line, i (i)}<li>{line}</li>{/each}
					{#if second}<li data-testid="mt-second-words">{second}</li>{/if}
				</ul>
			</section>

			<section class="panel target" aria-labelledby="mt-tgt" data-testid="mt-target">
				<h2 id="mt-tgt">Force Target</h2>
				{#if flag}
					<p class="tflag {flag.tone}" data-testid="mt-target-flag" data-tone={flag.tone}>{flag.word}</p>
				{/if}
				{#if bar && target}
					<div class="fbar" role="img" aria-label="Peak {num(bar.value, unit)} against a target of {num(target.peak, unit)}, plus or minus {num(target.tol)}%">
						<span class="band" style:left={bar.lo} style:width={bar.width}></span>
						<span class="tick" style:left={bar.target}></span>
						<span class="peak {flag?.tone ?? ''}" style:left={bar.peak}></span>
					</div>
					<p class="hint num">Peak {num(bar.value, unit)}; target {num(target.peak, unit)} within {num(target.tol)}%. A signal only: a hit is never rejected for its force.</p>
				{:else}
					<p class="hint">{target ? 'The bar shows the next hit against the target.' : 'No target set. Use a hit you like as the target, or set one in the setup.'}</p>
				{/if}
				{@render verbButton('target', 'Use This Hit as the Target', null)}
			</section>

			{#if modal.onSimulator && modal.isLive}
				<section class="panel sim" aria-labelledby="mt-sim" data-testid="mt-sim">
					<h2 id="mt-sim">Simulated Hit</h2>
					<p class="hint">On the simulator: the next hit is made this way.</p>
					<div class="aq-seg sm" role="group" aria-label="Simulated hit">
						{#each MODAL_HIT_KINDS as k (k)}
							<button type="button" disabled={!modal.isController} onclick={(e) => { (e.currentTarget as HTMLElement).blur(); void modal.simHit(k); }} data-testid="mt-sim-{k}">{SIM_WORDS[k]}</button>
						{/each}
					</div>
				</section>
			{/if}
		{:else}
			<section class="panel judge" aria-label="The drive">
				<p class="verdict" data-testid="mt-frames">{s ? `${s.counts.point.accepted} of ${s.counts.point.asked} frames` : '-'}</p>
				<div class="row2">
					{@render verbButton('drive_start', 'Start Driving', null, 'primary')}
					{@render verbButton('drive_stop', 'Stop Driving', null)}
					{@render verbButton('undo', 'Undo', MODAL_KEY_WORDS.undo)}
				</div>
				{@render clearPoint()}
				{#if modal.verbRefusal}<p class="refused" role="alert" data-testid="mt-refusal" data-reason={modal.verbRefusal.reason}>{refusalWords(modal.verbRefusal)}</p>{/if}
				{#if check?.shaker}
					<ul class="report">
						<li>{check.shaker.drives.length} {check.shaker.drives.length === 1 ? 'shaker' : 'shakers'}, {check.estimator}.</li>
						{#each check.shaker.drives as d (d.row)}<li class="num">Row {d.row}: {num(d.level_vrms, 'Vrms')}, peak {num(d.peak_v, 'V')}</li>{/each}
						<li class="num">About {num(check.shaker.seconds_per_point, 's')} a point.</li>
					</ul>
				{/if}
			</section>
		{/if}


	</aside>
</div>

<style>
	.measure {
		display: grid;
		grid-template-columns: 250px minmax(0, 1fr) 300px;
		grid-template-rows: auto minmax(0, 1fr);
		grid-template-areas: 'bar bar bar' 'left center right';
		gap: var(--sp-3);
		height: 100%;
		min-height: 0;
		padding: var(--sp-3);
		box-sizing: border-box;
	}

	.bar {
		grid-area: bar;
	}

	.left,
	.right {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-height: 0;
		overflow: auto;
	}

	.left {
		grid-area: left;
	}

	.right {
		grid-area: right;
	}

	.center {
		grid-area: center;
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		min-height: 0;
		min-width: 0;
		overflow: auto;
	}

	.panel {
		background: var(--bg-1);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-md);
		padding: var(--sp-3);
	}

	.panel h2 {
		margin: 0 0 var(--sp-2);
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
		color: var(--text-2);
	}

	p {
		margin: 0;
	}

	.hint {
		font-size: var(--text-xs);
		color: var(--text-3);
		margin-top: var(--sp-1);
	}

	.big {
		font-size: 30px;
		line-height: 1.1;
		font-weight: var(--weight-strong);
		color: var(--text-1);
	}

	.measure[data-state='armed'] .state .big {
		color: var(--accent);
	}

	.counter .count {
		white-space: nowrap;
		font-size: 56px;
		line-height: 1;
		font-weight: var(--weight-strong);
		color: var(--text-1);
	}

	.counter .unit {
		font-size: var(--text-sm);
		color: var(--text-2);
		margin-top: var(--sp-1);
	}

	.counter .rej {
		font-size: var(--text-lg);
		color: var(--text-3);
		margin-top: var(--sp-2);
	}

	.counter .rej.some {
		color: var(--status-fail);
	}

	.counter .tot {
		font-size: var(--text-xs);
		color: var(--text-2);
		margin-top: var(--sp-1);
	}

	.points {
		display: flex;
		flex-direction: column;
		min-height: 160px;
		flex: 1;
	}

	.points .head {
		display: flex;
		align-items: center;
		justify-content: space-between;
	}

	.points .head h2 {
		margin: 0;
	}

	.nav {
		display: flex;
		gap: var(--sp-1);
	}

	.endtest {
		display: flex;
		flex-direction: column;
	}

	.plist {
		list-style: none;
		margin: var(--sp-2) 0 0;
		padding: 0;
		overflow: auto;
		flex: 1;
	}

	.pt {
		display: grid;
		grid-template-columns: 28px 1fr auto;
		grid-template-areas: 'n nm c' 'n w w';
		align-items: baseline;
		width: 100%;
		padding: 6px 8px;
		border: 1px solid transparent;
		border-radius: var(--radius-sm);
		background: none;
		color: var(--text-2);
		text-align: left;
		font: inherit;
		cursor: pointer;
	}

	.pt:disabled {
		cursor: default;
	}

	.pt .n {
		grid-area: n;
		font-weight: var(--weight-strong);
	}

	.pt .nm {
		grid-area: nm;
		font-size: var(--text-sm);
	}

	.pt .c {
		grid-area: c;
		font-size: var(--text-xs);
	}

	.pt .w {
		grid-area: w;
		font-size: var(--text-2xs);
		color: var(--text-3);
	}

	.pt.done .w {
		color: var(--status-pass);
	}

	.pt.on {
		background: var(--accent-weak);
		border-color: var(--accent);
		color: var(--text-1);
	}

	.tiles {
		display: flex;
		gap: var(--sp-2);
		overflow-x: auto;
		min-height: 74px;
		padding-bottom: 2px;
	}

	.tile {
		flex: 0 0 auto;
		min-width: 104px;
		padding: 6px 8px;
		border-radius: var(--radius-sm);
		border: 1px solid var(--line-2);
		border-left-width: 5px;
		background: var(--bg-1);
	}

	.tile.latest {
		box-shadow: 0 0 0 2px var(--focus);
	}

	.tile.pass {
		border-left-color: var(--status-pass);
	}

	.tile.fail {
		border-left-color: var(--status-fail);
	}

	.tile.wait {
		border-left-color: var(--status-warn);
	}

	.tile.muted {
		border-left-color: var(--line-3);
		opacity: 0.7;
	}

	.tn {
		font-size: var(--text-xs);
		color: var(--text-3);
	}

	.ts {
		font-size: var(--text-md);
		font-weight: var(--weight-strong);
		color: var(--text-1);
	}

	.tile.pass .ts {
		color: var(--status-pass);
	}

	.tile.fail .ts {
		color: var(--status-fail);
	}

	.tr {
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	.flag,
	.pflag {
		display: inline-block;
		margin: 3px 3px 0 0;
		padding: 1px 6px;
		border-radius: var(--radius-sm);
		font-size: var(--text-2xs);
		font-weight: var(--weight-strong);
		border: 1px solid currentColor;
	}

	.flag.double,
	.flag.overload,
	.pflag.double,
	.pflag.overload {
		color: var(--status-fail);
	}

	.flag.high,
	.flag.low,
	.flag.tip,
	.flag.cut,
	.pflag.tip {
		color: var(--status-warn);
	}

	.flag.on {
		color: var(--status-pass);
	}

	.plots {
		display: grid;
		grid-template-columns: repeat(3, minmax(0, 1fr));
		grid-auto-rows: minmax(150px, 1fr);
		gap: var(--sp-2);
		flex: 1;
		min-height: 0;
	}

	.plots.impact {
		grid-auto-rows: minmax(170px, 1fr);
	}

	/* Shaker mode draws seven plots: the coherence takes the third row whole. */
	.plots:not(.impact) > .plot:last-child {
		grid-column: 1 / -1;
	}

	.plot {
		position: relative;
		background: var(--bg-plot);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-sm);
		min-width: 0;
		min-height: 0;
		display: flex;
		flex-direction: column;
	}

	.plot .pflag {
		position: absolute;
		left: 50%;
		transform: translateX(-50%);
		bottom: 34px;
		background: var(--bg-1);
	}

	.plot .pflag.overload {
		bottom: 58px;
	}

	.plot .pflag.tip {
		max-width: 90%;
		white-space: normal;
		text-align: center;
	}

	.empty {
		margin: auto;
		padding: var(--sp-3);
		font-size: var(--text-xs);
		color: var(--text-3);
		text-align: center;
	}

	.pick {
		display: flex;
		align-items: center;
		gap: var(--sp-3);
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	.pick label {
		display: flex;
		align-items: center;
		gap: var(--sp-1);
	}

	.verdict {
		font-size: var(--text-lg);
		font-weight: var(--weight-strong);
		color: var(--text-1);
		margin-bottom: var(--sp-2);
	}

	.verdict[data-status='rejected'] {
		color: var(--status-fail);
	}

	.verdict[data-status='accepted'] {
		color: var(--status-pass);
	}

	.verdict[data-status='pending'] {
		color: var(--status-warn);
	}

	.ar {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: var(--sp-2);
	}

	.row2 {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: var(--sp-2);
		margin-top: var(--sp-2);
	}

	.btn {
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 2px;
		width: 100%;
	}

	.btn .keys {
		font-size: var(--text-2xs);
		font-weight: var(--weight-regular);
		opacity: 0.8;
	}

	.btn.sm {
		flex-direction: row;
		width: auto;
		gap: 4px;
		padding: 2px 8px;
	}

	.btn.accept,
	.btn.reject {
		min-height: 76px;
		font-size: 22px;
		font-weight: var(--weight-strong);
		border: none;
	}

	.btn.accept {
		background: var(--accent);
		color: var(--text-on-accent);
	}

	.btn.reject {
		background: var(--status-fail-fill);
		color: var(--status-fail-text);
	}

	.btn.accept.off,
	.btn.reject.off {
		opacity: 0.4;
	}

	.refused {
		margin-top: var(--sp-2);
		font-size: var(--text-xs);
		color: var(--status-fail);
	}

	.report {
		margin: var(--sp-2) 0 0;
		padding-left: 1.1em;
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	.tflag {
		display: inline-block;
		font-size: var(--text-xl);
		font-weight: var(--weight-strong);
		padding: 2px 10px;
		border-radius: var(--radius-sm);
		margin-bottom: var(--sp-2);
		border: 2px solid currentColor;
	}

	.tflag.high {
		color: var(--status-fail);
	}

	.tflag.low {
		color: var(--blue-1);
	}

	.tflag.on {
		color: var(--status-pass);
	}

	.fbar {
		position: relative;
		height: 18px;
		border-radius: 3px;
		background: var(--bg-3);
		margin: var(--sp-1) 0;
	}

	.fbar .band {
		position: absolute;
		top: 0;
		bottom: 0;
		background: var(--status-pass);
		opacity: 0.35;
	}

	.fbar .tick {
		position: absolute;
		top: -3px;
		bottom: -3px;
		width: 2px;
		margin-left: -1px;
		background: var(--status-pass);
	}

	.fbar .peak {
		position: absolute;
		top: -5px;
		bottom: -5px;
		width: 6px;
		margin-left: -3px;
		border-radius: 2px;
		background: var(--text-1);
	}

	.fbar .peak.high {
		background: var(--status-fail);
	}

	.fbar .peak.low {
		background: var(--blue-1);
	}

	.target .btn {
		margin-top: var(--sp-2);
	}

	.keys dl {
		margin: 0;
		display: grid;
		gap: 2px;
		font-size: var(--text-xs);
	}

	.keys dl div {
		display: flex;
		justify-content: space-between;
		gap: var(--sp-2);
	}

	.keys dt {
		font-weight: var(--weight-strong);
		color: var(--text-1);
	}

	.keys dd {
		margin: 0;
		color: var(--text-2);
	}

	.ask {
		margin-top: var(--sp-2);
		padding: var(--sp-2);
		border: 1px solid var(--status-warn);
		border-radius: var(--radius-sm);
		font-size: var(--text-xs);
		color: var(--text-1);
	}

	.btn.clear {
		margin-top: var(--sp-2);
	}

	.btn.clear.off {
		opacity: 0.5;
	}

	.endline .help {
		margin: var(--sp-1) 0;
		font-size: var(--text-sm);
		color: var(--text-1);
	}

	.endline .outcome {
		margin: 0;
		font-size: var(--text-md);
		font-weight: var(--weight-strong);
	}

	.endline.fail .outcome {
		color: var(--status-fail);
	}
</style>
