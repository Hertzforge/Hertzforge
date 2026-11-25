<script lang="ts">
	/**
	 * Configure: the station every test type runs on, set once for all of them. The simulator or
	 * NI-DAQmx devices (the station picker), the models the simulator plays for the multiple-drive
	 * test types, what the simulator gives a Recorder run, and where runs are kept. The test types
	 * show only their channel tables; they take the station from here.
	 *
	 * The choice is a draft until Use This Station, which sends the station and, on the simulator,
	 * its models in one `station.select`. The service refuses a change while a run is live, and the
	 * view says so; it can still be read then. Leaving with a draft not in use asks first.
	 */
	import type { AppState } from '$lib/app/state.svelte';
	import type { SimModel, SimulatorChoice } from '$lib/protocol/types';
	import DataFolderLine from '$lib/shell/DataFolderLine.svelte';
	import RecorderSimSource from './recorder/RecorderSimSource.svelte';
	import { recorderRows } from './recorder/recorderSetup';
	import { modelWords, SHOCK_PLANT_WORDS, SINE_STRUCTURE_WORDS } from './simModels';
	import StationPanel from './StationPanel.svelte';
	import { onMount } from 'svelte';

	interface Props {
		app: AppState;
	}
	let { app }: Props = $props();

	const acq = $derived(app.acquire);
	const tests = $derived(app.tests);
	const sel = $derived(acq.selected);
	const draft = $derived(acq.stationDraft);
	const sim = $derived(acq.stations?.stations[0] ?? null);
	const locked = $derived(acq.locked);
	const seat = $derived(acq.session?.controller ?? null);
	/** Control can be taken here: connected, not held, and nobody present holds it. */
	const canClaim = $derived(acq.connected && !acq.isController && (!seat || !seat.present));
	const useWhy = $derived(locked ? 'The station is not changed while a run is live.' : !acq.connected ? 'The control service is not connected.' : !acq.isController ? 'Take control to change the station.' : null);
	const fromName = $derived(tests.configureFrom === 'chooser' ? 'Test Type' : (tests.type(tests.configureFrom)?.title ?? 'Back'));
	const recorded = $derived(recorderRows(acq.rows).recorded.map((r) => r.index));

	let heading = $state<HTMLElement | null>(null);
	/** Back was asked for with a draft not in use: the view asks before leaving. */
	let leaving = $state(false);

	onMount(() => {
		if (tests.takeFocus()) heading?.focus();
	});

	const MODELS: { key: keyof SimulatorChoice; label: string; help: string; words: Record<string, string> }[] = [
		{ key: 'shock_plant', label: 'MIMO Shock Plant', help: 'The simulated structure a MIMO Shock run is played on. Its inputs count the enabled control and monitor rows.', words: SHOCK_PLANT_WORDS },
		{ key: 'sine_structure', label: 'MIMO Sine and MIMO Mixed Structure', help: 'The simulated structure a MIMO Sine or MIMO Mixed Mode run is played on. Its inputs count every enabled row that is not a drive.', words: SINE_STRUCTURE_WORDS }
	];

	const optionsOf = (key: keyof SimulatorChoice): SimModel[] => sim?.models?.[key] ?? [];

	function selectedWords(): string {
		if (!sel) return 'Waiting for the control service.';
		if (sel.kind === 'sim') return `Simulator, ${sel.inputs} inputs and ${sel.outputs} ${sel.outputs === 1 ? 'output' : 'outputs'} on one shared clock.`;
		const names = sel.devices.join(', ');
		return `NI-DAQmx: ${names}. ${sel.inputs} inputs and ${sel.outputs} outputs${sel.timing_family ? `, one clock (${sel.timing_family})` : ''}.`;
	}

	function back() {
		if (acq.stationDirty && !leaving) {
			leaving = true;
			return;
		}
		if (leaving) acq.revertStation();
		leaving = false;
		tests.fromConfigure();
	}

	async function use() {
		if (await acq.useStation()) leaving = false;
	}
</script>

<div class="configure" data-testid="configure-screen">
	<header class="head">
		<button class="btn ghost sm back" data-testid="configure-back" onclick={back}>
			<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M7.5 2.5L4 6l3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
			{fromName}
		</button>
		<span class="sep" aria-hidden="true">/</span>
		<h1 class="here" tabindex="-1" bind:this={heading}>Configure</h1>
		<span class="aq-sub">The station every test type runs on.</span>
	</header>

	{#if leaving}
		<div class="leave" role="alertdialog" aria-labelledby="leave-words" data-testid="configure-leave">
			<span id="leave-words">The station chosen here is not in use yet.</span>
			<button class="btn ghost sm" data-testid="configure-stay" onclick={() => (leaving = false)}>Stay</button>
			<button class="btn sm" data-testid="configure-leave-confirm" onclick={back}>Put Back and Leave</button>
		</div>
	{/if}

	<div class="body">
		<StationPanel {acq} />

		{#if draft.kind === 'sim'}
			<section class="aq-panel" aria-labelledby="sim-models-title" data-testid="sim-models">
				<div class="aq-head">
					<h2 id="sim-models-title">Simulator Models</h2>
					<span class="aq-sub">What the simulator plays for the test types that drive several shakers. Each test type checks the model against its channel table.</span>
				</div>
				<div class="models">
					{#each MODELS as m (m.key)}
						<div class="model">
							<label for="model-{m.key}" class="lbl">{m.label}</label>
							<select id="model-{m.key}" class="field" value={draft.simulator[m.key] ?? ''} disabled={locked} onchange={(e) => acq.chooseModel(m.key, e.currentTarget.value || null)} data-testid="model-{m.key}">
								<option value="">Sized to the Channel Table</option>
								{#each optionsOf(m.key) as o (o.name)}<option value={o.name}>{modelWords(o, m.words)}</option>{/each}
							</select>
							<span class="aq-small aq-muted">{m.help}</span>
						</div>
					{/each}
				</div>
			</section>
		{/if}

		<section class="aq-panel use" aria-label="The station in use" data-testid="station-use-bar">
			<p class="current" data-testid="station-current"><span class="eyebrow">In Use</span> {selectedWords()}</p>
			{#if acq.stationDirty}
				<div class="aq-actions">
					{#if useWhy}<span class="aq-muted aq-small">{useWhy}</span>{/if}
					{#if canClaim && !locked}<button class="btn sm" data-testid="configure-claim" onclick={() => acq.claim()} disabled={acq.busy === 'Take Control'}>Take Control</button>{/if}
					<button class="btn ghost sm" data-testid="station-revert" onclick={() => acq.revertStation()}>Put Back</button>
					<button class="btn primary sm" data-testid="station-use" disabled={!!useWhy || (draft.kind === 'ni' && draft.devices.length === 0) || acq.busy !== null} onclick={use}>Use This Station</button>
				</div>
			{/if}
		</section>

		{#if draft.kind === 'sim'}
			<section class="aq-panel" aria-label="The Recorder on the simulator">
				<RecorderSimSource rows={recorded} />
			</section>
		{/if}

		<section class="aq-panel" aria-label="Where runs are kept">
			<DataFolderLine {locked} />
		</section>
	</div>
</div>

<style>
	.configure {
		display: flex;
		flex-direction: column;
		height: 100%;
		min-height: 0;
	}

	.head {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 4px var(--sp-2);
		min-height: 40px;
		padding: 4px var(--sp-4);
		border-bottom: 1px solid var(--line-1);
	}

	.back {
		gap: 4px;
		margin-left: calc(-1 * var(--sp-2));
		color: var(--text-2);
	}

	.sep {
		color: var(--text-4);
	}

	.here {
		font-size: var(--text-md);
		font-weight: var(--weight-strong);
		color: var(--text-1);
	}

	.leave {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2);
		padding: var(--sp-2) var(--sp-4);
		font-size: var(--text-sm);
		border-bottom: 1px solid var(--line-1);
		background: var(--bg-2);
	}

	.body {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-height: 0;
		overflow-y: auto;
		padding: var(--sp-4) var(--sp-5) var(--sp-6);
		max-width: 1180px;
	}

	.models {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
		gap: var(--sp-3);
	}

	.model {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.lbl {
		font-size: var(--text-xs);
		font-weight: var(--weight-medium);
		color: var(--text-2);
	}

	.use {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-2) var(--sp-4);
	}

	.current {
		display: flex;
		align-items: baseline;
		gap: var(--sp-2);
		margin: 0;
		font-size: var(--text-sm);
		color: var(--text-2);
	}
</style>
