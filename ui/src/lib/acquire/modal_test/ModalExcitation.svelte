<script lang="ts">
	/**
	 * How the structure is excited. Impact: the trigger on the hammer row, the checks on every hit
	 * (an overload and a double hit reject it; the tip drop and a force off the target only flag
	 * it), how hits are accepted, the force window and the exponential window, and the force target.
	 * Shaker: the source, the shakers each with its level, and the ramp, ceiling, delay and overlap.
	 * The windows in samples, the exponential window's time constant and the damping it adds, the
	 * drive peaks and the time a point takes are the control service's, from its check.
	 */
	import NumberField from '../NumberField.svelte';
	import Findings from '../Findings.svelte';
	import { MODAL_SOURCES } from '$lib/protocol/modal_test';
	import { modalRows } from './modalTestSetup';
	import type { ModalTestStore } from './modalTestStore.svelte';
	import { num, SOURCE_WORDS } from './modalTestWords';

	interface Props {
		modal: ModalTestStore;
	}
	let { modal }: Props = $props();

	const d = $derived(modal.setup);
	const i = $derived(d.impact);
	const sh = $derived(d.shaker);
	const locked = $derived(modal.locked);
	const E = $derived(modal.fieldErrors);
	const W = $derived(modal.fieldWarnings);
	const errs = (k: string) => E[k] ?? [];
	const warns = (k: string) => W[k] ?? [];
	const findings = (k: string) => [...errs(k).map((message) => ({ level: 'error' as const, message })), ...warns(k).map((message) => ({ level: 'warning' as const, message }))];
	const c = $derived(modal.checkOf);
	const ci = $derived(c?.impact ?? null);
	const cs = $derived(c?.shaker ?? null);
	const hammer = $derived(modalRows(modal.rows).references[0] ?? null);
	const drives = $derived(modalRows(modal.rows).drives);
	const SOURCE_HELP: Record<string, string> = {
		random: 'Random: a new record every frame, with a Hann window and overlap. Leakage remains; many averages bring it down.',
		burst_random: 'Burst random: random for part of each frame, then silence, so the response dies out inside the frame and needs no window.',
		pseudo_random: 'Pseudo random: one frame repeated, periodic in the frame and free of leakage. With several shakers it gives a rank-one force matrix and is refused.',
		periodic_random: 'Periodic random: a new periodic record each frame, played after the delay frames so the response is steady. Leakage free and fine with several shakers.',
		periodic_chirp: 'Chirp (periodic): a sweep repeated every frame. One shaker only.',
		burst_chirp: 'Burst chirp: a sweep for part of each frame, then silence. One shaker only.'
	};

	function toggleDrive(row: number, on: boolean) {
		if (on && !sh.drives.some((x) => x.row === row)) sh.drives = [...sh.drives, { row, level_vrms: 0.05 }].sort((a, b) => a.row - b.row);
		if (!on) sh.drives = sh.drives.filter((x) => x.row !== row);
	}
</script>

<div class="stack">
	{#if d.mode === 'impact'}
		<section class="aq-panel" aria-labelledby="mt-trig" data-testid="mt-impact">
			<div class="aq-head"><h2 id="mt-trig">Trigger and Checks</h2>{#if hammer}<span class="aq-sub">On the hammer, row {hammer.index}.</span>{/if}</div>
			<div class="aq-fields">
				<NumberField id="mt-trig-lvl" label="Trigger level" unit="% of range" step={1} min={0} max={100} bind:value={i.trigger_level_pct} errors={errs('impact.trigger_level_pct')} disabled={locked} testid="mt-trigger" help={ci ? `${num(ci.trigger.level, ci.trigger.unit)} on the hammer.` : 'Of the hammer row\'s range.'} />
				<NumberField id="mt-pre" label="Pretrigger" unit="% of block" step={1} min={0} max={50} bind:value={i.pretrigger_pct} errors={errs('impact.pretrigger_pct')} warnings={warns('impact.pretrigger_pct')} disabled={locked} testid="mt-pretrigger" help={ci ? `${ci.trigger.pretrigger_frames} samples before the trigger.` : undefined} />
				<NumberField id="mt-hold" label="Holdoff" unit="blocks" integer step={1} min={0} bind:value={i.holdoff_blocks} errors={errs('impact.holdoff_blocks')} disabled={locked} testid="mt-holdoff" help="Blocks after a hit before the next can trigger." />
				<NumberField id="mt-dbl" label="Double hit over" unit="% of peak" step={1} min={0} max={100} bind:value={i.double_hit_pct} errors={errs('impact.double_hit_pct')} disabled={locked} testid="mt-double" help="A later impact over this share of the peak rejects the hit." />
				<NumberField id="mt-tip" label="Tip warning over" unit="dB" step={1} min={0} bind:value={i.tip_warn_db} errors={errs('impact.tip_warn_db')} disabled={locked} testid="mt-tip" help="The force spectrum's drop across the band that flags the tip as too soft." />
			</div>
			<div class="pol">
				<span class="lbl" id="mt-pol-l">Hammer polarity</span>
				<div class="aq-seg" role="radiogroup" aria-labelledby="mt-pol-l">
					<button type="button" role="radio" aria-checked={i.polarity === 'positive'} disabled={locked} onclick={() => (i.polarity = 'positive')} data-testid="mt-polarity-positive">Positive</button>
					<button type="button" role="radio" aria-checked={i.polarity === 'negative'} disabled={locked} onclick={() => (i.polarity = 'negative')} data-testid="mt-polarity-negative">Negative</button>
				</div>
				<p class="help" data-testid="mt-watches">{ci?.trigger.watches ? `The trigger watches ${ci.trigger.watches}.` : 'The sign of the hammer\'s force: the trigger, the peak and the double hit are judged in it. A hammer whose gauge reads negative on impact is Negative.'}</p>
			</div>
		</section>

		<section class="aq-panel" aria-labelledby="mt-acc" data-testid="mt-accept">
			<div class="aq-head"><h2 id="mt-acc">Accepting Hits</h2></div>
			<div class="aq-seg" role="radiogroup" aria-label="Accept">
				<button type="button" role="radio" aria-checked={i.accept === 'auto'} disabled={locked} onclick={() => (i.accept = 'auto')} data-testid="mt-accept-auto">Automatic</button>
				<button type="button" role="radio" aria-checked={i.accept === 'manual'} disabled={locked} onclick={() => (i.accept = 'manual')} data-testid="mt-accept-manual">Manual</button>
			</div>
			<p class="help">{i.accept === 'auto' ? 'Each hit is judged as it lands: an overload or a double hit is rejected, any other accepted. Accept or reject overrides the verdict on the latest hit until the next one lands.' : 'Each hit waits for you: Accept (Enter or A) or Reject (Backspace or R).'}</p>
			<label class="aq-switch spaced" for="mt-adv">
				<input id="mt-adv" type="checkbox" role="switch" checked={i.auto_advance} disabled={locked} onchange={(e) => (i.auto_advance = e.currentTarget.checked)} data-testid="mt-advance" />
				<span class="track" aria-hidden="true"><span class="thumb"></span></span>
				<span>Move to the next point when a point has its hits</span>
				<span class="word">{i.auto_advance ? 'On' : 'Off'}</span>
			</label>
		</section>

		<section class="aq-panel" aria-labelledby="mt-win" data-testid="mt-windows">
			<div class="aq-head"><h2 id="mt-win">Windows</h2></div>
			<label class="aq-switch" for="mt-fw">
				<input id="mt-fw" type="checkbox" role="switch" checked={i.force_window.on} disabled={locked} onchange={(e) => (i.force_window.on = e.currentTarget.checked)} data-testid="mt-fw-on" />
				<span class="track" aria-hidden="true"><span class="thumb"></span></span>
				<span>Force window</span>
				<span class="word">{i.force_window.on ? 'On' : 'Off'}</span>
			</label>
			{#if i.force_window.on}
				<div class="aq-fields spaced">
					<NumberField id="mt-fw-w" label="Width" unit="% of block" step={1} min={0} max={100} bind:value={i.force_window.width_pct} errors={errs('impact.force_window.width_pct')} disabled={locked} testid="mt-fw-width" />
					<NumberField id="mt-fw-f" label="Fall" unit="% of width" step={1} min={0} max={100} bind:value={i.force_window.fall_pct} errors={errs('impact.force_window.fall_pct')} disabled={locked} testid="mt-fw-fall" />
				</div>
				{#if ci?.force_window}<p class="aq-small derived" data-testid="mt-fw-derived">From the control service: open for {ci.force_window.pre_frames} samples before the trigger and {ci.force_window.width_frames} after, then falling to zero over {ci.force_window.fall_frames}.</p>{/if}
				<p class="help">It keeps the pulse and clears the noise after it. It does not cure a double hit: the second impact lies inside the pulse's own time, and a window short enough to cut it would cut the pulse too. A double hit is rejected and the hit taken again.</p>
			{/if}
			<label class="aq-switch spaced" for="mt-ew">
				<input id="mt-ew" type="checkbox" role="switch" checked={i.exponential_window.on} disabled={locked} onchange={(e) => (i.exponential_window.on = e.currentTarget.checked)} data-testid="mt-ew-on" />
				<span class="track" aria-hidden="true"><span class="thumb"></span></span>
				<span>Exponential window</span>
				<span class="word">{i.exponential_window.on ? 'On' : 'Off'}</span>
			</label>
			{#if i.exponential_window.on}
				<div class="aq-fields spaced">
					<NumberField id="mt-ew-e" label="Left at the block end" unit="%" step={1} min={0.1} max={100} bind:value={i.exponential_window.end_pct} errors={errs('impact.exponential_window.end_pct')} disabled={locked} testid="mt-ew-end" />
				</div>
				{#if ci?.exponential_window}<p class="aq-small derived" data-testid="mt-ew-derived">From the control service: time constant {num(ci.exponential_window.tau_s, 's')}, {num(ci.exponential_window.end * 100)}% left at the block end. It adds {ci.exponential_window.added_damping} to every damping ratio.</p>{/if}
			{/if}
			<Findings items={findings('impact.exponential_window')} testid="mt-ew-findings" />
		</section>

		<section class="aq-panel" aria-labelledby="mt-tgt-set" data-testid="mt-target-setup">
			<div class="aq-head"><h2 id="mt-tgt-set">Force Target</h2></div>
			<label class="aq-switch" for="mt-tgt-on">
				<input id="mt-tgt-on" type="checkbox" role="switch" checked={i.target.on} disabled={locked} onchange={(e) => (i.target.on = e.currentTarget.checked)} data-testid="mt-target-on" />
				<span class="track" aria-hidden="true"><span class="thumb"></span></span>
				<span>Flag each hit against a target peak</span>
				<span class="word">{i.target.on ? 'On' : 'Off'}</span>
			</label>
			{#if i.target.on}
				<div class="aq-fields spaced">
					<NumberField id="mt-tgt-pk" label="Target peak" unit={hammer?.units ?? ''} step={1} min={0} bind:value={i.target.peak} errors={errs('impact.target.peak')} disabled={locked} testid="mt-target-peak" />
					<NumberField id="mt-tgt-tol" label="Within" unit="%" step={1} min={0} max={100} bind:value={i.target.tolerance_pct} errors={errs('impact.target.tolerance_pct')} disabled={locked} testid="mt-target-tol" />
				</div>
			{/if}
			<p class="help">A hit above or below the band is flagged High or Low with its percent; it is never rejected for it. On the Measure screen, Use This Hit as the Target sets the target from a hit you like.</p>
		</section>
	{:else}
		<section class="aq-panel" aria-labelledby="mt-src" data-testid="mt-shaker">
			<div class="aq-head"><h2 id="mt-src">Source</h2></div>
			<div class="sel">
				<label for="mt-source" class="lbl">Source</label>
				<select id="mt-source" class="field" bind:value={sh.source} disabled={locked} data-testid="mt-source">
					{#each MODAL_SOURCES as s (s)}<option value={s}>{SOURCE_WORDS[s]}</option>{/each}
				</select>
			</div>
			<p class="help">{SOURCE_HELP[sh.source]}</p>
			<Findings items={findings('shaker.source')} testid="mt-source-findings" />
			<div class="aq-fields spaced">
				{#if sh.source === 'burst_random' || sh.source === 'burst_chirp'}<NumberField id="mt-burst" label="Burst" unit="% of frame" step={5} min={1} max={100} bind:value={sh.burst_pct} errors={errs('shaker.burst_pct')} disabled={locked} testid="mt-burst" />{/if}
				{#if sh.source === 'random'}<NumberField id="mt-ovl" label="Overlap" unit="%" step={5} min={0} max={90} bind:value={sh.overlap_pct} errors={errs('shaker.overlap_pct')} disabled={locked} testid="mt-overlap" />{/if}
				{#if sh.source === 'pseudo_random' || sh.source === 'periodic_random' || sh.source === 'periodic_chirp'}<NumberField id="mt-delay" label="Delay" unit="frames" integer step={1} min={0} max={100} bind:value={sh.delay_frames} errors={errs('shaker.delay_frames')} disabled={locked} testid="mt-delay" help="Frames played before averaging, so the response is steady." />{/if}
				<NumberField id="mt-ramp" label="Ramp up" unit="s" step={0.5} min={0} max={60} bind:value={sh.ramp_s} errors={errs('shaker.ramp_s')} disabled={locked} testid="mt-ramp" />
				<NumberField id="mt-ceil" label="Drive ceiling" unit="V" step={0.1} min={0} max={10} bind:value={sh.ceiling_v} errors={errs('shaker.ceiling_v')} disabled={locked} testid="mt-ceiling" />
				<NumberField id="mt-seed" label="Seed" integer step={1} min={0} bind:value={sh.seed} errors={errs('shaker.seed')} disabled={locked} testid="mt-seed" help="The same seed plays the same drive again." />
			</div>
		</section>

		<section class="aq-panel" aria-labelledby="mt-shakers" data-testid="mt-shakers">
			<div class="aq-head"><h2 id="mt-shakers">Shakers</h2><span class="aq-sub">One drive row for each shaker, each with its own force reference.</span></div>
			{#if drives.length}
				<table class="aq-table">
					<thead><tr><th>Drive</th><th class="num">Row</th><th>Description</th><th>Level</th><th class="num">Peak</th></tr></thead>
					<tbody>
						{#each drives as r (r.index)}
							{@const on = sh.drives.find((x) => x.row === r.index)}
							{@const k = sh.drives.findIndex((x) => x.row === r.index)}
							<tr>
								<td><input type="checkbox" aria-label="Drive row {r.index}" checked={!!on} disabled={locked} onchange={(e) => toggleDrive(r.index, e.currentTarget.checked)} data-testid="mt-drive-row" data-row={r.index} /></td>
								<td class="num">{r.index}</td>
								<td>{r.description}</td>
								<td>{#if on}<NumberField id="mt-lvl-{r.index}" label="Row {r.index} level" compact unit="Vrms" step={0.01} min={0} bind:value={on.level_vrms} errors={errs(`shaker.drives.${k}.level_vrms`)} disabled={locked} testid="mt-drive-level" />{/if}</td>
								<td class="num">{cs?.drives.find((x) => x.row === r.index) ? num(cs.drives.find((x) => x.row === r.index)!.peak_v, 'V') : ''}</td>
							</tr>
						{/each}
					</tbody>
				</table>
			{:else}
				<p class="aq-small aq-muted">No drive row yet: add one for each shaker in Station &amp; Channels.</p>
			{/if}
			<Findings items={findings('shaker.drives')} testid="mt-drives-findings" />
			{#if cs}<p class="aq-small derived" data-testid="mt-shaker-derived">From the control service: crest factor {num(cs.crest)}, {cs.window === 'hann' ? 'a Hann window' : 'no window'}, about {num(cs.seconds_per_point, 's')} a point. Stop ramps the drives {num(cs.stop.depth_db, 'dB')} down over {num(cs.stop.ramp_s, 's')}; Abort takes them to zero at once.</p>{/if}
		</section>
	{/if}
</div>

<style>
	.pol {
		margin-top: var(--sp-3);
	}

	.pol .lbl {
		display: block;
		margin-bottom: var(--sp-1);
	}

	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		max-width: 860px;
	}

	.help {
		font-size: var(--text-xs);
		color: var(--text-3);
		margin: var(--sp-1) 0 0;
	}

	.spaced {
		margin-top: var(--sp-2);
	}

	.derived {
		margin: var(--sp-2) 0 0;
	}
</style>
