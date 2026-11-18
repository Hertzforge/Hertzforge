<script lang="ts">
	/**
	 * The MIMO Sine setup in a few lines, beside every section: the channels, the references, the
	 * test, the schedule and the law; the service's verdict on the setup (the shape, the grid, the
	 * test's length, the tracking, each control's peaks); every refusal and warning the service gave,
	 * by name in plain words with its own message; and what is left to fix, each a click from its
	 * section.
	 */
	import { formatNumber } from '$lib/viz/format';
	import { clock, db } from '../words';
	import { MSINE_SECTIONS, msineItemKey, msineSectionOf, msineTitleOf, TEST_KINDS, type MsineSection } from './msineSetup';
	import type { MimoSineStore } from './msineStore.svelte';
	import { reasonWords } from './msineWords';

	interface Props {
		ms: MimoSineStore;
		onopen: (s: MsineSection) => void;
	}
	let { ms, onopen }: Props = $props();

	const d = $derived(ms.setup);
	const dry = $derived(ms.check && ms.checkedKey === ms.key ? ms.check.derived : null);
	const todo = $derived(MSINE_SECTIONS.filter((s) => ms.counts[s.id].errors > 0));
	const levels = $derived(d.schedule.levels.filter(Number.isFinite));
	const findings = $derived(ms.findings);
	const SHAPE: Record<string, string> = { square: 'Square', more_controls_than_drives: 'More controls than drives', fewer_controls_than_drives: 'Fewer controls than drives' };
	const sectionOf = (it: { field?: string | null; channel?: number | null; row?: number; reason: string }) => msineSectionOf(msineItemKey(it as never, d));
</script>

<aside class="aq-panel summary" aria-label="Setup summary" data-testid="msine-summary">
	<h2>Summary</h2>
	<dl>
		<div><dt>Title</dt><dd>{msineTitleOf(d)}</dd></div>
		<div><dt>Channels</dt><dd>{d.controls.length} {d.controls.length === 1 ? 'control' : 'controls'}, {d.drives.length} {d.drives.length === 1 ? 'drive' : 'drives'}{d.limits.channels.length ? `, ${d.limits.channels.length} limit` : ''}</dd></div>
		<div><dt>References</dt><dd>{d.phase_specified ? 'Amplitude and phase per control' : 'Amplitudes only'}</dd></div>
		<div><dt>Test</dt><dd>{TEST_KINDS.find((k) => k.id === d.test.kind)?.label}</dd></div>
		<div><dt>Schedule</dt><dd>{levels.length} {levels.length === 1 ? 'level' : 'levels'} from {db(levels[0])}, {d.schedule.mode === 'auto' ? 'automatic' : 'by hand'}</dd></div>
		<div><dt>Control law</dt><dd data-testid="msine-summary-law">{ms.law ? `${ms.law.name} ${ms.law.version}` : '-'}</dd></div>
		<div><dt>Requests</dt><dd>{d.requests.honour ? 'Acted on' : 'Shown only'}</dd></div>
	</dl>

	{#if dry}
		<h3>From the Control Service</h3>
		<dl data-testid="msine-summary-derived">
			<div><dt>Shape</dt><dd>{SHAPE[dry.shape] ?? dry.shape}</dd></div>
			<div><dt>Band</dt><dd class="num">{formatNumber(dry.band_hz[0], 4)} to {formatNumber(dry.band_hz[1], 4)} Hz</dd></div>
			<div><dt>Grid</dt><dd class="num">{formatNumber(dry.grid.fs_hz, 6)} Hz, a tick {formatNumber(dry.grid.tick_s * 1000, 3)} ms{dry.grid.actual ? '' : ', until the station arms'}</dd></div>
			{#if dry.test_s !== null}<div><dt>Test</dt><dd class="num">{clock(dry.test_s)}</dd></div>{/if}
			<div><dt>Identification</dt><dd class="num">{clock(dry.identification_s)}</dd></div>
			<div><dt>Tracking</dt><dd>{dry.tracking.verdict === 'ok' ? 'Keeps up with the sweep' : (dry.tracking.message ?? dry.tracking.verdict)}</dd></div>
			{#each dry.profiles as p (p.channel)}
				<div><dt>Control {p.channel}</dt><dd class="num">{formatNumber(p.peak_accel_g, 4)} g peak at full level</dd></div>
			{/each}
			<div><dt>Feasibility</dt><dd>{dry.feasibility.message}</dd></div>
		</dl>
	{:else if ms.checking}
		<p class="aq-muted aq-small">Checking the setup.</p>
	{/if}

	{#if findings.errors.length || findings.warnings.length}
		<h3>The Control Service Says</h3>
		<ul class="said" data-testid="msine-refusals">
			{#each findings.errors as f, i (i)}
				<li class="error" data-reason={f.reason}>
					<button class="link" onclick={() => onopen(sectionOf(f))}><strong>{reasonWords(f.reason)}.</strong></button>
					<span>{f.message}</span>
				</li>
			{/each}
			{#each findings.warnings as f, i (i)}
				<li class="warning" data-reason={f.reason}>
					<button class="link" onclick={() => onopen(sectionOf(f))}><strong>{reasonWords(f.reason)}.</strong></button>
					<span>{f.message}</span>
				</li>
			{/each}
		</ul>
	{/if}

	<div class="ready" data-testid="msine-ready" data-ready={todo.length === 0 && !!dry}>
		{#if todo.length}
			<h3>To Fix</h3>
			<ul>
				{#each todo as s (s.id)}
					<li><button class="btn ghost sm" onclick={() => onopen(s.id)}>{s.label}: {ms.counts[s.id].errors} {ms.counts[s.id].errors === 1 ? 'error' : 'errors'}</button></li>
				{/each}
			</ul>
		{:else if dry}
			<p class="aq-small ok">
				<svg viewBox="0 0 14 14" width="12" height="12" aria-hidden="true"><path d="M2.5 7.3l2.8 2.8 6-6.3" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" /></svg>
				Nothing missing.
			</p>
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
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
	}

	h3 {
		margin: var(--sp-3) 0 var(--sp-2);
		font-size: var(--text-xs);
		font-weight: var(--weight-strong);
		color: var(--text-2);
	}

	dl {
		display: flex;
		flex-direction: column;
		gap: 6px;
		margin: 0;
	}

	dl div {
		display: grid;
		grid-template-columns: 110px minmax(0, 1fr);
		gap: var(--sp-2);
		font-size: var(--text-xs);
	}

	dt {
		color: var(--text-3);
	}

	dd {
		margin: 0;
		color: var(--text-1);
	}

	ul {
		margin: 0;
		padding: 0;
		list-style: none;
	}

	.said {
		display: flex;
		flex-direction: column;
		gap: 6px;
		font-size: var(--text-xs);
	}

	.said li {
		padding-left: 8px;
		border-left: 3px solid var(--line-1);
	}

	.said li.error {
		border-left-color: var(--red-2, var(--amber-2));
	}

	.said li.warning {
		border-left-color: var(--amber-2);
	}

	.link {
		padding: 0;
		margin-right: 4px;
		color: inherit;
		background: none;
		border: 0;
		text-align: left;
		cursor: pointer;
	}

	.ok {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		color: var(--blue-1);
	}
</style>
