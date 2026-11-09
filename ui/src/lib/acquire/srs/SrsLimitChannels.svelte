<script lang="ts">
	/**
	 * Inputs whose response must stay under a bound (a fixture's resonance, a sensitive part of the
	 * article): each channel's bound on its SRS (one level, or one per band), and the gain from the
	 * drive to it, supplied here or measured by identification. Before the first pulse the control
	 * service notches the drive where a prediction would pass a bound, and before each pulse caps
	 * every component by what the prediction leaves; once the run has planned the notch, what it did
	 * is shown here and on the run.
	 */
	import { formatNumber } from '$lib/viz/format';
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import { rowName } from '../mimo/mimoSetup';
	import PointsTable from './PointsTable.svelte';
	import { newLimitChannel } from './srsSetup';
	import type { SrsStore } from './srsStore.svelte';
	import { notchWords } from './srsWords';

	interface Props {
		srs: SrsStore;
	}
	let { srs }: Props = $props();

	const lc = $derived(srs.setup.limit_channels);
	const locked = $derived(srs.locked);
	const E = $derived(srs.fieldErrors);
	const errs = (k: string) => E[k] ?? [];
	const rows = $derived(srs.rows.filter((r) => r.enabled && r.role !== 'drive' && r.role !== 'control'));
	const bands = $derived(srs.bandsHz);
	const notch = $derived(srs.status?.notch ?? null);

	function add() {
		lc.channels.push(newLimitChannel(rows.find((r) => !lc.channels.some((c) => c.channel === r.index)) ?? rows[0]));
	}
</script>

<div class="cols">
	<section class="aq-panel" aria-labelledby="rlc-title" data-testid="srs-limit-channels">
		<div class="aq-head">
			<h2 id="rlc-title">Limit Channels</h2>
			<span class="aq-sub">Responses that must stay under a bound, whatever the control channel asks for.</span>
		</div>
		<label class="aq-switch" for="rlc-on">
			<input id="rlc-on" type="checkbox" role="switch" checked={lc.enabled} disabled={locked} onchange={(e) => (lc.enabled = e.currentTarget.checked)} data-testid="srs-limit-on" />
			<span class="track" aria-hidden="true"><span class="thumb"></span></span>
			<span>Hold channels under a bound</span>
			<span class="word">{lc.enabled ? 'On' : 'Off'}</span>
		</label>
		{#if lc.enabled}
			<div class="aq-fields spaced">
				<NumberField id="rlc-hr" label="Headroom" unit="dB" step={0.5} min={0} bind:value={lc.headroom_db} errors={errs('limit_channels.headroom_db')} disabled={locked} testid="srs-limit-headroom" help="How far under its bound each prediction must stay. Usual: 1 dB." />
			</div>
			<p class="aq-lead aq-small">A limit beats the control channel: where a channel's prediction would pass its bound, the drive is notched there, and the control SRS sits under the target in those bands by what the notch takes.</p>
			{#each lc.channels as ch, i (i)}
				<div class="chan" data-testid="srs-limit-{i}">
					<div class="chead">
						<label class="lbl" for="rlc-{i}-ch">Channel</label>
						<select id="rlc-{i}-ch" class="field" value={ch.channel} disabled={locked} onchange={(e) => (ch.channel = Number(e.currentTarget.value))} data-testid="srs-limit-{i}-channel">
							{#if !rows.some((r) => r.index === ch.channel)}<option value={ch.channel}>Channel {ch.channel}</option>{/if}
							{#each rows as r (r.index)}<option value={r.index}>{r.index}: {rowName(r)}</option>{/each}
						</select>
						<span class="aq-spacer"></span>
						<button class="btn ghost sm" disabled={locked} onclick={() => lc.channels.splice(i, 1)} data-testid="srs-limit-{i}-remove">Remove</button>
					</div>
					<Findings items={errs(`limit_channels.channels.${i}.channel`).map((message) => ({ level: 'error' as const, message }))} />

					<div class="aq-group">
						<h3>Bound</h3>
						<div class="aq-seg" role="radiogroup" aria-label="Channel {ch.channel} bound">
							<button type="button" role="radio" aria-checked={ch.bound.kind === 'flat'} disabled={locked} onclick={() => srs.setBoundKind(i, 'flat')} data-testid="srs-limit-{i}-flat">One level</button>
							<button type="button" role="radio" aria-checked={ch.bound.kind === 'per_band'} disabled={locked || (ch.bound.kind !== 'per_band' && !bands)} onclick={() => srs.setBoundKind(i, 'per_band')} data-testid="srs-limit-{i}-per-band">Per band</button>
						</div>
						{#if ch.bound.kind === 'flat'}
							<div class="aq-fields spaced">
								<NumberField id="rlc-{i}-b" label="Bound on its SRS" unit="g" step={5} min={0} value={ch.bound.accel_g} onchange={(v) => (ch.bound.accel_g = v ?? NaN)} errors={errs(`limit_channels.channels.${i}.bound`)} disabled={locked} testid="srs-limit-{i}-bound" />
							</div>
						{:else}
							{#if bands && ch.bound.bounds_g.length !== bands.length}
								<p class="aq-notice warn aq-small" role="alert">The band grid has changed to {bands.length} bands; this bound has {ch.bound.bounds_g.length}. <button class="btn sm" disabled={locked} onclick={() => srs.fitPerBand()}>Fit to the Grid</button></p>
							{/if}
							<table class="aq-table spaced" data-testid="srs-limit-{i}-bounds">
								<thead><tr><th>Band</th><th class="r">Bound, g</th></tr></thead>
								<tbody>
									{#each ch.bound.bounds_g as g, k (k)}
										<tr>
											<td class="num">{bands?.[k] !== undefined ? `${formatNumber(bands[k], 4)} Hz` : `Band ${k + 1}`}</td>
											<td class="num"><NumberField id="rlc-{i}-b{k}" label="Band {k + 1} bound" compact nullable="No bound" fallback={Number.isFinite(ch.bound.accel_g) ? ch.bound.accel_g : 50} step={5} min={0} value={g} onchange={(v) => (ch.bound.bounds_g[k] = v)} disabled={locked} /></td>
										</tr>
									{/each}
								</tbody>
							</table>
							<Findings items={errs(`limit_channels.channels.${i}.bound`).map((message) => ({ level: 'error' as const, message }))} />
						{/if}
					</div>

					<div class="aq-group">
						<h3>Gain From the Drive</h3>
						<label class="aq-switch" for="rlc-{i}-frf">
							<input id="rlc-{i}-frf" type="checkbox" role="switch" checked={ch.frf.supplied} disabled={locked} onchange={(e) => (ch.frf.supplied = e.currentTarget.checked)} data-testid="srs-limit-{i}-frf-on" />
							<span class="track" aria-hidden="true"><span class="thumb"></span></span>
							<span>Supply the gain</span>
							<span class="word">{ch.frf.supplied ? 'On' : 'Off'}</span>
						</label>
						{#if ch.frf.supplied}
							<p class="aq-lead aq-small">The gain from the drive to this channel, g per volt, frequency rising. The prediction uses it in place of a measurement.</p>
							<PointsTable
								rows={ch.frf.points}
								columns={[
									{ key: 'freq_hz', label: 'Frequency', unit: 'Hz', step: 10 },
									{ key: 'mag', label: 'Gain', unit: 'g/V', step: 0.5 }
								]}
								prefix="limit_channels.channels.{i}.frf.points"
								errors={E}
								{locked}
								make={(last) => ({ freq_hz: last ? Math.round(last.freq_hz * 2) : 100, mag: last?.mag ?? 1 })}
								testid="srs-limit-{i}-frf"
								label="Channel {ch.channel} gain"
							/>
						{:else}
							<p class="aq-small aq-muted">Off: identification measures the gain to this channel along with the control channel's.</p>
						{/if}
						<Findings items={errs(`limit_channels.channels.${i}.frf`).map((message) => ({ level: 'error' as const, message }))} />
					</div>
				</div>
			{/each}
			<button class="btn sm spaced" disabled={locked || !rows.length} onclick={add} data-testid="srs-limit-add">Add Limit Channel</button>
			{#if !rows.length}<p class="aq-small aq-muted">Enable an input in the channel table (Station & Channels) to hold it under a bound.</p>{/if}
			<Findings items={errs('limit_channels.channels').map((message) => ({ level: 'error' as const, message }))} />
		{:else}
			<p class="aq-muted aq-small">Off: only the control channel is controlled. The drive ceiling and the other limits still hold.</p>
		{/if}
	</section>

	{#if lc.enabled}
		<section class="aq-panel" aria-labelledby="rlc-notch" data-testid="srs-notch">
			<div class="aq-head"><h2 id="rlc-notch">The Notch</h2><span class="aq-sub">Planned by the control service at full level, before the first pulse.</span></div>
			{#if notch}
				<p class="aq-small" data-testid="srs-notch-words">{notchWords(notch)}</p>
			{:else}
				<p class="aq-muted aq-small">Shown once a run has identified the system and proceeded: which bands the drive is notched in, and the worst prediction before and after.</p>
			{/if}
		</section>
	{/if}
</div>

<style>
	.cols {
		display: grid;
		grid-template-columns: minmax(0, 1.4fr) minmax(0, 1fr);
		gap: var(--sp-3);
		align-items: start;
	}

	.spaced {
		margin-top: var(--sp-3);
	}

	.chan {
		margin-top: var(--sp-3);
		padding: var(--sp-3);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-md);
	}

	.chead {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
	}

	.lbl {
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}

	.r {
		text-align: right;
	}

	@media (max-width: 1240px) {
		.cols {
			grid-template-columns: minmax(0, 1fr);
		}
	}
</style>
