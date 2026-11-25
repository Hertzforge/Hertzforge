/**
 * Configure: the station every test type runs on, set in one place. The view is reached from the
 * chooser and from every test type's screen and goes back there; the simulator's models go with
 * the station in one `station.select`, which the mock checks by name; no test type's screen holds
 * the station any more, and a layout kept with the old station card still opens.
 */
import { describe, expect, it } from 'vitest';
import { ServiceClient } from '$lib/protocol/client.svelte';
import { createControlServer, SIM_MODELS } from '$lib/protocol/mock/control';
import { mockSocketFactory } from '$lib/protocol/mock/socket';
import { fromSaved, toSaved } from './screen/layout';
import { SINE_HOOKS } from './sine/sineViews.svelte';
import { AcquireState } from './store.svelte';
import { TestTypeView, type LiveTest } from './testTypes.svelte';

async function controller() {
	const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(createControlServer({ speed: 40 }), 1) });
	const acq = new AcquireState(client, { session: null, local: null, checkDelayMs: 0 });
	client.connect();
	await client.whenOpen();
	await acq.sync();
	await acq.claim();
	return { client, acq };
}

const refusal = (client: ServiceClient, params: Record<string, unknown>) =>
	client.request('station.select', params).then(
		() => null,
		(e: { reason?: string; detail?: { param?: string } }) => [e.reason, e.detail?.param]
	);

describe('Configure, reached from the chooser and every screen', () => {
	it('opens from the chooser and from a test type, and goes back to where it was opened', () => {
		const src = { live: null as LiveTest };
		const view = new TestTypeView(() => src.live);
		view.open();
		expect(view.screen).toBe('chooser');
		view.toConfigure();
		expect(view.screen).toBe('configure');
		expect(view.configureFrom).toBe('chooser');
		view.fromConfigure();
		expect(view.screen).toBe('chooser');

		expect(view.choose('mimo_shock')).toBe(true);
		view.toConfigure();
		expect(view.screen).toBe('configure');
		expect(view.configureFrom).toBe('mimo_shock');
		// opened again from Configure itself it stays put, still going back to MIMO Shock
		view.toConfigure();
		expect(view.configureFrom).toBe('mimo_shock');
		view.fromConfigure();
		expect(view.screen).toBe('mimo_shock');
	});

	it('opens while a run is live, to read the station, and goes back to the run', () => {
		const src = { live: 'sine' as LiveTest };
		const view = new TestTypeView(() => src.live);
		view.open();
		expect(view.screen).toBe('sine');
		view.toConfigure();
		expect(view.screen).toBe('configure');
		view.fromConfigure();
		expect(view.screen).toBe('sine');
	});
});

describe("the simulator's models, with the station", () => {
	it('lists the models the simulator plays, and puts a chosen one in use with the station', async () => {
		const { acq } = await controller();
		const sim = acq.stations?.stations[0];
		expect(sim?.kind).toBe('sim');
		expect(sim?.kind === 'sim' && sim.models.shock_plant.map((m) => m.name)).toEqual(SIM_MODELS.shock_plant.map((m) => m.name));
		expect(sim?.kind === 'sim' && sim.models.sine_structure.length).toBeGreaterThan(0);
		expect(acq.selected?.simulator).toEqual({ shock_plant: null, sine_structure: null });
		expect(acq.stationDirty).toBe(false);

		acq.chooseModel('shock_plant', 'shock-jointed-6x3');
		acq.chooseModel('sine_structure', 'sine-weak-4x4');
		expect(acq.stationDirty).toBe(true);
		expect(await acq.useStation()).toBe(true);
		expect(acq.selected?.kind).toBe('sim');
		expect(acq.selected?.simulator).toEqual({ shock_plant: 'shock-jointed-6x3', sine_structure: 'sine-weak-4x4' });
		expect(acq.stationDraft.simulator).toEqual({ shock_plant: 'shock-jointed-6x3', sine_structure: 'sine-weak-4x4' });
		expect(acq.stationDirty).toBe(false);

		// a model chosen and put back is the one in use again
		acq.chooseModel('shock_plant', null);
		expect(acq.stationDirty).toBe(true);
		acq.revertStation();
		expect(acq.stationDraft.simulator.shock_plant).toBe('shock-jointed-6x3');
		expect(acq.stationDirty).toBe(false);

		// sized to the channel table again
		acq.chooseModel('shock_plant', null);
		acq.chooseModel('sine_structure', null);
		expect(await acq.useStation()).toBe(true);
		expect(acq.selected?.simulator).toEqual({ shock_plant: null, sine_structure: null });
	});

	it("refuses a model the simulator does not play, at its field, and models on NI-DAQmx devices", async () => {
		const { client, acq } = await controller();
		expect(await refusal(client, { kind: 'sim', simulator: { shock_plant: 'shock-none', sine_structure: null } })).toEqual(['bad_params', 'simulator.shock_plant']);
		expect(await refusal(client, { kind: 'sim', simulator: { shock_plant: null, sine_structure: 'sine-none' } })).toEqual(['bad_params', 'simulator.sine_structure']);
		// a sine structure is not a shock plant
		expect(await refusal(client, { kind: 'sim', simulator: { shock_plant: 'sine-weak-4x4' } })).toEqual(['bad_params', 'simulator.shock_plant']);
		expect(await refusal(client, { kind: 'sim', simulator: 'shock-linear-4x4' })).toEqual(['bad_params', 'simulator']);
		expect(await refusal(client, { kind: 'ni', devices: ['cDAQ1Mod1'], simulator: { shock_plant: 'shock-linear-4x4' } })).toEqual(['bad_params', 'simulator']);
		// nothing refused changed the station
		await acq.sync();
		expect(acq.selected?.kind).toBe('sim');
		expect(acq.selected?.simulator).toEqual({ shock_plant: null, sine_structure: null });
		expect(await refusal(client, { kind: 'sim', simulator: { shock_plant: 'shock-linear-4x4' } })).toBeNull();
	});
});

describe('no test type holds the station', () => {
	const sources = import.meta.glob('/src/lib/**/*.svelte', { query: '?raw', import: 'default', eager: true }) as Record<string, string>;

	it('mounts the station picker, the simulator models and the Recorder sim source in Configure only', () => {
		const files = Object.keys(sources);
		expect(files.length).toBeGreaterThan(50);
		const using = (what: RegExp) => files.filter((f) => what.test(sources[f])).map((f) => f.replace('/src/lib/', ''));
		expect(using(/import StationPanel from/)).toEqual(['acquire/ConfigureScreen.svelte']);
		expect(using(/import RecorderSimSource from/)).toEqual(['acquire/ConfigureScreen.svelte']);
		expect(using(/ProvenanceBadge/)).toEqual([]);
		expect(using(/Station &amp; Channels|Station & Channels/)).toEqual([]);
		// the Configure button is on the chooser, the screen header, and MIMO Random's header
		expect(using(/import ConfigureButton from/).sort()).toEqual(['acquire/TestTypeChooser.svelte', 'acquire/mimo/MimoRandomScreen.svelte', 'acquire/screen/ScreenHeader.svelte']);
	});

	it('opens a layout kept with the old station card, without it', () => {
		const offer = { types: SINE_HOOKS.types, cards: SINE_HOOKS.cards };
		expect(offer.cards).not.toContain('station');
		const kept = JSON.parse(toSaved(SINE_HOOKS.defaultLayout(1.4)));
		kept.dock.cards.push({ kind: 'station', open: true, shown: true });
		kept.pages[0].panels = [{ id: 'cd9', kind: 'card', card: 'station', at: { x: 0, y: 0, w: 4, h: 4 } }, ...kept.pages[0].panels];
		const l = fromSaved(JSON.stringify(kept), offer, 1.4);
		expect(l).not.toBeNull();
		expect(l!.pages.length).toBe(kept.pages.length);
		expect(l!.dock.cards.map((c) => c.kind)).toEqual([...SINE_HOOKS.cards]);
		expect(l!.pages.flatMap((p) => p.panels).some((x) => x.kind === 'card' && x.card === 'station')).toBe(false);
		expect(l!.pages[0].panels.length).toBe(SINE_HOOKS.defaultLayout(1.4).pages[0].panels.length);
	});
});
