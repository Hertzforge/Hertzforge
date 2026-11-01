<script lang="ts">
	/**
	 * The station: the simulator, or NI-DAQmx devices from the service's scan. Driver-simulated
	 * NI devices are named as such and give SIM data; only physical devices give LIVE data. The
	 * choice is a draft until Use This Station; the service checks it against a fresh scan and
	 * says by name why a selection cannot be used. Nothing is opened until a run arms.
	 */
	import type { NiDevice } from '$lib/protocol/types';
	import type { AcquireState } from './store.svelte';

	interface Props {
		acq: AcquireState;
	}
	let { acq }: Props = $props();

	const sim = $derived(acq.stations?.stations[0] ?? null);
	const ni = $derived(acq.stations?.stations[1] ?? null);
	const sel = $derived(acq.selected);
	const draft = $derived(acq.stationDraft);
	const disabled = $derived(acq.locked);
	const why = $derived(acq.locked ? 'The station is locked while a run is live.' : null);
	const useWhy = $derived(!acq.connected ? 'The control service is not connected.' : !acq.isController ? 'Take control to change the station.' : null);

	interface Group {
		chassis: NiDevice | null;
		modules: NiDevice[];
	}

	/** Chassis with their modules by slot, then standalone devices. */
	const groups = $derived.by((): Group[] => {
		const devices = ni?.devices ?? [];
		const chassis = devices.filter((d) => d.parent === null && d.inputs.length === 0 && d.outputs.length === 0);
		const out: Group[] = chassis.map((c) => ({ chassis: c, modules: devices.filter((d) => d.parent === c.id).sort((a, b) => (a.slot ?? 0) - (b.slot ?? 0)) }));
		const placed = new Set(out.flatMap((g) => [g.chassis?.id, ...g.modules.map((m) => m.id)]));
		const loose = devices.filter((d) => !placed.has(d.id));
		if (loose.length) out.push({ chassis: null, modules: loose });
		return out;
	});

	const scanned = $derived(ni?.scanned_unix ? new Date(ni.scanned_unix * 1000).toLocaleTimeString('en-US', { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false }) : null);
	const usable = (d: NiDevice) => d.inputs.length + d.outputs.length > 0;

	function allOn(g: Group): boolean | 'mixed' {
		const on = g.modules.filter((m) => draft.devices.includes(m.id)).length;
		return on === 0 ? false : on === g.modules.length ? true : 'mixed';
	}

	function selectedWords(): string {
		if (!sel) return '';
		if (sel.kind === 'sim') return `Simulator, ${sel.inputs} inputs and ${sel.outputs} output on one shared clock.`;
		const names = sel.devices.join(', ');
		return `NI-DAQmx: ${names}. ${sel.inputs} inputs and ${sel.outputs} outputs${sel.timing_family ? `, one clock (${sel.timing_family})` : ''}.`;
	}
</script>

<section class="aq-panel station" aria-labelledby="station-title" data-testid="station-panel">
	<div class="aq-head">
		<h2 id="station-title">Station</h2>
		<span class="aq-sub">Where the run's inputs and drive come from.</span>
		<span class="aq-spacer"></span>
		{#if why}<span class="aq-muted aq-small">{why}</span>{/if}
	</div>

	<div class="kinds" role="radiogroup" aria-label="Station">
		<button class="kind" role="radio" aria-checked={draft.kind === 'sim'} data-testid="station-sim" disabled={disabled} onclick={() => acq.chooseStation('sim')}>
			<span class="radio" aria-hidden="true"></span>
			<span class="body">
				<span class="name">Simulator {#if sel?.kind === 'sim'}<span class="inuse">In use</span>{/if}</span>
				<span class="desc">A simulated test setup with resonances near 200 and 550 Hz, on one shared clock{sim ? `, ${sim.inputs} inputs and ${sim.outputs} output` : ''}. Its data is SIM.</span>
			</span>
		</button>
		<button class="kind" role="radio" aria-checked={draft.kind === 'ni'} data-testid="station-ni" disabled={disabled || !ni?.available} onclick={() => acq.chooseStation('ni')}>
			<span class="radio" aria-hidden="true"></span>
			<span class="body">
				<span class="name">NI-DAQmx {#if sel?.kind === 'ni'}<span class="inuse">In use</span>{/if}</span>
				{#if ni && !ni.available}
					<span class="desc unavailable" data-testid="ni-unavailable">{ni.unavailable?.message ?? 'Not available.'}</span>
				{:else if ni}
					<span class="desc">{ni.devices.filter(usable).length} {ni.devices.filter(usable).length === 1 ? 'device' : 'devices'} in the last scan{scanned ? `, at ${scanned}` : ''}. Physical devices give LIVE data; driver-simulated devices give SIM data.</span>
				{:else}
					<span class="desc">Waiting for the service's scan.</span>
				{/if}
			</span>
		</button>
	</div>

	{#if draft.kind === 'ni' && ni?.available}
		<div class="devices">
			<div class="aq-head devhead">
				<h3>Devices</h3>
				<span class="aq-sub">All of a run's channels must share one sample clock: choose modules in one chassis, or one standalone device.</span>
				<span class="aq-spacer"></span>
				<button class="btn sm" data-testid="rescan" onclick={() => acq.rescan()} disabled={acq.scanning || !acq.connected}>
					{acq.scanning ? 'Scanning' : 'Rescan'}
				</button>
			</div>
			<div class="aq-scroll">
				<table class="aq-table" data-testid="device-table">
					<thead>
						<tr>
							<th><span class="visually-hidden">Use</span></th>
							<th>Device</th>
							<th>Product</th>
							<th>Serial</th>
							<th class="num">Slot</th>
							<th class="num">Inputs</th>
							<th class="num">Outputs</th>
							<th>Clock</th>
							<th>Kind</th>
						</tr>
					</thead>
					<tbody>
						{#each groups as g (g.chassis?.id ?? 'standalone')}
							{#if g.chassis}
								{@const state = allOn(g)}
								<tr class="chassis">
									<td>
										<input
											type="checkbox"
											aria-label="Use every module in {g.chassis.id}"
											checked={state === true}
											{@attach (el: HTMLInputElement) => {
												el.indeterminate = state === 'mixed';
											}}
											{disabled}
											onchange={(e) => g.modules.filter(usable).forEach((m) => acq.toggleDevice(m.id, e.currentTarget.checked))}
										/>
									</td>
									<td class="mono">{g.chassis.id}</td>
									<td>{g.chassis.product}</td>
									<td class="mono">{g.chassis.serial || '-'}</td>
									<td></td>
									<td colspan="2" class="aq-muted aq-small">Chassis, {g.modules.length} {g.modules.length === 1 ? 'module' : 'modules'}</td>
									<td class="mono">{g.chassis.timing_family ?? '-'}</td>
									<td>{@render kind(g.chassis)}</td>
								</tr>
							{/if}
							{#each g.modules as m (m.id)}
								<tr class:module={!!g.chassis} class:on={draft.devices.includes(m.id)}>
									<td>
										<input type="checkbox" aria-label="Use {m.id}" data-testid="device-{m.id}" checked={draft.devices.includes(m.id)} disabled={disabled || !usable(m)} onchange={(e) => acq.toggleDevice(m.id, e.currentTarget.checked)} />
									</td>
									<td class="mono">{m.id}{#if sel?.devices.includes(m.id)}<span class="inuse">In use</span>{/if}</td>
									<td>{m.product}</td>
									<td class="mono">{m.serial || '-'}</td>
									<td class="num">{m.slot ?? '-'}</td>
									<td class="num">{m.inputs.length}</td>
									<td class="num">{m.outputs.length}</td>
									<td class="mono">{m.timing_family ?? '-'}</td>
									<td>{@render kind(m)}</td>
								</tr>
							{/each}
						{/each}
					</tbody>
				</table>
			</div>
		</div>
	{/if}

	<div class="foot">
		<p class="current" data-testid="station-current"><span class="eyebrow">In use</span> {selectedWords()}</p>
		{#if acq.stationDirty}
			<div class="aq-actions">
				{#if useWhy}<span class="aq-muted aq-small">{useWhy}</span>{/if}
				<button class="btn ghost sm" onclick={() => acq.revertStation()}>Put Back</button>
				<button class="btn primary sm" data-testid="station-use" disabled={disabled || !!useWhy || (draft.kind === 'ni' && draft.devices.length === 0) || acq.busy !== null} onclick={() => acq.useStation()}>Use This Station</button>
			</div>
		{/if}
	</div>
</section>

{#snippet kind(d: NiDevice)}
	{#if d.simulated}
		<span class="tag simulated" title="The driver reports that no physical device is behind this name.">Driver-simulated</span>
	{:else}
		<span class="tag physical">Physical</span>
	{/if}
{/snippet}

<style>
	.kinds {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
		gap: var(--sp-3);
	}

	.kind {
		display: flex;
		align-items: flex-start;
		gap: var(--sp-3);
		padding: var(--sp-3);
		text-align: left;
		color: var(--text-1);
		background: var(--bg-2);
		border: 1px solid var(--line-2);
		border-radius: var(--radius-md);
	}

	.kind:hover:not(:disabled) {
		border-color: var(--line-3);
	}

	.kind[aria-checked='true'] {
		border-color: var(--blue-2);
		box-shadow: inset 0 0 0 1px var(--blue-2);
		background: var(--blue-weak);
	}

	.kind:disabled {
		cursor: default;
	}

	.kind:disabled:not([aria-checked='true']) {
		opacity: 0.7;
	}

	.radio {
		flex: none;
		width: 16px;
		height: 16px;
		margin-top: 1px;
		border: 1.5px solid var(--line-3);
		border-radius: 50%;
	}

	.kind[aria-checked='true'] .radio {
		border-color: var(--blue-2);
		background: radial-gradient(circle, var(--blue-2) 0 4px, transparent 4.5px);
	}

	.body {
		display: flex;
		flex-direction: column;
		gap: 3px;
	}

	.name {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		font-weight: var(--weight-strong);
	}

	.desc {
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	.desc.unavailable {
		color: var(--amber-1);
	}

	.inuse {
		display: inline-block;
		margin-left: 6px;
		padding: 0 6px;
		font-family: var(--font-ui);
		font-size: var(--text-2xs);
		font-weight: var(--weight-strong);
		letter-spacing: 0.05em;
		text-transform: uppercase;
		color: var(--blue-1);
		border: 1px solid var(--blue-line);
		border-radius: 3px;
	}

	.devices {
		margin-top: var(--sp-4);
	}

	.devhead {
		margin-bottom: var(--sp-2);
	}

	.devhead h3 {
		font-size: var(--text-sm);
	}

	tr.chassis td {
		background: var(--bg-2);
		font-weight: var(--weight-medium);
	}

	tr.module td:nth-child(2) {
		padding-left: 22px;
	}

	tr.on td {
		background: var(--blue-weak);
	}

	td input[type='checkbox'] {
		margin: 0;
		accent-color: var(--blue-2);
	}

	.tag {
		display: inline-block;
		padding: 1px 7px;
		font-size: var(--text-2xs);
		font-weight: var(--weight-strong);
		border-radius: 3px;
		white-space: nowrap;
	}

	.tag.simulated {
		color: var(--amber-1);
		border: 1px dashed var(--amber-2);
		background: repeating-linear-gradient(-45deg, transparent 0 4px, var(--amber-weak) 4px 7px);
	}

	.tag.physical {
		color: var(--text-on-accent);
		background: var(--blue-2);
		border: 1px solid var(--blue-2);
	}

	.foot {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-2) var(--sp-4);
		margin-top: var(--sp-4);
		padding-top: var(--sp-3);
		border-top: 1px solid var(--line-1);
	}

	.current {
		display: flex;
		align-items: baseline;
		gap: var(--sp-2);
		font-size: var(--text-sm);
		color: var(--text-2);
	}
</style>
