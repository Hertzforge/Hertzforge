<script lang="ts">
	/**
	 * The station and the test's channels. The channel table (shared by every test type) holds the
	 * rows; here each enabled row has its role in the run: a control row is a control, or is taken out
	 * of the controls (it may then be a limit channel); a drive row is a drive, in the table's order,
	 * with its two ceilings, its aborts and the most identification may put on it. The rms ceiling
	 * bounds the random part; the peak ceiling is the budget the floor's peaks and every tone's peak
	 * share. There is no default for either: they are the amplifier's inputs. The peak abort, left
	 * empty, is the peak ceiling plus 1 dB, and the field shows that value.
	 */
	import ChannelTable from '../ChannelTable.svelte';
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import StationPanel from '../StationPanel.svelte';
	import { MIMO_MIXED_MAX_CONTROLS, MIMO_MIXED_MAX_DRIVES } from '$lib/protocol/mimoMixed';
	import { abortVpkOf, setControl } from './mmixSetup';
	import { volts } from './mmixWords';
	import type { MimoMixedStore } from './mmixStore.svelte';

	interface Props {
		mm: MimoMixedStore;
	}
	let { mm }: Props = $props();

	const d = $derived(mm.setup);
	const locked = $derived(mm.locked);
	const E = $derived(mm.fieldErrors);
	const errs = (k: string) => E[k] ?? [];
	const controlRows = $derived(mm.rows.filter((r) => r.enabled && r.role === 'control'));
	const name = (i: number) => {
		const r = mm.rows.find((x) => x.index === i);
		return r?.description ? `${i}, ${r.description}` : String(i);
	};
</script>

<div class="stack">
	<StationPanel acq={mm.acq} />
	<ChannelTable acq={mm.acq} />

	<section class="aq-panel" aria-labelledby="mmix-ch-title" data-testid="mmix-channels">
		<div class="aq-head"><h2 id="mmix-ch-title">The Run's Channels</h2><span class="aq-sub">From the channel table's enabled rows: up to {MIMO_MIXED_MAX_CONTROLS} controls and {MIMO_MIXED_MAX_DRIVES} drives.</span></div>

		<div class="aq-group">
			<h3>Controls</h3>
			<p class="aq-lead aq-small">The floor is controlled as a spectral matrix across every control, as in MIMO Random. Each tone and each narrowband has a target at every control. A control row taken out of the controls can be a limit channel.</p>
			{#if controlRows.length}
				<ul class="picks">
					{#each controlRows as r (r.index)}
						{@const on = d.controls.some((c) => c.channel === r.index)}
						<li>
							<label class="aq-switch">
								<input type="checkbox" role="switch" aria-checked={on} checked={on} disabled={locked} onchange={(e) => setControl(d, mm.rows, r.index, e.currentTarget.checked)} data-testid="mmix-control-{r.index}" aria-label="Channel {r.index} is a control" />
								<span class="track" aria-hidden="true"><span class="thumb"></span></span>
								<span class="word">{on ? 'Control' : 'Not a control'}</span>
							</label>
							<span class="nm">Channel {name(r.index)}</span>
						</li>
					{/each}
				</ul>
			{/if}
			<Findings items={errs('channels.controls').map((message) => ({ level: 'error' as const, message }))} testid="mmix-controls-findings" />
		</div>

		<div class="aq-group">
			<h3>Drives</h3>
			<p class="aq-lead aq-small">Every enabled drive row drives the run, in the table's order. The floor's random and every tone add on each drive, so the peak ceiling is a budget they share; when the tones would take a drive past it, the ceiling rule under Laws scales them down and they show as limiting. A drive at its abort ends the run.</p>
			{#if d.drives.length}
				<table class="aq-table" data-testid="mmix-drives">
					<thead><tr><th>Drive</th><th class="num">RMS ceiling</th><th class="num">Peak ceiling</th><th class="num" title="The peak at which the run aborts; empty, the peak ceiling plus 1 dB.">Peak abort</th><th class="num">RMS abort</th><th class="num" title="The most identification may put on this drive.">Identification at most</th></tr></thead>
					<tbody>
						{#each d.drives as x, i (x.channel)}
							<tr data-testid="mmix-drive-{i}">
								<td>Channel {name(x.channel)}</td>
								<td><NumberField compact id="mmix-drive-rms-{i}" label="Drive {x.channel} rms ceiling" unit="V rms" step={0.1} min={0} max={10} bind:value={x.ceiling_vrms} errors={errs(`drives.${i}.ceiling_vrms`)} disabled={locked} testid="mmix-drive-rms-{i}" /></td>
								<td><NumberField compact id="mmix-drive-peak-{i}" label="Drive {x.channel} peak ceiling" unit="V pk" step={0.5} min={0} max={10} bind:value={x.peak_v} errors={errs(`drives.${i}.peak_v`)} disabled={locked} testid="mmix-drive-peak-{i}" /></td>
								<td>
									<NumberField compact id="mmix-drive-vpk-{i}" label="Drive {x.channel} peak abort" unit="V pk" step={0.5} min={0} max={10} nullable={Number.isFinite(x.peak_v) ? `${volts(abortVpkOf(x))}, +1 dB` : 'Ceiling +1 dB'} fallback={Number.isFinite(x.peak_v) ? Number(abortVpkOf(x).toFixed(3)) : 1} bind:value={x.abort_vpk} errors={errs(`drives.${i}.abort_vpk`)} disabled={locked} testid="mmix-drive-vpk-{i}" />
									{#if x.abort_vpk === null && Number.isFinite(x.peak_v)}<span class="sub" data-testid="mmix-drive-vpk-default-{i}">Default: {volts(abortVpkOf(x))}, the peak ceiling plus 1 dB</span>{/if}
								</td>
								<td><NumberField compact id="mmix-drive-abort-{i}" label="Drive {x.channel} rms abort" unit="V rms" step={0.1} min={0} max={10} nullable="None" fallback={Number.isFinite(x.ceiling_vrms) ? x.ceiling_vrms * 1.2 : 1} bind:value={x.abort_vrms} errors={errs(`drives.${i}.abort_vrms`)} disabled={locked} testid="mmix-drive-abort-{i}" /></td>
								<td><NumberField compact id="mmix-drive-pre-{i}" label="Drive {x.channel} identification at most" unit="V rms" step={0.05} min={0} max={10} bind:value={x.pretest_max_vrms} errors={errs(`drives.${i}.pretest_max_vrms`)} disabled={locked} testid="mmix-drive-pre-{i}" /></td>
							</tr>
						{/each}
					</tbody>
				</table>
				{#each d.drives as x, i (x.channel)}
					<Findings items={[...errs(`drives.${i}.ceiling_vrms`), ...errs(`drives.${i}.peak_v`), ...errs(`drives.${i}.abort_vpk`), ...errs(`drives.${i}.abort_vrms`), ...errs(`drives.${i}.pretest_max_vrms`), ...errs(`drives.${i}`)].map((message) => ({ level: 'error' as const, message: `Drive ${x.channel}: ${message}` }))} />
				{/each}
			{/if}
			<Findings items={errs('channels.drives').map((message) => ({ level: 'error' as const, message }))} />
		</div>
	</section>
</div>

<style>
	.sub {
		display: block;
		color: var(--text-3);
		font-size: var(--text-xs);
	}

	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
	}

	.picks {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-2) var(--sp-4);
		margin: 0 0 var(--sp-2);
		padding: 0;
		list-style: none;
	}

	.picks li {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
	}

	.nm {
		font-size: var(--text-sm);
	}
</style>
