<script lang="ts">
	/**
	 * What the run is held inside besides each control's own bands: limit channels, each with its
	 * bound against frequency (the whole drive vector is scaled down, its direction kept, where a
	 * limit channel would pass it: the notch); how the notch is smoothed and released; and the
	 * shakers' ratings, one record per drive, for the feasibility check and the kinematic monitor.
	 */
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import SineProfileTable from '../sine/SineProfileTable.svelte';
	import { limitRows, MAX_LIMITS } from './msineSetup';
	import type { MimoSineStore } from './msineStore.svelte';

	interface Props {
		ms: MimoSineStore;
	}
	let { ms }: Props = $props();

	const d = $derived(ms.setup);
	const lim = $derived(d.limits);
	const locked = $derived(ms.locked);
	const E = $derived(ms.fieldErrors);
	const W = $derived(ms.fieldWarnings);
	const errs = (k: string) => E[k] ?? [];
	const free = $derived(limitRows(d, ms.rows).filter((r) => !lim.channels.some((c) => c.channel === r.index)));
	let pick = $state<number | null>(null);
	const MASSES = [
		['armature', 'Armature'],
		['slip_table', 'Slip table'],
		['drive_adapter', 'Head or adapter'],
		['fixturing', 'Fixturing'],
		['test_article', 'Test article']
	] as const;
</script>

<div class="stack">
	<section class="aq-panel" aria-labelledby="msine-lim-title" data-testid="msine-limits">
		<div class="aq-head">
			<h2 id="msine-lim-title">Limit Channels</h2>
			<span class="aq-sub">Watched, not controlled: up to {MAX_LIMITS}, each an enabled input that is not a control.</span>
		</div>
		<p class="aq-lead aq-small">Where a limit channel's predicted or measured response would pass its bound, the whole drive vector is scaled down with its direction kept, and every control falls with it; the run shows the scale and the depth at every control. A limit channel outside its own abort band ends the run.</p>
		{#each lim.channels as ch, i (ch.channel)}
			<div class="aq-group" data-testid="msine-limit-{ch.channel}">
				<div class="row">
					<h3>Channel {ch.channel} {ms.rows.find((r) => r.index === ch.channel)?.description ?? ''}</h3>
					<button class="btn sm ghost" disabled={locked} onclick={() => ms.removeLimit(ch.channel)}>Remove</button>
				</div>
				<div class="aq-fields">
					<NumberField id="msine-lim-ab-lo-{i}" label="Abort below" unit="dB" step={0.5} min={0} bind:value={ch.profile.abort_db[0]} disabled={locked} />
					<NumberField id="msine-lim-ab-hi-{i}" label="Abort above" unit="dB" step={0.5} min={0} bind:value={ch.profile.abort_db[1]} disabled={locked} />
				</div>
				<SineProfileTable rows={ch.profile.rows} prefix="limits.channels.{i}.profile.rows" errors={E} warnings={W} {locked} testid="msine-bound-{ch.channel}" label="Channel {ch.channel} bound" valueWord="Bound" bands={false} />
				<Findings items={errs(`limits.channels.${i}.channel`).map((message) => ({ level: 'error' as const, message }))} />
			</div>
		{/each}
		{#if free.length && lim.channels.length < MAX_LIMITS}
			<div class="add">
				<label for="msine-lim-pick" class="lbl">Add a limit channel</label>
				<select id="msine-lim-pick" class="field" bind:value={pick} disabled={locked} data-testid="msine-limit-pick">
					<option value={null}>Choose a channel</option>
					{#each free as r (r.index)}<option value={r.index}>Channel {r.index}{r.description ? `, ${r.description}` : ''}</option>{/each}
				</select>
				<button class="btn sm" disabled={locked || pick === null} onclick={() => pick !== null && ms.addLimit(pick) && (pick = null)} data-testid="msine-limit-add">Add</button>
			</div>
		{:else if !lim.channels.length}
			<p class="aq-small aq-muted">No input can be a limit channel: switch on a monitor row in the channel table, or take a control row out of the controls.</p>
		{/if}
		<Findings items={errs('limits.channels').map((message) => ({ level: 'error' as const, message }))} />

		<div class="aq-group">
			<h3>The Notch</h3>
			<div class="aq-fields">
				<NumberField id="msine-notch-smooth" label="Smoothed over" unit="ticks" integer step={1} min={1} bind:value={lim.notch.smoothing_ticks} errors={errs('limits.notch.smoothing_ticks')} disabled={locked} />
				<NumberField id="msine-notch-release" label="Released at most" unit="dB/s" step={5} min={0} bind:value={lim.notch.release_ramp_db_per_s} errors={errs('limits.notch.release_ramp_db_per_s')} disabled={locked} />
				<NumberField id="msine-notch-hysteresis" label="Released once allowed by" unit="dB" step={0.5} min={0} max={6} bind:value={lim.notch.release_hysteresis_db} errors={errs('limits.notch.release_hysteresis_db')} help="How far the limit readings must allow the notch to rise before it begins to, so a reading near its bound neither toggles the notch nor releases it with a kick." disabled={locked} testid="msine-notch-hysteresis" />
				<NumberField id="msine-notch-near" label="Near the bound within" unit="dB" step={0.5} min={0} max={20} bind:value={lim.notch.near_bound_db} errors={errs('limits.notch.near_bound_db')} help="How near its bound a limit channel must be for a notched reference a law returns to be taken." disabled={locked} />
			</div>
		</div>
	</section>

	<section class="aq-panel" aria-labelledby="msine-feas-title" data-testid="msine-feasibility">
		<div class="aq-head"><h2 id="msine-feas-title">Shaker Ratings</h2><span class="aq-sub">Optional. Without them feasibility is not checked and the kinematic monitor is off.</span></div>
		<label class="aq-switch" for="msine-feas-on">
			<input id="msine-feas-on" type="checkbox" role="switch" bind:checked={d.feasibility.enabled} disabled={locked} data-testid="msine-feasibility-on" />
			<span class="track" aria-hidden="true"><span class="thumb"></span></span>
			<span>Check every shaker against the references</span>
			<span class="word">{d.feasibility.enabled ? 'On' : 'Off'}</span>
		</label>
		{#if d.feasibility.enabled}
			<label class="aq-switch" for="msine-kin">
				<input id="msine-kin" type="checkbox" role="switch" bind:checked={lim.kinematic_monitor} disabled={locked} />
				<span class="track" aria-hidden="true"><span class="thumb"></span></span>
				<span>Abort when a shaker passes its velocity or displacement rating</span>
				<span class="word">{lim.kinematic_monitor ? 'On' : 'Off'}</span>
			</label>
			{#each d.feasibility.shakers as s, i (s.drive)}
				<div class="aq-group">
					<h3>The Shaker on Drive {s.drive}</h3>
					<label for="msine-shaker-{i}" class="lbl">Shaker record (JSON)</label>
					<textarea id="msine-shaker-{i}" class="field mono" rows="3" bind:value={s.shaker_text} disabled={locked}></textarea>
					<div class="aq-fields">
						{#each MASSES as [k, label] (k)}
							<NumberField id="msine-mass-{i}-{k}" label={label} unit="lb" step={1} min={0} bind:value={s.moving_mass_lb[k]} disabled={locked} />
						{/each}
					</div>
					<Findings items={errs(`feasibility.shakers.${i}`).map((message) => ({ level: 'error' as const, message }))} />
				</div>
			{/each}
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

	.row {
		display: flex;
		align-items: center;
		justify-content: space-between;
	}

	.add {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		margin: var(--sp-2) 0;
	}

	.lbl {
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}

	.mono {
		font-family: var(--font-mono);
		width: 100%;
	}
</style>
