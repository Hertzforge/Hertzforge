<script lang="ts">
	/**
	 * The station and the test's channels. The channel table (shared by every test type) holds the
	 * rows; here each enabled row has its part in the run: a control row is a control, or is taken
	 * out of the controls (it may then be a limit channel); a drive row is a drive, in the table's
	 * order, with its own ceiling and abort in volts peak (there is no default: they are the
	 * amplifier inputs). On the simulator, the structure the run is played on; then the run's words
	 * for its record.
	 */
	import ChannelTable from '../ChannelTable.svelte';
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import StationPanel from '../StationPanel.svelte';
	import MsineRecord from './MsineRecord.svelte';
	import { MAX_CONTROLS, MAX_DRIVES, setControl, STRUCTURES, structuresFor } from './msineSetup';
	import type { MimoSineStore } from './msineStore.svelte';

	interface Props {
		ms: MimoSineStore;
	}
	let { ms }: Props = $props();

	const d = $derived(ms.setup);
	const locked = $derived(ms.locked);
	const E = $derived(ms.fieldErrors);
	const errs = (k: string) => E[k] ?? [];
	const controlRows = $derived(ms.rows.filter((r) => r.enabled && r.role === 'control'));
	const onSim = $derived(ms.acq.selected?.kind !== 'ni');
	const sized = $derived(structuresFor(ms.rows));
	const name = (i: number) => {
		const r = ms.rows.find((x) => x.index === i);
		return r?.description ? `${i}, ${r.description}` : String(i);
	};
</script>

<div class="stack">
	<StationPanel acq={ms.acq} />
	<ChannelTable acq={ms.acq} />

	<section class="aq-panel" aria-labelledby="msine-ch-title" data-testid="msine-channels">
		<div class="aq-head"><h2 id="msine-ch-title">The Run's Channels</h2><span class="aq-sub">From the channel table's enabled rows: up to {MAX_CONTROLS} controls and {MAX_DRIVES} drives.</span></div>

		<div class="aq-group">
			<h3>Controls</h3>
			<p class="aq-lead aq-small">Each control has its own reference, an amplitude and a phase against frequency, and is judged on its own. A control row taken out of the controls can be a limit channel.</p>
			{#if controlRows.length}
				<ul class="picks">
					{#each controlRows as r (r.index)}
						{@const on = d.controls.some((c) => c.channel === r.index)}
						<li>
							<label class="aq-switch">
								<input type="checkbox" role="switch" aria-checked={on} checked={on} disabled={locked} onchange={(e) => setControl(d, ms.rows, r.index, e.currentTarget.checked)} data-testid="msine-control-{r.index}" aria-label="Channel {r.index} is a control" />
								<span class="track" aria-hidden="true"><span class="thumb"></span></span>
								<span class="word">{on ? 'Control' : 'Not a control'}</span>
							</label>
							<span class="nm">Channel {name(r.index)}</span>
						</li>
					{/each}
				</ul>
			{/if}
			<Findings items={errs('channels.controls').map((message) => ({ level: 'error' as const, message }))} testid="msine-controls-findings" />
		</div>

		<div class="aq-group">
			<h3>Drives</h3>
			<p class="aq-lead aq-small">Every enabled drive row drives the run, in the table's order, each with its own amplitude and phase. A drive vector with a drive over its ceiling is scaled down as a whole, its direction kept; a drive at its abort ends the run.</p>
			{#if d.drives.length}
				<table class="aq-table" data-testid="msine-drives">
					<thead><tr><th>Drive</th><th class="num">Ceiling</th><th class="num">Abort</th></tr></thead>
					<tbody>
						{#each d.drives as x, i (x.channel)}
							<tr>
								<td>Channel {name(x.channel)}</td>
								<td><NumberField compact id="msine-drive-ceil-{i}" label="Drive {x.channel} ceiling" unit="V pk" step={0.5} min={0} max={10} bind:value={x.ceiling_vpk} errors={errs(`drives.${i}.ceiling_vpk`)} disabled={locked} testid="msine-drive-ceiling-{i}" /></td>
								<td><NumberField compact id="msine-drive-abort-{i}" label="Drive {x.channel} abort" unit="V pk" step={0.5} min={0} max={10} bind:value={x.abort_vpk} errors={errs(`drives.${i}.abort_vpk`)} disabled={locked} testid="msine-drive-abort-{i}" /></td>
							</tr>
						{/each}
					</tbody>
				</table>
				{#each d.drives as x, i (x.channel)}
					<Findings items={[...errs(`drives.${i}.ceiling_vpk`), ...errs(`drives.${i}.abort_vpk`)].map((message) => ({ level: 'error' as const, message: `Drive ${x.channel}: ${message}` }))} />
				{/each}
			{/if}
			<Findings items={errs('channels.drives').map((message) => ({ level: 'error' as const, message }))} />
		</div>

		<div class="aq-group">
			<h3>Simulator</h3>
			<div class="sel">
				<label for="msine-structure" class="lbl">Structure</label>
				<select id="msine-structure" class="field" value={d.simulator.structure ?? ''} disabled={locked || !onSim} onchange={(e) => (d.simulator.structure = e.currentTarget.value || null)} data-testid="msine-structure">
					<option value="">The linear structure of the run's size</option>
					{#each STRUCTURES as s (s.id)}<option value={s.id} disabled={!sized.includes(s)}>{s.label}</option>{/each}
				</select>
				<span class="aq-small aq-muted">The simulated structure the run is played on; its size must match the controls and drives. Not used on hardware.</span>
				<Findings items={errs('simulator.structure').map((message) => ({ level: 'error' as const, message }))} testid="msine-structure-findings" />
			</div>
		</div>
	</section>

	<MsineRecord {ms} />
</div>

<style>
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

	.sel {
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
