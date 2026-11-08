<script lang="ts">
	/**
	 * The tolerance every band of a pulse's SRS is judged against, at the level fired: how far under
	 * and over the target a band may be before it alarms, and before it aborts the run. The same
	 * for every band, or one pair per band of the grid the control service derives. With a ladder
	 * that advances on passing pulses, a pulse passes when every band is inside its alarm band.
	 */
	import { formatNumber } from '$lib/viz/format';
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import type { SrsStore } from './srsStore.svelte';

	interface Props {
		srs: SrsStore;
	}
	let { srs }: Props = $props();

	const e = $derived(srs.setup.envelope);
	const locked = $derived(srs.locked);
	const E = $derived(srs.fieldErrors);
	const W = $derived(srs.fieldWarnings);
	const errs = (k: string) => E[k] ?? [];
	const warns = (k: string) => W[k] ?? [];
	const bands = $derived(srs.bandsHz);
	const fits = $derived(!!bands && e.bands.length === bands.length);
	const tableErrors = $derived(Object.entries(E).filter(([k]) => k === 'envelope.per_band' || /^envelope\.per_band\./.test(k)).flatMap(([, v]) => v));
</script>

{#snippet pair(id: string, label: string, v: [number, number], key: string, help: string, testid: string)}
	<div class="pair">
		<span class="lbl">{label}</span>
		<div class="aq-fields two">
			<NumberField id="{id}-lo" label="Under the target" unit="dB" step={0.5} min={0} value={v[0]} onchange={(x) => (v[0] = x ?? NaN)} errors={errs(key)} warnings={warns(key)} disabled={locked} testid="{testid}-lo" />
			<NumberField id="{id}-hi" label="Over the target" unit="dB" step={0.5} min={0} value={v[1]} onchange={(x) => (v[1] = x ?? NaN)} disabled={locked} testid="{testid}-hi" />
		</div>
		<p class="help">{help}</p>
	</div>
{/snippet}

<section class="aq-panel narrow" aria-labelledby="re-title" data-testid="srs-envelope">
	<div class="aq-head"><h2 id="re-title">Envelope</h2><span class="aq-sub">Judged band by band, against the target at the level fired.</span></div>
	{@render pair('re-al', 'Alarm', e.alarm_db, 'envelope.alarm_db', 'A band outside it alarms, and the pulse does not pass. Usual: 3 dB each side.', 'srs-alarm')}
	{@render pair('re-ab', 'Abort', e.abort_db, 'envelope.abort_db', 'A band outside it aborts the run. At least as wide as the alarm on each side. Usual: 9 dB under, 6 dB over.', 'srs-abort')}

	<div class="aq-group">
		<label class="aq-switch" for="re-pb">
			<input id="re-pb" type="checkbox" role="switch" checked={e.per_band} disabled={locked || (!e.per_band && !bands)} onchange={(ev) => srs.setPerBandEnvelope(ev.currentTarget.checked)} data-testid="srs-envelope-per-band" />
			<span class="track" aria-hidden="true"><span class="thumb"></span></span>
			<span>A pair for each band</span>
			<span class="word">{e.per_band ? 'On' : 'Off'}</span>
		</label>
		{#if !e.per_band}
			<p class="aq-small aq-muted">{bands ? 'Off: every band has the alarm and abort above.' : 'Offered once the control service has given the band grid (Target).'}</p>
		{:else}
			<p class="aq-small aq-muted">The pairs above are not used while this is on: each band has its own. New bands start from the pairs above.</p>
			{#if bands && !fits}
				<p class="aq-notice warn aq-small" role="alert">The band grid has changed to {bands.length} bands; the table has {e.bands.length}. <button class="btn sm" disabled={locked} onclick={() => srs.fitPerBand()} data-testid="srs-envelope-fit">Fit to the Grid</button></p>
			{/if}
			<table class="aq-table bands" data-testid="srs-envelope-table">
				<thead>
					<tr><th>Band</th><th class="r">Alarm under, dB</th><th class="r">Alarm over, dB</th><th class="r">Abort under, dB</th><th class="r">Abort over, dB</th></tr>
				</thead>
				<tbody>
					{#each e.bands as b, i (i)}
						<tr>
							<td class="num">{bands?.[i] !== undefined ? `${formatNumber(bands[i], 4)} Hz` : `Band ${i + 1}`}</td>
							{#each [[b.alarm_db, 0, 'alarm under'], [b.alarm_db, 1, 'alarm over'], [b.abort_db, 0, 'abort under'], [b.abort_db, 1, 'abort over']] as [v, k, what] (what)}
								<td class="num"><NumberField id="re-b{i}-{what}" label="Band {i + 1}, {what}" compact step={0.5} min={0} value={(v as number[])[k as number]} onchange={(x) => ((v as number[])[k as number] = x ?? NaN)} disabled={locked} /></td>
							{/each}
						</tr>
					{/each}
				</tbody>
			</table>
		{/if}
		<Findings items={tableErrors.map((message) => ({ level: 'error' as const, message }))} />
	</div>
</section>

<style>
	.narrow {
		max-width: 860px;
	}

	.pair {
		display: flex;
		flex-direction: column;
		gap: 4px;
		margin-bottom: var(--sp-3);
	}

	.lbl {
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
		color: var(--text-1);
	}

	.help {
		margin: 2px 0 0;
		font-size: var(--text-xs);
		color: var(--text-3);
	}

	.bands {
		margin-top: var(--sp-2);
	}

	.r {
		text-align: right;
	}
</style>
