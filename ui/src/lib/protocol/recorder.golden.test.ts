/**
 * The Recorder goldens (schema/golden/) through the shell's codec, types, setup, layout and words.
 * The files are read from the folder itself, so this suite, the Rust suite and the Python suite
 * all read the same frames: a golden added there is checked here without a list to update. Each
 * check that names a golden runs when that golden is in the folder. The last part holds the mock
 * control service's frames to the service's form.
 */
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { decodeFrame, encodeFrame, withShape } from './codec';
import type { Dtype, Event, ReadingView, Request, ResponseErr, ResponseOk, View } from './types';
import {
	RECORDER_TOPICS,
	type RecorderConfigureParams,
	type RecorderConfigureResult,
	type RecorderCutResult,
	type RecorderData,
	type RecorderLive,
	type RecorderLiveQuantity,
	type RecorderRecordResult,
	type RecorderRecordingsResult,
	type RecorderState,
	type RecorderSubscribeParams,
	type RecorderSubscribeResult
} from './recorder';
import { ServiceClient } from './client.svelte';
import { createControlServer } from './mock/control';
import { mockSocketFactory } from './mock/socket';
import { AcquireState } from '$lib/acquire/store.svelte';
import { defaultPulseScreen, pulseSubscription } from '$lib/acquire/pulse/pulseScreen';
import { RECORDER_PLOT_TYPES, RECORDER_QUANTITIES, RECORDER_SCREEN } from '$lib/acquire/recorder/recorderLayout';
import type { PlotPanel } from '$lib/acquire/screen/screen.svelte';
import { defaultRecorderSetup, fromRecorderConfig, recorderByKey, recorderSectionOf, toRecorderConfig } from '$lib/acquire/recorder/recorderSetup';
import { RecorderStore } from '$lib/acquire/recorder/recorderStore.svelte';
import { recorderCardStatus, RECORDER_OUTCOME_WORDS, savedWords } from '$lib/acquire/recorder/recorderWords';

const DIR = fileURLToPath(new URL('../../../../schema/golden/', import.meta.url));
const golden = (name: string) => `${DIR}${name}`;
const has = (name: string) => existsSync(golden(name));
const readJson = (name: string) => JSON.parse(readFileSync(golden(name), 'utf-8'));
const readBin = (name: string) => new Uint8Array(readFileSync(golden(name)));

/** Every Recorder golden frame in the folder (a golden recording's folder is the reader's, not the codec's). */
const RECORDER = readdirSync(DIR).filter((f) => /(^|_)recorder(_|\.)/.test(f) && statSync(golden(f)).isFile());
const TEXT = RECORDER.filter((f) => f.endsWith('.json') && !f.endsWith('.expected.json')).sort();
const BIN = RECORDER.filter((f) => f.endsWith('.bin')).sort();
const QUANTITIES: readonly RecorderLiveQuantity[] = RECORDER_QUANTITIES;
/** The Run reading's rows, as PROTOCOL names them. */
const RUN_ROWS = ['Rate', 'Asked', 'Group delay', 'Recording', 'Recorded', 'Backlog', 'Saved', 'Fires', 'Marks', 'Free above floor'];
/** The state's fields, as PROTOCOL lists them. */
const STATE_KEYS = ['clock', 'disk', 'end', 'grid', 'health', 'marks', 'mode', 'outputs', 'phase', 'record', 'recording', 'recordings', 'rows', 'run_id', 'source', 'state', 'station', 'station_frames', 'stop', 't_s', 'title', 'trigger'];
const HEAD_KEYS = ['class', 'disk', 'grid', 'health', 'marks', 'phase', 'recording', 'recordings', 'rows', 'run_id', 'source', 'state', 'station_frames', 't_s', 'trigger'];
const RECORDING_KEYS = ['bytes', 'complete', 'fault', 'first_station_frame', 'folder', 'frames', 'fs_hz', 'gap_free', 'gaps', 'group_delay_frames', 'marks', 'name', 'pre_frames', 'pre_frames_asked', 'requested_fs_hz', 'rows', 'seconds', 'trigger', 'trigger_station_frame'];
const ITEM_REASONS = ['out_of_range', 'not_power_of_two', 'row_not_recorded', 'frf_same_row', 'no_input_channel'];
/** The quantities that take `channels`, and the one that takes `points` and `seconds`, as PROTOCOL states them. */
const TAKES_CHANNELS: readonly RecorderLiveQuantity[] = ['time', 'spectrum'];

type Expected = { dtype: Dtype; shape: number[]; values: number[] };
const isExpected = (v: unknown): v is Expected => typeof v === 'object' && v !== null && 'dtype' in v && 'values' in v;
function toTyped(node: unknown): unknown {
	if (isExpected(node)) {
		const ctor = { f32: Float32Array, f64: Float64Array, i32: Int32Array, u32: Uint32Array, u8: Uint8Array }[node.dtype];
		return withShape(ctor.from(node.values), node.shape);
	}
	if (Array.isArray(node)) return node.map(toTyped);
	if (node !== null && typeof node === 'object') return Object.fromEntries(Object.entries(node).map(([k, v]) => [k, toTyped(v)]));
	return node;
}

/**
 * A plot's channels, point count and seconds, each only where the quantity takes it. The shell's
 * own subscriptions name the rows of every time and spectrum plot and send a point count with the
 * time history only (its seconds take the service's default).
 */
function keysAsTheServiceTakes(plots: { quantity: RecorderLiveQuantity; channels?: unknown; points?: number | null; seconds?: number }[], shells: boolean) {
	for (const p of plots) {
		const hasChannels = 'channels' in p && p.channels !== undefined;
		if (hasChannels) expect(TAKES_CHANNELS).toContain(p.quantity);
		if (shells) expect(hasChannels).toBe(TAKES_CHANNELS.includes(p.quantity));
		if (p.quantity !== 'time') {
			expect('points' in p && p.points !== undefined).toBe(false);
			expect('seconds' in p && p.seconds !== undefined).toBe(false);
		}
		if (shells && p.quantity === 'time') expect(typeof p.points).toBe('number');
	}
}

/** The form of a view the shell compares with the service's: titles, axes, series names and roles. */
function shapeOf(v: View): unknown {
	if (v.view === 'spectrum') return { view: v.view, title: v.title, x: v.x, y: v.y, series: v.series.map((s) => [s.name, s.role, s.y_min !== undefined]), link: v.link ?? null };
	if (v.view === 'reading') return { view: v.view, title: v.title, rows: (v as ReadingView).rows.map((r) => r.label) };
	return { view: v.view };
}

const testConfig = () => (has('request_recorder_configure.json') ? (readJson('request_recorder_configure.json') as Request<RecorderConfigureParams>).params.config : null);

describe('Recorder goldens in the folder', () => {
	it('are Recorder text frames or binary frames with their expected form', () => {
		for (const f of BIN) expect(RECORDER).toContain(f.replace(/\.bin$/, '.expected.json'));
		for (const f of RECORDER) expect(f).toMatch(/\.(json|bin)$/);
	});

	for (const name of TEXT) {
		it(`${name} decodes and re-encodes as the same compact Recorder text frame`, () => {
			const text = readFileSync(golden(name), 'utf-8');
			const message = decodeFrame(text);
			expect(message).toEqual(JSON.parse(text));
			expect(encodeFrame(message)).toBe(JSON.stringify(JSON.parse(text)));
		});
	}

	for (const name of BIN) {
		it(`${name} is a Recorder binary frame that decodes to its expected form`, () => {
			const bin = readBin(name);
			expect(new TextDecoder().decode(bin.slice(0, 4))).toBe('SDS1');
			const expected = toTyped(readJson(name.replace(/\.bin$/, '.expected.json'))) as Record<string, unknown>;
			expect(decodeFrame(bin)).toEqual(expected);
		});
	}
});

describe('Recorder goldens through the shell', () => {
	it.skipIf(!has('request_recorder_configure.json'))('a Recorder configuration request survives the setup draft and back, every field', () => {
		const config = testConfig()!;
		expect(toRecorderConfig(fromRecorderConfig(config))).toEqual(config);
	});

	it.skipIf(!has('request_recorder_configure.json'))("the shell's own Recorder setup sends the same fields the service's example has", () => {
		const config = testConfig()!;
		const mine = defaultRecorderSetup();
		mine.trigger.mode = config.trigger?.mode ?? mine.trigger.mode;
		Object.assign(mine.trigger, { row: config.trigger?.row ?? 1, level: config.trigger?.level ?? 0.25, record_s: config.trigger?.record_s ?? 1 });
		const sent = toRecorderConfig(mine);
		expect(Object.keys(sent).sort()).toEqual(Object.keys(config).sort());
		for (const k of ['acquisition', 'trigger', 'analysis', 'capture'] as const) expect(Object.keys(sent[k] ?? {}).sort()).toEqual(Object.keys(config[k] ?? {}).sort());
	});

	it.skipIf(!has('response_recorder_configure.json'))('the Recorder configure result is a configured state with what the service derived', () => {
		const res = (readJson('response_recorder_configure.json') as ResponseOk<RecorderConfigureResult>).result;
		expect([res.state.state, res.state.phase]).toEqual(['configured', 'idle']);
		expect(recorderCardStatus(res.state, null).text).toBe('Set up, ready to start');
		expect(Object.keys(res.state).sort()).toEqual(STATE_KEYS);
		const d = res.derived;
		expect(Object.keys(d).sort()).toEqual(['analysis', 'disk', 'grid', 'outputs', 'rows', 'trigger']);
		expect([d.outputs, d.grid.actual]).toEqual(['none', false]);
		expect(Object.keys(d.grid).sort()).toEqual(['actual', 'batch_frames', 'block_frames', 'fs_hz', 'group_delay_frames']);
		expect(Object.keys(d.analysis).sort()).toEqual(['averages_s', 'block_s', 'df_hz', 'fs_hz', 'lines', 'overlap_pct', 'span_hz']);
		expect(Object.keys(d.trigger).sort()).toEqual(['holdoff_frames', 'pre_frames', 'record_frames']);
		expect(Object.keys(d.disk).sort()).toEqual(['bytes_per_s', 'floor_bytes', 'free_bytes', 'seconds_above_floor']);
		expect(d.grid.batch_frames).toBe(2 * d.grid.block_frames);
	});

	it.skipIf(!has('response_error_recorder_config_invalid.json'))("an invalid Recorder configuration's findings land on the fields and sections they name", () => {
		const err = readJson('response_error_recorder_config_invalid.json') as ResponseErr;
		expect([err.error.code, err.error.reason]).toEqual(['bad_request', 'config_invalid']);
		const items = (err.error.detail as { errors: { reason: string; field?: string | null; message: string }[] }).errors;
		expect(items.length).toBeGreaterThan(0);
		for (const k of Object.keys(recorderByKey(items))) expect(k).not.toMatch(/^config\.|\[/);
		for (const it of items) {
			expect(ITEM_REASONS).toContain(it.reason);
			const section = recorderSectionOf(Object.keys(recorderByKey([it]))[0]);
			const want = it.reason === 'no_input_channel' ? 'station' : /^(capture|metadata|title)/.test(it.field ?? '') ? 'capture' : /^analysis/.test(it.field ?? '') ? 'analysis' : 'trigger';
			expect(section).toBe(want);
		}
	});

	it.skipIf(!has('response_recorder_start.json'))('a Recorder start answers once acquisition runs, with the station figures', () => {
		const s = (readJson('response_recorder_start.json') as ResponseOk<RecorderState>).result;
		expect(s.state).toBe('running');
		expect(['monitoring', 'recording']).toContain(s.phase);
		expect([s.outputs, s.grid?.actual]).toEqual(['none', true]);
		expect(Object.keys(s).sort()).toEqual(STATE_KEYS);
		const words = recorderCardStatus(s, null).text;
		if (s.phase === 'monitoring') expect(words).toBe('Monitoring: nothing is being saved');
		else expect(words).toMatch(/^Recording rec_\d{4}: /);
	});

	it.skipIf(!has('request_recorder_record.json'))('a Recorder record request takes no parameter', () => {
		expect((readJson('request_recorder_record.json') as Request<Record<string, unknown>>).params ?? {}).toEqual({});
	});

	it.skipIf(!has('response_recorder_record.json'))('a Recorder record is answered with the recording, its trigger station frame and its pre-window', () => {
		const r = (readJson('response_recorder_record.json') as ResponseOk<RecorderRecordResult>).result;
		expect(Object.keys(r).sort()).toEqual(['name', 'pre_frames', 'station_frame']);
		expect(r.name).toMatch(/^rec_\d{4}$/);
		expect(r.station_frame).toBeGreaterThanOrEqual(r.pre_frames);
	});

	it.skipIf(!has('response_recorder_cut.json'))('a Recorder cut is answered with the recording and its last station frame', () => {
		const r = (readJson('response_recorder_cut.json') as ResponseOk<RecorderCutResult>).result;
		expect(Object.keys(r).sort()).toEqual(['last_station_frame', 'name']);
		expect(r.name).toMatch(/^rec_\d{4}$/);
	});

	it.skipIf(!has('event_recorder_state.json'))('the Recorder state event has every field and reads in the card words', () => {
		const ev = readJson('event_recorder_state.json') as Event<RecorderState>;
		expect(ev.topic).toBe(RECORDER_TOPICS.state);
		const s = ev.data;
		expect(Object.keys(s).sort()).toEqual(STATE_KEYS);
		if (s.health) expect(Object.keys(s.health).sort()).toEqual(['batch_gap_ms', 'batches', 'blocks_dropped', 'driver_warnings', 'pass_ms', 'station_errors']);
		if (s.trigger) expect(Object.keys(s.trigger).sort()).toEqual(['armed', 'fires', 'held_frames', 'holdoff_left_s', 'last_station_frame', 'left', 'pre_frames', 'saved']);
		if (s.recording) expect(Object.keys(s.recording).sort()).toEqual(['backlog_s', 'first_station_frame', 'frames_fed', 'frames_on_disk', 'length_s', 'name', 'pre_frames', 'pre_frames_asked', 'seconds', 'target_frames', 'trigger', 'trigger_station_frame']);
		if (s.disk) expect(Object.keys(s.disk).sort()).toEqual(['fault', 'floor_bytes', 'free_bytes', 'seconds_above_floor']);
		const words = recorderCardStatus(s, { row: { index: 1, unit: 'g' }, level: 0.25, slope: 'rising' }, s.end).text;
		if (s.state === 'running' && s.phase === 'recording' && s.recording) expect(words).toMatch(/^Recording rec_\d{4}: \d+\.\d\d s(, until cut| of \d+\.\d\d s)$/);
		if (s.state === 'running' && s.phase === 'armed' && s.mode === 'level') expect(words).toMatch(/^Armed: .*waiting for row 1 to rise through 0\.250 g \(pre-window \d+\.\d\d s of \d+\.\d\d s\)$/);
		if (s.end) expect(words).toBe(RECORDER_OUTCOME_WORDS[s.end.outcome]);
	});

	it.skipIf(!has('event_recorder_live.bin'))('the Recorder live frame carries the Run reading and the numbers the card reads', () => {
		const ev = decodeFrame(readBin('event_recorder_live.bin')) as unknown as Event<RecorderLive>;
		expect(ev.topic).toBe(RECORDER_TOPICS.live);
		const l = ev.data;
		expect(l.views.map((v) => [v.view, v.title])).toEqual([['reading', 'Run']]);
		expect((l.views[0] as ReadingView).rows.map((r) => r.label)).toEqual(RUN_ROWS);
		for (const r of (l.views[0] as ReadingView).rows) expect(r.status).toBe('info');
		expect(Object.keys(l).sort()).toEqual([...HEAD_KEYS, 'views'].sort());
		expect(Object.keys(l.grid).sort()).toEqual(['fs_hz', 'group_delay_frames', 'requested_fs_hz']);
		for (const r of l.rows) expect(Object.keys(r).sort()).toEqual(['description', 'index', 'marks', 'overload', 'peak', 'rms', 'role', 'unit']);
	});

	it.skipIf(!has('event_recorder_data.bin'))("a Recorder data frame's plots are quantities the shell draws, rows and first frames only where they apply", () => {
		const ev = decodeFrame(readBin('event_recorder_data.bin')) as unknown as Event<RecorderData>;
		expect(ev.topic).toBe(RECORDER_TOPICS.data);
		expect('views' in ev.data).toBe(false);
		expect(Object.keys(ev.data).sort()).toEqual([...HEAD_KEYS, 'plots'].sort());
		for (const p of ev.data.plots) {
			expect(QUANTITIES).toContain(p.quantity);
			expect(p.channels === null).toBe(p.quantity !== 'time' && p.quantity !== 'spectrum');
			expect(p.first_frame === null).toBe(p.quantity !== 'time' || p.views.length === 0);
			for (const v of p.views) expect(['spectrum', 'reading']).toContain(v.view);
			if (p.quantity === 'time') for (const v of p.views) if (v.view === 'spectrum') for (const s of v.series) expect(s.y_min).toBeDefined();
			if (p.quantity === 'frf' && p.views.length) expect(p.views.map((v) => (v.view === 'spectrum' ? v.title : ''))).toEqual(['FRF', 'Phase', 'Coherence']);
		}
	});

	it.skipIf(!has('request_recorder_subscribe.json'))('a Recorder subscription asks only for quantities the shell knows, each key only where it applies', () => {
		const p = (readJson('request_recorder_subscribe.json') as Request<RecorderSubscribeParams>).params;
		if ('plots' in p) {
			for (const plot of p.plots) expect(QUANTITIES).toContain(plot.quantity);
			keysAsTheServiceTakes(p.plots, false);
		}
	});

	it('every Recorder plot type the shell offers subscribes with each key only where it applies', () => {
		const first = defaultPulseScreen(RECORDER_SCREEN, RECORDER_PLOT_TYPES).pages.flatMap((pg) => pg.panels.filter((x): x is PlotPanel => x.kind === 'plot'));
		const one = RECORDER_PLOT_TYPES.map((t, i): PlotPanel => ({ ...first[0], id: `pl-${i}`, plot: { ...first[0].plot, quantity: t.id } }));
		for (const plots of [first, one]) {
			const sent = pulseSubscription(RECORDER_PLOT_TYPES, plots, 10) as { plots: { quantity: RecorderLiveQuantity; channels?: unknown; points?: number }[] };
			keysAsTheServiceTakes(sent.plots, true);
		}
		expect(pulseSubscription(RECORDER_PLOT_TYPES, [], 10)).toEqual({ default: true });
		expect(RECORDER_PLOT_TYPES.map((t) => t.id)).toEqual([...RECORDER_QUANTITIES]);
	});

	it.skipIf(!has('response_recorder_subscribe.json'))('the Recorder subscription is answered with every default filled in', () => {
		const r = (readJson('response_recorder_subscribe.json') as ResponseOk<RecorderSubscribeResult>).result;
		expect([...r.quantities].sort()).toEqual([...QUANTITIES].sort());
		for (const p of r.plots ?? []) {
			if (p.quantity === 'time' || p.quantity === 'spectrum') expect(p.channels).toBeDefined();
			if (p.quantity === 'time') expect([typeof p.points, typeof p.seconds]).toEqual(['number', 'number']);
		}
	});

	it.skipIf(!has('response_recorder_recordings.json'))('the Recorder recordings list has every field, the one in progress last', () => {
		const r = (readJson('response_recorder_recordings.json') as ResponseOk<RecorderRecordingsResult>).result;
		expect(Object.keys(r).sort()).toEqual(['next', 'recordings']);
		r.recordings.forEach((x, i) => {
			expect(Object.keys(x).sort()).toEqual(RECORDING_KEYS);
			expect(x.folder).toBe(`streams/${x.name}`);
			if (!x.complete) expect(i).toBe(r.recordings.length - 1);
			if (x.complete) expect(savedWords(x)).toMatch(/^Saved rec_\d{4}: \d+\.\d\d s, /);
		});
	});
});

describe("the mock control service's Recorder frames have the service's form", () => {
	const until = async (cond: () => boolean, ms = 8000) => {
		const t0 = Date.now();
		while (!cond()) {
			if (Date.now() - t0 > ms) throw new Error('timed out waiting');
			await new Promise((r) => setTimeout(r, 5));
		}
	};

	it.skipIf(!has('event_recorder_data.bin') || !has('event_recorder_live.bin') || !has('request_recorder_subscribe.json'))('the Recorder subscription echo, the live views and every quantity of a data frame', async () => {
		const server = createControlServer({ speed: 20 });
		const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(server, 1) });
		const acq = new AcquireState(client, { session: null, local: null, checkDelayMs: 0 });
		client.connect();
		await client.whenOpen();
		await acq.sync();
		await acq.claim();
		// the goldens' table: rows 1 to 4 recorded at a 5 g range, row 5 the drive
		const [control, drive] = acq.rows;
		const inputs = ['Fixture', 'Bracket', 'Cover'].map((description, k) => ({ ...control, index: k + 2, description, role: 'monitor' as const, physical: null, range_max: 5 }));
		acq.rows = [{ ...control, description: 'Table', range_max: 5 }, ...inputs, { ...drive, index: 5 }];
		expect(await acq.applyTable()).toBe(true);
		const recorder = new RecorderStore(acq, { local: null, checkDelayMs: 0 });
		await recorder.sync();
		const config = testConfig();
		recorder.setup = config ? fromRecorderConfig(config) : recorder.setup;
		expect(await recorder.apply()).toBe(true);
		const want = (readJson('request_recorder_subscribe.json') as Request<RecorderSubscribeParams>).params as { rate_hz: number; plots: unknown[] };
		let data: RecorderData | null = null;
		client.on(RECORDER_TOPICS.data, (d) => (data = d as unknown as RecorderData));
		const observer = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(server, 1) });
		observer.connect();
		await observer.whenOpen();
		let observedLive: RecorderLive | null = null;
		observer.on(RECORDER_TOPICS.live, (d) => (observedLive = d as unknown as RecorderLive));
		const echo = await client.request('recorder.subscribe', { rate_hz: want.rate_hz, plots: want.plots });
		if (has('response_recorder_subscribe.json')) expect(echo).toEqual((readJson('response_recorder_subscribe.json') as ResponseOk<RecorderSubscribeResult>).result);
		expect(await recorder.start()).toBe(true);
		await until(() => !!data && data.plots.every((p) => p.views.length > 0) && !!observedLive, 10000);
		const theirs = (decodeFrame(readBin('event_recorder_data.bin')) as unknown as Event<RecorderData>).data;
		const mine = data as unknown as RecorderData;
		expect(Object.keys(mine).sort()).toEqual(Object.keys(theirs).sort());
		for (const p of theirs.plots) {
			const m = mine.plots.find((x) => x.id === p.id)!;
			expect(m.quantity).toBe(p.quantity);
			if (p.views.length) expect(m.views.map(shapeOf)).toEqual(p.views.map(shapeOf));
		}
		const live = (decodeFrame(readBin('event_recorder_live.bin')) as unknown as Event<RecorderLive>).data;
		expect(Object.keys(observedLive as unknown as RecorderLive).sort()).toEqual(Object.keys(live).sort());
		expect((observedLive as RecorderLive | null)?.views.map(shapeOf)).toEqual(live.views.map(shapeOf));
		await recorder.abort();
	}, 20000);
});
