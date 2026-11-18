<script lang="ts">
	/**
	 * How the run builds the drive: the control law, chosen from the catalog's MIMO Sine laws that
	 * accept the setup, with the parameters its own manifest declares, its check record and its
	 * stated limit; the laws that do not accept the setup, each with why; then the tracking filter,
	 * the guards the controller holds every law to, and the switch for a law's requests to hold or
	 * slow the sweep. Nothing here names a law but the stated limits of the built-in ones.
	 */
	import LawPanel from '../law/LawPanel.svelte';
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import { defaultMsineSetup, TEST_KINDS } from './msineSetup';
	import type { MimoSineStore } from './msineStore.svelte';
	import { LAW_LIMITS } from './msineWords';

	interface Props {
		ms: MimoSineStore;
	}
	let { ms }: Props = $props();

	const d = $derived(ms.setup);
	const c = $derived(d.control);
	const locked = $derived(ms.locked);
	const E = $derived(ms.fieldErrors);
	const errs = (k: string) => E[k] ?? [];
	const DEF = defaultMsineSetup();
	const kindWord = $derived(TEST_KINDS.find((k) => k.id === d.test.kind)?.label.toLowerCase() ?? '');

	type Key = 'max_step_db' | 'max_rise_db_per_s' | 'divergence_s' | 'abort_ticks' | 'law_budget' | 'law_misses';
	const GUARDS: { key: Key; label: string; unit?: string; integer?: boolean; step: number; min?: number; max?: number; help: string }[] = [
		{ key: 'max_step_db', label: "A drive's change a tick", unit: 'dB', step: 0.5, min: 1, max: 3, help: "The most each drive's amplitude may change in one tick, either way." },
		{ key: 'max_rise_db_per_s', label: "A drive's rise a second", unit: 'dB/s', step: 10, min: 0, max: 10000, help: "The most each drive's amplitude may rise in a second." },
		{ key: 'divergence_s', label: 'A guard bound without a break', unit: 's', step: 0.5, min: 0, max: 60, help: 'A guard that binds this long without a break takes the controlled stop.' },
		{ key: 'abort_ticks', label: 'Ticks outside an abort band', integer: true, step: 1, min: 1, max: 100, help: 'Ticks in a row a control may be outside its abort band, outside a settle window, before the run aborts.' },
		{ key: 'law_budget', label: 'Time a call, share of the tick', step: 0.05, min: 0, max: 1, help: 'A call that takes longer is a miss; the drive in force is held.' },
		{ key: 'law_misses', label: 'Misses before a stop', integer: true, step: 1, min: 1, max: 100, help: 'Unusable answers in a row (late, failed or refused) that end the run with the controlled stop.' }
	];

	function usual() {
		if (locked) return;
		for (const g of GUARDS) c[g.key] = DEF.control[g.key];
		c.rotation_cap_deg = DEF.control.rotation_cap_deg;
	}

	const typeWords = (t: string) => TEST_KINDS.find((k) => k.id === t)?.label ?? t;
</script>

<div class="stack">
	<LawPanel
		laws={ms.offeredLaws}
		law={ms.law}
		chosenId={d.law.id}
		values={d.law.params}
		subtitle="Fixed for the run. Listed: the laws that accept a {kindWord} on {d.controls.length} {d.controls.length === 1 ? 'control' : 'controls'} and {d.drives.length} {d.drives.length === 1 ? 'drive' : 'drives'}{d.phase_specified ? '' : ' with amplitudes only'}."
		{locked}
		connected={ms.connected}
		busy={ms.busy}
		problems={ms.lawProblems}
		errors={E}
		channelLabels={d.controls.map((x) => `Control ${x.channel}`)}
		onchoose={(id) => ms.chooseLaw(id)}
		onimport={(path) => ms.importLaw(path)}
	>
		{#snippet facts(entry)}
			{@const e = entry as { test_types?: string[]; amplitude_only?: boolean }}
			{#if e.test_types}<div><dt>Tests</dt><dd data-testid="msine-law-tests">{e.test_types.map(typeWords).join(', ')}</dd></div>{/if}
			{#if e.amplitude_only !== undefined}<div><dt>Amplitudes only</dt><dd>{e.amplitude_only ? 'Accepted: it chooses the phases' : 'Needs a phase on every reference'}</dd></div>{/if}
		{/snippet}
		{#snippet notice(entry)}
			{#if LAW_LIMITS[entry.id] && entry.builtin}
				<div class="limit" data-testid="msine-law-limit">
					<h4>Its Limit</h4>
					<p class="aq-small">{LAW_LIMITS[entry.id]}</p>
				</div>
			{/if}
		{/snippet}
	</LawPanel>

	{#if ms.refusedLaws.length}
		<section class="aq-panel" aria-labelledby="msine-refused-title" data-testid="msine-refused-laws">
			<div class="aq-head"><h2 id="msine-refused-title">Not Offered for This Setup</h2></div>
			<ul class="refused">
				{#each ms.refusedLaws as r (r.law.id)}
					<li data-testid="msine-refused-{r.law.id}"><span class="name">{r.law.name}</span> <span class="aq-small">{r.why}</span></li>
				{/each}
			</ul>
		</section>
	{/if}

	<section class="aq-panel" aria-labelledby="msine-track-title" data-testid="msine-tracking">
		<div class="aq-head"><h2 id="msine-track-title">Tracking</h2><span class="aq-sub">Every control and limit channel is tracked against the one tone's phase.</span></div>
		<div class="aq-fields">
			<div class="tf">
				<label for="msine-track-kind" class="lbl">Filter bandwidth</label>
				<select id="msine-track-kind" class="field" bind:value={c.tracking.kind} disabled={locked}><option value="proportional">A share of the frequency</option><option value="fixed">Fixed</option></select>
			</div>
			{#if c.tracking.kind === 'fixed'}
				<NumberField id="msine-track-hz" label="Bandwidth" unit="Hz" step={1} bind:value={c.tracking.hz} disabled={locked} />
			{:else}
				<NumberField id="msine-track-pct" label="Bandwidth" unit="%" step={1} bind:value={c.tracking.pct} disabled={locked} />
				<NumberField id="msine-track-max" label="At most" unit="Hz" step={10} bind:value={c.tracking.max_hz} disabled={locked} />
			{/if}
		</div>
		<Findings items={[...errs('control.tracking').map((message) => ({ level: 'error' as const, message })), ...(ms.fieldWarnings['control.tracking'] ?? []).map((message) => ({ level: 'warning' as const, message }))]} />
	</section>

	<section class="aq-panel" aria-labelledby="msine-gd-title" data-testid="msine-guards">
		<div class="aq-head">
			<h2 id="msine-gd-title">Guards on Any Law</h2>
			<span class="aq-sub">Held whatever the law answers.</span>
			<span class="aq-spacer"></span>
			<button class="btn sm ghost" disabled={locked} onclick={usual}>Usual Values</button>
		</div>
		<div class="aq-fields">
			{#each GUARDS as g (g.key)}
				<NumberField id="msine-{g.key}" label={g.label} unit={g.unit} integer={g.integer} step={g.step} min={g.min} max={g.max} value={c[g.key]} onchange={(v) => (c[g.key] = v ?? NaN)} errors={errs(`control.${g.key}`)} help="{g.help} Usual: {DEF.control[g.key]}." disabled={locked} testid="msine-{g.key}" />
			{/each}
			<NumberField id="msine-rotation" label="A drive's turn a tick" unit="deg" nullable="No cap" fallback={30} step={5} min={0} max={180} bind:value={c.rotation_cap_deg} errors={errs('control.rotation_cap_deg')} help="The most each drive's phase may turn in one tick. The largest turn the law asks for is reported either way." disabled={locked} testid="msine-rotation" />
		</div>
	</section>

	<section class="aq-panel" aria-labelledby="msine-req-title" data-testid="msine-requests">
		<div class="aq-head"><h2 id="msine-req-title">The Law's Requests</h2><span class="aq-sub">A law may ask to hold or slow the sweep. Every request is shown either way.</span></div>
		<label class="aq-switch" for="msine-honour">
			<input id="msine-honour" type="checkbox" role="switch" bind:checked={d.requests.honour} disabled={locked} data-testid="msine-honour" />
			<span class="track" aria-hidden="true"><span class="thumb"></span></span>
			<span>Act on the law's requests</span>
			<span class="word">{d.requests.honour ? 'On' : 'Off: shown, not acted on'}</span>
		</label>
		{#if d.requests.honour}
			<div class="aq-fields">
				<NumberField id="msine-hold" label="A hold at most" unit="s" step={0.5} min={0} max={2} bind:value={d.requests.max_hold_s} errors={errs('requests.max_hold_s')} help="A granted hold freezes the schedule's clock; the test grows by it." disabled={locked} />
				<NumberField id="msine-slow" label="A slow-down to at least" step={0.05} min={0.1} max={1} bind:value={d.requests.min_slow_factor} errors={errs('requests.min_slow_factor')} help="Of the scheduled sweep rate." disabled={locked} />
			</div>
		{/if}
	</section>
</div>

<style>
	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
	}

	.limit {
		margin-top: var(--sp-2);
		padding: var(--sp-2) var(--sp-3);
		background: var(--bg-2);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-md);
	}

	.limit h4 {
		margin: 0 0 4px;
		font-size: var(--text-xs);
		font-weight: var(--weight-strong);
	}

	.limit p {
		margin: 0;
	}

	.refused {
		display: flex;
		flex-direction: column;
		gap: 4px;
		margin: 0;
		padding: 0;
		list-style: none;
	}

	.refused .name {
		font-weight: var(--weight-strong);
	}

	.tf {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.lbl {
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}
</style>
