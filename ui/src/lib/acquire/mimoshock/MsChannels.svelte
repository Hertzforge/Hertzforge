<script lang="ts">
	/**
	 * The station and the test's channels. The channel table (shared by every test type) holds the
	 * rows; here each enabled row has its part in the run: a control row is a control, or is taken
	 * out of the controls to be a limit; a drive row is a drive, in the table's order, with its own
	 * ceiling and abort in volts peak (there is no default: they are the amplifier inputs); a monitor
	 * row can be a limit with the peak it stops the run at. On the simulator, the plant the run is
	 * played on; and the capture.
	 */
	import ChannelTable from '../ChannelTable.svelte';
	import Findings from '../Findings.svelte';
	import NumberField from '../NumberField.svelte';
	import StationPanel from '../StationPanel.svelte';
	import { limitRows, MAX_CONTROLS, MAX_DRIVES, PLANTS, setControl } from './msSetup';
	import type { MimoShockStore } from './msStore.svelte';

	interface Props {
		ms: MimoShockStore;
	}
	let { ms }: Props = $props();

	const d = $derived(ms.setup);
	const locked = $derived(ms.locked);
	const E = $derived(ms.fieldErrors);
	const errs = (k: string) => E[k] ?? [];
	const rows = $derived(ms.rows.filter((r) => r.enabled));
	const controlRows = $derived(rows.filter((r) => r.role === 'control'));
	const limitable = $derived(limitRows(d, ms.rows));
	const onSim = $derived(ms.acq.selected?.kind !== 'ni');
	const name = (i: number) => {
		const r = ms.rows.find((x) => x.index === i);
		return r?.description ? `${i}, ${r.description}` : String(i);
	};

	function setLimit(channel: number, on: boolean) {
		if (locked) return;
		if (on && !d.limits.some((l) => l.channel === channel)) d.limits = [...d.limits, { channel, abort_peak: NaN }];
		if (!on) d.limits = d.limits.filter((l) => l.channel !== channel);
	}
</script>

<div class="stack">
	<StationPanel acq={ms.acq} />
	<ChannelTable acq={ms.acq} />

	<section class="aq-panel" aria-labelledby="ms-ch-title" data-testid="ms-channels">
		<div class="aq-head"><h2 id="ms-ch-title">The Run's Channels</h2><span class="aq-sub">From the channel table's enabled rows: up to {MAX_CONTROLS} controls and {MAX_DRIVES} drives.</span></div>

		<div class="aq-group">
			<h3>Controls</h3>
			<p class="aq-lead aq-small">Each control has its own target and is judged on its own. A control row taken out of the controls can be a limit below.</p>
			{#if controlRows.length}
				<ul class="picks">
					{#each controlRows as r (r.index)}
						<li>
							<label class="aq-switch">
								<input type="checkbox" role="switch" aria-checked={d.controls.includes(r.index)} checked={d.controls.includes(r.index)} disabled={locked} onchange={(e) => setControl(d, ms.rows, r.index, e.currentTarget.checked)} data-testid="ms-control-{r.index}" aria-label="Channel {r.index} is a control" />
								<span class="track" aria-hidden="true"><span class="thumb"></span></span>
								<span class="word">{d.controls.includes(r.index) ? 'Control' : 'Not a control'}</span>
							</label>
							<span class="nm">Channel {name(r.index)}</span>
						</li>
					{/each}
				</ul>
			{/if}
			<Findings items={errs('channels.controls').map((message) => ({ level: 'error' as const, message }))} />
		</div>

		<div class="aq-group">
			<h3>Drives</h3>
			<p class="aq-lead aq-small">Every enabled drive row drives the run, in the table's order. A drive over its ceiling is scaled down to it; a drive over its abort is refused and the run aborts.</p>
			{#if d.drives.length}
				<table class="aq-table" data-testid="ms-drives">
					<thead><tr><th>Drive</th><th class="num">Ceiling</th><th class="num">Abort</th></tr></thead>
					<tbody>
						{#each d.drives as x, i (x.channel)}
							<tr>
								<td>Channel {name(x.channel)}</td>
								<td><NumberField compact id="ms-drive-ceil-{i}" label="Drive {x.channel} ceiling" unit="V pk" step={0.5} min={0} max={10} bind:value={x.ceiling_vpk} errors={errs(`drives.${i}.ceiling_vpk`)} disabled={locked} testid="ms-drive-ceiling-{i}" /></td>
								<td><NumberField compact id="ms-drive-abort-{i}" label="Drive {x.channel} abort" unit="V pk" step={0.5} min={0} max={10} bind:value={x.abort_vpk} errors={errs(`drives.${i}.abort_vpk`)} disabled={locked} testid="ms-drive-abort-{i}" /></td>
							</tr>
						{/each}
					</tbody>
				</table>
				{#each d.drives as _, i (i)}
					<Findings items={[...errs(`drives.${i}.ceiling_vpk`), ...errs(`drives.${i}.abort_vpk`)].map((message) => ({ level: 'error' as const, message: `Drive ${d.drives[i].channel}: ${message}` }))} />
				{/each}
			{/if}
			<Findings items={errs('channels.drives').map((message) => ({ level: 'error' as const, message }))} />
		</div>

		<div class="aq-group">
			<h3>Limits</h3>
			<p class="aq-lead aq-small">A limit is watched, not controlled: a measured or predicted peak over its abort peak ends the run.</p>
			{#if limitable.length}
				<table class="aq-table" data-testid="ms-limits">
					<thead><tr><th>Channel</th><th>Limit</th><th class="num">Abort peak</th></tr></thead>
					<tbody>
						{#each limitable as r (r.index)}
							{@const li = d.limits.findIndex((l) => l.channel === r.index)}
							<tr>
								<td>Channel {name(r.index)}</td>
								<td>
									<label class="aq-switch">
										<input type="checkbox" role="switch" aria-checked={li >= 0} checked={li >= 0} disabled={locked} onchange={(e) => setLimit(r.index, e.currentTarget.checked)} data-testid="ms-limit-{r.index}" aria-label="Channel {r.index} is a limit" />
										<span class="track" aria-hidden="true"><span class="thumb"></span></span>
										<span class="word">{li >= 0 ? 'Limit' : 'Off'}</span>
									</label>
								</td>
								<td>
									{#if li >= 0}
										<NumberField compact id="ms-limit-peak-{r.index}" label="Channel {r.index} abort peak" unit={r.units} step={1} min={0} bind:value={d.limits[li].abort_peak} errors={errs(`limits.${li}.abort_peak`)} disabled={locked} testid="ms-limit-peak-{r.index}" />
									{/if}
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			{:else}
				<p class="aq-small aq-muted">No row can be a limit: switch on a monitor row in the table, or take a control row out of the controls.</p>
			{/if}
		</div>

		<div class="aq-group">
			<h3>Simulator</h3>
			<div class="aq-fields">
				<div class="sel">
					<label for="ms-plant" class="lbl">Plant</label>
					<select id="ms-plant" class="field" value={d.simulator.plant ?? ''} disabled={locked || !onSim} onchange={(e) => (d.simulator.plant = e.currentTarget.value || null)} data-testid="ms-plant">
						<option value="">Chosen by the table's size</option>
						{#each PLANTS as p (p.id)}<option value={p.id}>{p.label}</option>{/each}
					</select>
					<span class="aq-small aq-muted">The simulated structure the run is played on. Its size must match the controls and drives. Not used on hardware.</span>
					<Findings items={errs('simulator.plant').map((message) => ({ level: 'error' as const, message }))} />
				</div>
			</div>
		</div>

		<div class="aq-group">
			<h3>Capture</h3>
			<div class="aq-fields">
				<label class="aq-switch">
					<input type="checkbox" role="switch" aria-checked={d.capture.auto_start} bind:checked={d.capture.auto_start} disabled={locked} data-testid="ms-capture-auto" />
					<span class="track" aria-hidden="true"><span class="thumb"></span></span>
					<span class="word">{d.capture.auto_start ? 'Write the run to disk when it ends' : 'Do not write the run to disk'}</span>
				</label>
				<div class="sel">
					<label for="ms-run-name" class="lbl">Run name</label>
					<input id="ms-run-name" class="field" bind:value={d.capture.run_name} disabled={locked || !d.capture.auto_start} data-testid="ms-run-name" />
					<Findings items={errs('capture.run_name').map((message) => ({ level: 'error' as const, message }))} />
				</div>
			</div>
		</div>
	</section>
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
