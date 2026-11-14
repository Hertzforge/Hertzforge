/**
 * The Recorder store over the real client and codec, against the mock control service: the dry run
 * of the draft with the service's derived numbers and its findings on their sections, the setup
 * applied, each trigger mode run on the stated source (from the start, by hand with the
 * pre-window, on a level with its count and holdoff), each verb's answer as the service gave it,
 * every refusal a verb meets (the wrong state and the wrong mode with their facts, an observer,
 * too little free space), the run slot shared with the other test types, the marks and the
 * overload indicator, a window's subscription, the safe stop with nobody in control, and a stale
 * reply that must not take a finished run back. The mock refuses as the service does; these tests
 * hold it to that.
 */
import { describe, expect, it } from 'vitest';
import { RequestError, ServiceClient } from '$lib/protocol/client.svelte';
import { createControlServer, type ControlServerOptions } from '$lib/protocol/mock/control';
import { mockSocketFactory } from '$lib/protocol/mock/socket';
import type { RecorderData, RecorderLive, RecorderState } from '$lib/protocol/recorder';
import { ExplorerStore } from '../explorer/explorerStore.svelte';
import { AcquireState } from '../store.svelte';
import { RecorderStore } from './recorderStore.svelte';
import { RecorderViewsState } from './recorderViews.svelte';
import { recorderCardStatus } from './recorderWords';

const until = async (cond: () => boolean, ms = 8000) => {
	const t0 = Date.now();
	while (!cond()) {
		if (Date.now() - t0 > ms) throw new Error('timed out waiting');
		await new Promise((r) => setTimeout(r, 5));
	}
};

/**
 * A screen on the mock service with four recorded rows: the control row and three inputs, 5 g of
 * range each (so the default hysteresis is 0.05 g), and the drive row left in place.
 */
async function screen(options: ControlServerOptions = {}, range = 5) {
	const server = createControlServer({ speed: 20, ...options });
	const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(server, 1) });
	const acq = new AcquireState(client, { session: null, local: null, checkDelayMs: 0 });
	client.connect();
	await client.whenOpen();
	await acq.sync();
	await acq.claim();
	const rows = acq.rows.map((r) => ({ ...r, abort: { ...r.abort }, range_max: r.role === 'drive' ? r.range_max : range }));
	for (const index of [3, 4, 5]) rows.push({ ...rows[0], abort: { ...rows[0].abort }, index, description: `Input ${index}`, role: 'monitor', physical: null });
	acq.rows = rows;
	expect(await acq.applyTable()).toBe(true);
	const recorder = new RecorderStore(acq, { local: null, checkDelayMs: 0 });
	await recorder.sync();
	/** A second screen on the same service, observing. */
	const observer = async () => {
		const c = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(server, 1) });
		c.connect();
		await c.whenOpen();
		return c;
	};
	return { client, acq, recorder, observer };
}

const refusal = async (p: Promise<unknown>) => (await p.catch((e: unknown) => e)) as RequestError;

/** PROTOCOL's level trigger on the markers: row 1 rising through 0.25 g, 0.5 s before, 1.0 s after. */
function level(recorder: RecorderStore, count: number | null = 3, holdoff = 0) {
	Object.assign(recorder.setup.trigger, { mode: 'level', row: 1, level: 0.25, slope: 'rising', hysteresis: null, pre_s: 0.5, record_s: 1, count, holdoff_s: holdoff });
}

async function running(recorder: RecorderStore) {
	if (recorder.setupStatus !== 'applied') expect(await recorder.apply()).toBe(true);
	expect(await recorder.start()).toBe(true);
	await until(() => recorder.status?.state === 'running');
}

describe('the Recorder store', () => {
	it("shows the service's derived numbers for the Recorder draft from the dry run, never its own", async () => {
		const { recorder } = await screen();
		level(recorder, 5);
		recorder.setup.analysis.frf = { reference: 3, response: 5 };
		await recorder.checkSetup();
		const d = recorder.derived!;
		expect(d.grid).toEqual({ fs_hz: 12800, block_frames: 256, batch_frames: 512, group_delay_frames: 0, actual: false });
		expect([d.outputs, d.rows]).toEqual(['none', [1, 3, 4, 5]]);
		expect(d.analysis).toEqual({ fs_hz: 12800, df_hz: 6.25, lines: 800, span_hz: 5000, block_s: 0.16, overlap_pct: 50, averages_s: 0.72 });
		expect(d.trigger).toEqual({ pre_frames: 6400, record_frames: 12800, holdoff_frames: 0 });
		expect([d.disk.bytes_per_s, d.disk.floor_bytes, d.disk.free_bytes, d.disk.seconds_above_floor]).toEqual([204800, 1073741824, 107374182400, 519045]);
		// the draft changed: the numbers checked for the old one are not shown, and none is computed here
		recorder.setup.analysis.decimation = 4;
		expect(recorder.derived).toBeNull();
		await recorder.checkSetup();
		expect([recorder.derived?.analysis.fs_hz, recorder.derived?.analysis.span_hz, recorder.derived?.analysis.df_hz]).toEqual([3200, 1250, 1.5625]);
		expect(recorder.check?.warnings.map((w) => [w.reason, recorder.counts.station.warnings])).toEqual([['drive_channel_present', 1]]);
	});

	it('lands each Recorder configuration finding on its field and section, by the names the service uses', async () => {
		const { recorder, acq } = await screen();
		level(recorder);
		Object.assign(recorder.setup.trigger, { row: 2, pre_s: 12, record_s: 0 });
		recorder.setup.analysis.block = 3000;
		recorder.setup.analysis.frf = { reference: 1, response: 1 };
		const res = await recorder.checkSetup();
		expect(res?.errors.map((e) => [e.reason, e.field])).toEqual([
			['row_not_recorded', 'trigger.row'],
			['out_of_range', 'trigger.pre_s'],
			['out_of_range', 'trigger.record_s'],
			['not_power_of_two', 'analysis.block'],
			['frf_same_row', 'analysis.frf']
		]);
		expect([recorder.counts.trigger.errors, recorder.counts.analysis.errors]).toEqual([3, 2]);
		Object.assign(recorder.setup.trigger, { row: 1, level: 6, pre_s: 0.5, record_s: 1 });
		recorder.setup.analysis.block = 2048;
		recorder.setup.analysis.frf = null;
		expect((await recorder.checkSetup())?.errors.map((e) => [e.reason, e.field])).toEqual([['out_of_range', 'trigger.level']]);
		expect(recorder.fieldErrors['trigger.level']?.[0]).toMatch(/5 g either side/);
		// no recorded row: the table's own finding, on the station's section
		acq.rows = acq.rows.map((r) => ({ ...r, enabled: r.role === 'drive' }));
		expect(await acq.applyTable()).toBe(true);
		recorder.setup.trigger.mode = 'manual';
		const none = await recorder.checkSetup();
		expect(none?.errors.map((e) => e.reason)).toContain('no_input_channel');
		expect(recorder.counts.station.errors).toBeGreaterThan(0);
	});

	it('refuses a Recorder configuration by its shape with the parameter: a key it does not have, a level trigger with no length', async () => {
		const { client } = await screen();
		const e = await refusal(client.request('recorder.configure', { config: { trigger: { mode: 'free_run' } }, dry_run: true }));
		expect([e.reason, e.detail?.param]).toEqual(['bad_params', 'config.trigger.mode']);
		const f = await refusal(client.request('recorder.configure', { config: { trigger: { mode: 'level', row: 1, level: 0.25 } }, dry_run: true }));
		expect([f.reason, f.detail?.param]).toEqual(['bad_params', 'config.trigger.record_s']);
		const g = await refusal(client.request('recorder.configure', { config: { capture: { data_root: '/tmp' } }, dry_run: true }));
		expect([g.reason, g.detail?.param]).toEqual(['bad_params', 'config.capture.data_root']);
	});

	it('records a Recorder run from its first frame for exactly its length, then monitors', async () => {
		const { recorder } = await screen();
		Object.assign(recorder.setup.trigger, { mode: 'start', record_s: 2 });
		await running(recorder);
		expect(recorder.status?.phase).toBe('recording');
		expect(recorder.status?.recording).toMatchObject({ name: 'rec_0001', trigger: 'start', first_station_frame: 0, trigger_station_frame: 0, pre_frames: 0, target_frames: 25600, length_s: 2 });
		expect(recorder.status?.trigger).toBeNull();
		await until(() => recorder.now?.phase === 'monitoring' && recorder.recordings.length === 1 && recorder.recordings[0].complete);
		const r = recorder.recordings[0];
		expect([r.frames, r.seconds, r.bytes, r.complete, r.gap_free, r.folder]).toEqual([25600, 2, 409600, true, true, 'streams/rec_0001']);
		expect(recorderCardStatus(recorder.now, recorder.cardSetup).text).toBe('Monitoring: nothing is being saved');
		await until(() => !!recorder.folder);
		expect(recorder.folder).toMatch(/Hertzforge_Data\/recorder$/);
		// the folder is the state's, not a log line's: a line naming another changes nothing
		expect(recorder.folder).toBe(recorder.status?.record?.folder);
		recorder.log = [{ seq: 9999, at_unix: 0, kind: 'note', text: 'records in /elsewhere' }, ...recorder.log];
		expect(recorder.folder).toBe(recorder.status?.record?.folder);
		expect(recorder.log.map((e) => e.text)).toEqual(expect.arrayContaining(['the station programmed 12800 Hz (asked 12800 Hz)', 'outputs: none; drive row 2 not opened', 'rec_0001 began at station frame 0 (pre-window 0 frames)', 'rec_0001 saved: 25600 frames, 2.000 s, no gaps']));
		await recorder.stop();
		await until(() => recorder.status?.state === 'finished');
		expect([recorder.status?.end?.outcome, recorder.status?.end?.recordings, recorder.status?.phase]).toEqual(['operator_stop', 1, 'stopped']);
	}, 15000);

	it('records a manual Recorder recording with the pre-window it holds, and cuts it at the station frame it answers', async () => {
		const { recorder } = await screen();
		recorder.setup.trigger.pre_s = 1;
		await running(recorder);
		expect(recorderCardStatus(recorder.now, recorder.cardSetup).text).toBe('Monitoring: nothing is being saved');
		const armed = (await recorder.armTrigger())!;
		expect(armed).toEqual({ pre_frames: 12800, held_frames: 0 });
		expect(recorder.answer?.words).toBe('Armed: the pre-window of 1.00 s fills from the next batch');
		await until(() => (recorder.now?.trigger?.held_frames ?? 0) >= 12800);
		expect(recorderCardStatus(recorder.now, recorder.cardSetup).text).toBe('Armed: Record begins with the pre-window (pre-window 1.00 s of 1.00 s)');
		const rec = (await recorder.record())!;
		expect([rec.name, rec.pre_frames]).toEqual(['rec_0001', 12800]);
		expect(rec.station_frame % 512).toBe(0);
		expect(recorder.answer?.words).toBe(`rec_0001 began at station frame ${rec.station_frame}, with 1.00 s before it`);
		await until(() => (recorder.now?.recording?.seconds ?? 0) > 1);
		expect(recorderCardStatus(recorder.now, recorder.cardSetup).text).toMatch(/^Recording rec_0001: \d+\.\d\d s, until cut$/);
		const cut = (await recorder.cut())!;
		expect(cut.name).toBe('rec_0001');
		await until(() => recorder.recordings.some((r) => r.complete));
		const r = recorder.recordings[0];
		expect([r.first_station_frame, r.trigger_station_frame, r.pre_frames, r.pre_frames_asked]).toEqual([rec.station_frame - 12800, rec.station_frame, 12800, 12800]);
		expect(r.frames).toBe(cut.last_station_frame - r.first_station_frame + 1);
		// a record not armed has no pre-window, and the run returns to monitoring, not armed
		await until(() => recorder.now?.phase === 'monitoring');
		const bare = (await recorder.record())!;
		expect(bare.pre_frames).toBe(0);
		await recorder.cut();
		await recorder.abort();
		await until(() => recorder.status?.state === 'finished');
		expect([recorder.status?.end?.outcome, recorder.status?.end?.recordings]).toEqual(['operator_abort', 2]);
	}, 15000);

	it('fires a Recorder level trigger at each marker for its count, each recording exactly its pre-window and length', async () => {
		const { recorder } = await screen();
		level(recorder, 3);
		await running(recorder);
		await recorder.armTrigger();
		await until(() => recorder.now?.phase === 'armed');
		expect(recorderCardStatus(recorder.now, recorder.cardSetup).text).toMatch(/^Armed: waiting for row 1 to rise through 0\.250 g \(pre-window \d\.\d\d s of 0\.50 s\)$/);
		await until(() => recorder.recordings.filter((r) => r.complete).length === 3 && recorder.now?.phase === 'monitoring', 10000);
		const recs = recorder.recordings;
		const trig = recs.map((r) => r.trigger_station_frame);
		// the first sample at or over 0.25 g: 22 frames into a marker that begins at 6400 + 25600 k
		for (const t of trig) expect((t - 6400) % 25600).toBe(22);
		expect(trig.slice(1).map((t, i) => t - trig[i])).toEqual([25600, 25600]);
		// each holds its pre-window and exactly 1.0 s after; a re-armed trigger waits for a full fresh pre-window
		for (const r of recs) expect([r.frames - r.pre_frames, r.trigger, r.trigger_station_frame - r.first_station_frame]).toEqual([12800, 'level', r.pre_frames]);
		expect(recs.slice(1).map((r) => r.pre_frames)).toEqual([6400, 6400]);
		expect(recs[0].pre_frames).toBeLessThanOrEqual(6400);
		expect([recorder.now?.trigger?.fires, recorder.now?.trigger?.saved, recorder.now?.trigger?.left, recorder.now?.trigger?.armed]).toEqual([3, 3, 0, false]);
		await recorder.stop();
	}, 20000);

	it('waits out a Recorder holdoff in station time from the last trigger', async () => {
		const { recorder } = await screen();
		level(recorder, 2, 3);
		await running(recorder);
		await recorder.armTrigger();
		await until(() => recorder.recordings.filter((r) => r.complete).length === 2, 15000);
		const [a, b] = recorder.recordings.map((r) => r.trigger_station_frame);
		expect(b - a).toBe(51200);
		await recorder.stop();
	}, 20000);

	it('refuses each Recorder verb outside its trigger mode or its phase, naming the mode or the state', async () => {
		const { recorder, client } = await screen();
		level(recorder);
		expect(await recorder.apply()).toBe(true);
		expect(await recorder.record()).toBeNull();
		expect([recorder.refusal?.reason, recorder.refusal?.detail?.state]).toEqual(['wrong_mode', undefined]);
		const before = await refusal(client.request('recorder.cut', {}));
		expect([before.reason, before.detail?.state, before.detail?.phase]).toEqual(['wrong_state', 'configured', 'idle']);
		await running(recorder);
		expect(await recorder.record()).toBeNull();
		expect([recorder.refusal?.reason, recorder.refusal?.title, recorder.refusal?.facts]).toEqual(['wrong_mode', 'Not in This Trigger Mode', ['Trigger: Level']]);
		expect(await recorder.disarm()).toBe(false);
		expect([recorder.refusal?.reason, recorder.refusal?.facts]).toEqual(['wrong_state', ['State: Running, Monitoring: nothing is being saved']]);
		await recorder.armTrigger();
		const again = await refusal(client.request('recorder.arm', {}));
		expect([again.reason, again.detail?.phase]).toEqual(['wrong_state', 'armed']);
		await recorder.stop();
		await until(() => recorder.status?.state === 'finished');
		const start = await refusal(client.request('recorder.arm', {}));
		expect(start.reason).toBe('wrong_state');
		// a run from the start takes no arm
		recorder.setup.trigger.mode = 'start';
		await running(recorder);
		const arm = await refusal(client.request('recorder.arm', {}));
		expect([arm.reason, arm.detail?.mode]).toEqual(['wrong_mode', 'start']);
		await recorder.abort();
	}, 15000);

	it('lets a Recorder observer stop and abort and read, but not arm, record or cut', async () => {
		const { recorder, observer } = await screen();
		await running(recorder);
		const o = await observer();
		for (const m of ['recorder.arm', 'recorder.record', 'recorder.disarm', 'recorder.cut']) expect((await refusal(o.request(m, {}))).reason).toBe('not_controller');
		const list = await o.request<{ recordings: unknown[]; next: number }>('recorder.recordings', {});
		expect(list).toEqual({ recordings: [], next: 0 });
		await o.request('recorder.abort', {});
		await until(() => recorder.status?.state === 'finished');
		expect([recorder.status?.end?.outcome, recorder.status?.phase]).toEqual(['operator_abort', 'aborted']);
		expect(recorder.status?.end?.message).toMatch(/^Aborted by client \d+.*: every recording is saved\.$/);
	}, 15000);

	it('refuses a Recorder start with too little free space, with both numbers, and makes nothing', async () => {
		const { recorder } = await screen({ recorderFreeBytes: 512 * 1024 * 1024 });
		expect(await recorder.apply()).toBe(true);
		expect(await recorder.start()).toBe(false);
		expect([recorder.refusal?.reason, recorder.refusal?.title, recorder.refusal?.facts]).toEqual(['not_enough_space', 'Not Enough Free Space', ['Free: 512 MiB', 'Floor: 1 GiB']]);
		expect(recorder.refusal?.detail).toEqual({ free_bytes: 536870912, floor_bytes: 1073741824 });
		expect(recorder.status?.state).toBe('configured');
		expect(recorder.log.some((e) => /records in/.test(e.text))).toBe(false);
	});

	it('holds the Recorder run slot against another test type, and is held off by one', async () => {
		const { recorder, acq } = await screen();
		await running(recorder);
		const explorer = new ExplorerStore(acq, { local: null, checkDelayMs: 0 });
		await explorer.sync();
		explorer.setup.max_drive_vpk = 0.5;
		expect(await explorer.apply()).toBe(true);
		expect(await explorer.start()).toBe(false);
		expect([explorer.refusal?.reason, explorer.refusal?.facts]).toEqual(['run_active', [`Run ${recorder.status?.run_id}`]]);
		await recorder.abort();
		await until(() => recorder.status?.state === 'finished');
		expect(await explorer.start()).toBe(true);
		expect(await recorder.reset()).toBe(false);
		expect(recorder.refusal?.reason).toBe('run_active');
		expect(recorder.refusal?.facts).toEqual([`Run ${explorer.status?.run_id}`]);
		await explorer.abort();
		await until(() => explorer.status?.state === 'finished');
		expect(await recorder.reset()).toBe(true);
	}, 15000);

	it('marks each Recorder limit crossing and goes on recording, and latches the overload indicator on its row only', async () => {
		const { recorder, acq } = await screen();
		const rows = acq.rows.map((r) => ({ ...r, abort: { ...r.abort } }));
		rows.find((r) => r.index === 1)!.abort.accel_g = 0.4;
		rows.find((r) => r.index === 1)!.range_max = 0.5;
		acq.rows = rows;
		expect(await acq.applyTable()).toBe(true);
		Object.assign(recorder.setup.trigger, { mode: 'start', record_s: 6 });
		await running(recorder);
		await until(() => recorder.recordings.some((r) => r.complete), 10000);
		const limits = recorder.marks.filter((m) => m.kind === 'limit');
		expect(limits.length).toBe(3);
		for (const m of limits) expect([m.row, m.axis, m.limit, m.recording, m.value >= 0.4]).toEqual([1, 'accel', 0.4, 'rec_0001', true]);
		expect(limits.slice(1).map((m, i) => m.station_frame - limits[i].station_frame)).toEqual([25600, 25600]);
		const over = recorder.marks.filter((m) => m.kind === 'overload');
		expect(over.map((m) => [m.row, m.level])).toEqual([[1, 0.45]]);
		expect(recorder.recordings[0]).toMatchObject({ frames: 76800, marks: 4, complete: true });
		await until(() => !!recorder.frameOfRun?.rows.length);
		expect(recorder.frameOfRun!.rows.map((r) => r.overload)).toEqual([true, false, false, false]);
		expect(recorder.status?.state).toBe('running');
		await recorder.stop();
	}, 15000);

	it('reads the Recorder run numbers from its own data frames once it has subscribed to plots, as the service sends them', async () => {
		const { recorder, client } = await screen();
		recorder.setup.analysis.frf = { reference: 3, response: 5 };
		expect(await recorder.apply()).toBe(true);
		const lives: number[] = [];
		client.on('recorder.live', () => lives.push(1));
		const datas: RecorderData[] = [];
		client.on('recorder.data', (d) => datas.push(d as unknown as RecorderData));
		const sub = await client.request<{ plots: unknown[]; quantities: string[] }>('recorder.subscribe', { rate_hz: 10, plots: [{ id: 'a', quantity: 'time' }, { id: 'b', quantity: 'spectrum', channels: [3] }, { id: 'c', quantity: 'frf' }, { id: 'd', quantity: 'readings' }] });
		expect(sub.plots).toEqual([
			{ id: 'a', quantity: 'time', channels: 'all', points: 1024, seconds: 2 },
			{ id: 'b', quantity: 'spectrum', channels: [3] },
			{ id: 'c', quantity: 'frf' },
			{ id: 'd', quantity: 'readings' }
		]);
		expect(sub.quantities).toEqual(['time', 'spectrum', 'frf', 'readings']);
		expect(await recorder.start()).toBe(true);
		await until(() => datas.some((d) => d.plots.every((p) => p.views.length > 0)), 10000);
		const d = datas.at(-1)!;
		const time = d.plots.find((p) => p.quantity === 'time')!;
		expect([typeof time.first_frame, time.channels]).toEqual(['number', [1, 3, 4, 5]]);
		expect(time.views[0].view === 'spectrum' && time.views[0].series.every((s) => s.y_min !== undefined)).toBe(true);
		expect(d.plots.find((p) => p.quantity === 'spectrum')!.channels).toEqual([3]);
		expect(d.plots.find((p) => p.quantity === 'frf')!.views.map((v) => (v.view === 'spectrum' ? v.title : v.view))).toEqual(['FRF', 'Phase', 'Coherence']);
		expect(d.plots.find((p) => p.quantity === 'readings')!.first_frame).toBeNull();
		expect(typeof recorder.frameOfRun?.station_frames).toBe('number');
		expect(lives).toHaveLength(0);
		await recorder.abort();
	}, 15000);

	it("refuses a Recorder plot's key the quantity does not take, and every page the shell offers is accepted", async () => {
		const { client } = await screen();
		for (const [plot, param] of [
			[{ id: 'a', quantity: 'frf', channels: 'all' }, 'plots[0].channels'],
			[{ id: 'a', quantity: 'spectrum', points: 512 }, 'plots[0].points'],
			[{ id: 'a', quantity: 'spectrum', seconds: 1 }, 'plots[0].seconds'],
			[{ id: 'a', quantity: 'time', seconds: 3 }, 'plots[0].seconds'],
			[{ id: 'a', quantity: 'time', points: 10 }, 'plots[0].points'],
			[{ id: 'a', quantity: 'survey' }, 'plots[0].quantity']
		] as const) {
			const e = await refusal(client.request('recorder.subscribe', { plots: [plot] }));
			expect([e.reason, e.detail?.param]).toEqual(['bad_params', param]);
		}
		const views = new RecorderViewsState(client, { setupTitle: () => 'x', showing: () => true, storage: null, session: null, events: null, subscribeDelayMs: 0 });
		expect(views.pages.map((p) => p.name)).toEqual(['Time', 'Spectra', 'FRF', 'Readings', 'Recordings']);
		for (const page of views.pages) {
			views.selectPage(page.id);
			const params = views.hooks.feed!.params(views.wanted, views.layout.rate);
			const res = await client.request<{ default: boolean }>('recorder.subscribe', params);
			expect(res.default).toBe(page.name === 'Recordings');
		}
		// time and spectra asked for the control channels, which the service does not take, are sent as every row
		views.selectPage(views.pages[0].id);
		const spectrum = views.addPlot('spectrum') as string;
		views.updatePlot(spectrum, { channels: 'control' });
		const sent = views.hooks.feed!.params(views.wanted, views.layout.rate) as { plots: { quantity: string; channels?: unknown }[] };
		expect(sent.plots.map((p) => p.channels)).toEqual(['all', 'all']);
		await expect(client.request('recorder.subscribe', sent)).resolves.toMatchObject({ default: false });
	});

	it('stops a Recorder run with every recording saved once nobody has held control for the timeout', async () => {
		const { recorder, acq } = await screen({ timeoutS: 0.5 });
		Object.assign(recorder.setup.trigger, { mode: 'start', record_s: null });
		await running(recorder);
		await acq.release();
		await until(() => recorder.status?.state === 'finished', 5000);
		expect([recorder.status?.stop, recorder.status?.end?.outcome, recorder.status?.end?.recordings]).toEqual(['safe_stop', 'safe_stop', 1]);
		expect(recorder.status?.end?.message).toBe('Nobody held control: the run stopped and every recording is saved.');
		await until(() => recorder.recordings.length === 1 && recorder.recordings[0].complete);
		expect(recorder.recordings[0].frames).toBeGreaterThan(0);
	}, 15000);

	it('keeps a finished Recorder run finished when a stale reply or frame arrives after it', async () => {
		const { recorder } = await screen();
		await running(recorder);
		const live = recorder.status!;
		const finished = { ...live, state: 'finished' } as RecorderState;
		const stale = { ...live, state: 'stopping' } as RecorderState;
		const host = recorder as unknown as { onStatus(s: RecorderState): void; onFrame(l: RecorderLive): void };
		host.onStatus(finished);
		host.onStatus(stale);
		expect(recorder.status?.state).toBe('finished');
		host.onFrame({ ...(recorder.frameOfRun ?? ({} as RecorderLive)), run_id: live.run_id!, state: 'running', phase: 'monitoring' } as RecorderLive);
		expect(recorder.now?.state).toBe('finished');
		await recorder.abort();
	});

	it("reads a new Recorder run's log on its own: the second run's folder is its own, nothing of the last run's kept", async () => {
		const { recorder } = await screen();
		Object.assign(recorder.setup.trigger, { mode: 'start', record_s: 0.5 });
		await running(recorder);
		await until(() => recorder.log.some((e) => /^rec_0001 saved/.test(e.text)));
		await recorder.stop();
		await until(() => recorder.status?.state === 'finished');
		expect(recorder.folder).toMatch(/\/recorder$/);
		const first = recorder.status!.run_id;
		expect(await recorder.reset()).toBe(true);
		expect(await recorder.start()).toBe(true);
		await until(() => recorder.log.some((e) => /^records in /.test(e.text)) && recorder.status?.run_id !== first);
		await until(() => recorder.folder?.endsWith('/recorder-2') ?? false, 3000);
		expect(recorder.log.filter((e) => /^records in /.test(e.text)).map((e) => e.text)).toEqual(['records in ~/Hertzforge_Data/recorder-2']);
		await recorder.abort();
	}, 15000);

	it('round-trips every Recorder setup field through what the service holds', async () => {
		const { recorder } = await screen();
		level(recorder, null, 2);
		recorder.setup.title = 'Fixture A';
		recorder.setup.trigger.hysteresis = 0.02;
		recorder.setup.trigger.slope = 'falling';
		recorder.setup.trigger.level = -0.25;
		recorder.setup.analysis = { block: 4096, window: 'flattop', overlap_pct: 60, averaging: 'exponential', averages: 16, decimation: 2, frf: { reference: 3, response: 4 } };
		recorder.setup.capture = { run_name: 'fixture_a', keep_free_gb: 2 };
		recorder.setup.metadata = [{ key: 'run_number', label: 'Run Number', value: '3' }];
		expect(await recorder.apply()).toBe(true);
		await until(() => recorder.served?.config?.title === 'Fixture A');
		const wire = structuredClone(recorder.wire);
		recorder.setup = { ...recorder.setup, title: 'changed' };
		expect(recorder.setupStatus).toBe('changed');
		expect(recorder.adoptServed()).toBe(true);
		expect(recorder.wire).toEqual(wire);
		expect(recorder.setupStatus).toBe('applied');
	});
});
