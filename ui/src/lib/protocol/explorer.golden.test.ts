/**
 * The Explorer goldens (schema/golden/) through the shell's codec, types, setup, layout and words.
 * The files are read from the folder itself, so this suite, the Rust suite and the Python suite
 * all read the same frames: a golden added there is checked here without a list to update. Each
 * check that names a golden runs when that golden is in the folder. The last part holds the mock
 * control service's frames to the service's form.
 */
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { decodeFrame, encodeFrame, withShape } from './codec';
import type { Dtype, Event, ReadingView, Request, ResponseErr, ResponseOk, View } from './types';
import {
	EXPLORER_TOPICS,
	type ExplorerConfigureParams,
	type ExplorerConfigureResult,
	type ExplorerData,
	type ExplorerLive,
	type ExplorerLiveQuantity,
	type ExplorerState,
	type ExplorerSteerResult,
	type ExplorerSubscribeParams,
	type ExplorerSubscribeResult,
	type ExplorerSurveyResult,
	type ExplorerSweepResult
} from './explorer';
import { ServiceClient } from './client.svelte';
import { createControlServer } from './mock/control';
import { mockSocketFactory } from './mock/socket';
import { AcquireState } from '$lib/acquire/store.svelte';
import { EXPLORER_CHANNEL_QUANTITIES, EXPLORER_PAGES, EXPLORER_QUANTITIES, explorerPagePlots, explorerSubscribeParams } from '$lib/acquire/explorer/explorerLayout';
import { defaultExplorerSetup, explorerByKey, explorerSectionOf, fromExplorerConfig, toExplorerConfig } from '$lib/acquire/explorer/explorerSetup';
import { ExplorerStore } from '$lib/acquire/explorer/explorerStore.svelte';
import { explorerLevelStatus } from '$lib/acquire/explorer/explorerWords';

const DIR = fileURLToPath(new URL('../../../../schema/golden/', import.meta.url));
const golden = (name: string) => `${DIR}${name}`;
const has = (name: string) => existsSync(golden(name));
const readJson = (name: string) => JSON.parse(readFileSync(golden(name), 'utf-8'));
const readBin = (name: string) => new Uint8Array(readFileSync(golden(name)));

/** Every Explorer golden in the folder: its text frames and its binary frames. */
const EXPLORER = readdirSync(DIR).filter((f) => /(^|_)explorer(_|\.)/.test(f));
const TEXT = EXPLORER.filter((f) => f.endsWith('.json') && !f.endsWith('.expected.json')).sort();
const BIN = EXPLORER.filter((f) => f.endsWith('.bin')).sort();
const QUANTITIES: readonly ExplorerLiveQuantity[] = EXPLORER_QUANTITIES;
/** The Run reading's rows, as PROTOCOL names them. */
const RUN_ROWS = ['Frequency', 'Target', 'Measured', 'Drive', 'Target drive', 'Ceiling', 'Sweep', 'Control rms', 'Velocity', 'Displacement', 'Peak', 'Pipeline'];

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

/** A plot's channels and point count, each only where the quantity takes it. */
function keysAsTheServiceTakes(plots: { quantity: ExplorerLiveQuantity; channels?: unknown; points?: number | null }[], sent: boolean) {
	for (const p of plots) {
		expect('channels' in p && p.channels !== undefined).toBe(EXPLORER_CHANNEL_QUANTITIES.includes(p.quantity));
		const takesPoints = p.quantity === 'time' || p.quantity === 'survey';
		if (!takesPoints) expect('points' in p && p.points !== undefined).toBe(false);
		// the shell sends a point count with the time history only; the answer fills the survey's in
		if (sent && p.quantity === 'survey') expect(p.points ?? null).toBeNull();
	}
}

/** The form of a view the shell compares with the service's: titles, axes, series names and roles. */
function shapeOf(v: View): unknown {
	if (v.view === 'spectrum') return { view: v.view, title: v.title, x: v.x, y: v.y, series: v.series.map((s) => [s.name, s.role]), link: v.link ?? null };
	if (v.view === 'reading') return { view: v.view, title: v.title, rows: (v as ReadingView).rows.map((r) => r.label) };
	return { view: v.view };
}

const testConfig = () => (has('request_explorer_configure.json') ? (readJson('request_explorer_configure.json') as Request<ExplorerConfigureParams>).params.config : null);

describe('Explorer goldens in the folder', () => {
	it('are Explorer text frames or binary frames with their expected form', () => {
		for (const f of BIN) expect(EXPLORER).toContain(f.replace(/\.bin$/, '.expected.json'));
		for (const f of EXPLORER) expect(f).toMatch(/\.(json|bin)$/);
	});

	for (const name of TEXT) {
		it(`${name} decodes and re-encodes as the same compact Explorer text frame`, () => {
			const text = readFileSync(golden(name), 'utf-8');
			const message = decodeFrame(text);
			expect(message).toEqual(JSON.parse(text));
			expect(encodeFrame(message)).toBe(JSON.stringify(JSON.parse(text)));
		});
	}

	for (const name of BIN) {
		it(`${name} is an Explorer binary frame that decodes to its expected form`, () => {
			const bin = readBin(name);
			expect(new TextDecoder().decode(bin.slice(0, 4))).toBe('SDS1');
			const expected = toTyped(readJson(name.replace(/\.bin$/, '.expected.json'))) as Record<string, unknown>;
			expect(decodeFrame(bin)).toEqual(expected);
		});
	}
});

describe('Explorer goldens through the shell', () => {
	it.skipIf(!has('request_explorer_configure.json'))('an Explorer configuration request survives the setup draft and back, every field', () => {
		const config = testConfig()!;
		expect(toExplorerConfig(fromExplorerConfig(config))).toEqual(config);
	});

	it.skipIf(!has('request_explorer_configure.json'))("the shell's own default Explorer setup sends the same fields the service's example has", () => {
		const config = testConfig()!;
		const mine = toExplorerConfig({ ...defaultExplorerSetup(), max_drive_vpk: 0.5 });
		expect(Object.keys(mine).sort()).toEqual(Object.keys(config).sort());
		for (const k of ['acquisition', 'slew', 'display', 'capture'] as const) expect(Object.keys(mine[k] ?? {}).sort()).toEqual(Object.keys(config[k] ?? {}).sort());
	});

	it.skipIf(!has('response_explorer_configure.json'))('the Explorer configure result is a configured state with what the service derived', () => {
		const res = (readJson('response_explorer_configure.json') as ResponseOk<ExplorerConfigureResult>).result;
		expect([res.state.state, res.state.phase]).toEqual(['configured', 'idle']);
		expect(explorerLevelStatus(res.state).text).toBe('Set up, ready to start');
		const d = res.derived;
		expect(Object.keys(d).sort()).toEqual(['band_hz', 'display', 'grid', 'pipeline_frames', 'pipeline_s', 'rise_s', 'stop_s', 'stop_wait_s', 'survey', 'sweep_max_oct_per_min']);
		expect(d.grid.actual).toBe(false);
		expect(d.sweep_max_oct_per_min).toBeCloseTo(60 * (testConfig()?.slew?.freq_oct_per_s ?? 1), 9);
		expect(d.survey.first_point_s).toHaveLength(2);
	});

	it.skipIf(!has('response_error_explorer_config_invalid.json'))("an invalid Explorer configuration's findings land on the fields and sections they name", () => {
		const err = readJson('response_error_explorer_config_invalid.json') as ResponseErr;
		expect([err.error.code, err.error.reason]).toEqual(['bad_request', 'config_invalid']);
		const items = (err.error.detail as { errors: { reason: string; field?: string | null; message: string }[] }).errors;
		const keys = Object.keys(explorerByKey(items));
		for (const k of keys) expect(k).not.toMatch(/^config\.|\[/);
		const reasons = ['out_of_range', 'band_above_grid', 'ceiling_over_range', 'safe_stop_budget', 'no_drive_channel', 'extra_drive_channels', 'no_input_channel'];
		for (const it of items) {
			expect(reasons).toContain(it.reason);
			const section = explorerSectionOf(Object.keys(explorerByKey([it]))[0]);
			expect(section).toBe(['no_drive_channel', 'extra_drive_channels', 'no_input_channel'].includes(it.reason) ? 'station' : it.field?.startsWith('capture') ? 'capture' : 'test');
		}
	});

	it.skipIf(!has('response_explorer_start.json'))('an Explorer start answers once the tone runs at 0 V', () => {
		const s = (readJson('response_explorer_start.json') as ResponseOk<ExplorerState>).result;
		expect([s.state, s.phase, s.drive_vpk, s.target_vpk]).toEqual(['running', 'quiet', 0, 0]);
		expect(s.grid?.actual).toBe(true);
		expect(explorerLevelStatus(s).text).toBe('Quiet: the tone is at 0 V');
	});

	it.skipIf(!has('request_explorer_steer.json'))('an Explorer steer has at most one part of each kind', () => {
		const p = (readJson('request_explorer_steer.json') as Request<Record<string, number>>).params;
		const freq = ['hz', 'step_hz', 'step_oct'].filter((k) => k in p);
		const level = ['vpk', 'step_db'].filter((k) => k in p);
		expect(freq.length).toBeLessThanOrEqual(1);
		expect(level.length).toBeLessThanOrEqual(1);
		expect(freq.length + level.length).toBeGreaterThan(0);
		expect(Object.keys(p).length).toBe(freq.length + level.length);
	});

	it.skipIf(!has('response_explorer_steer.json'))('an Explorer steer is answered with the targets adopted, the clamps, and both frames', () => {
		const r = (readJson('response_explorer_steer.json') as ResponseOk<ExplorerSteerResult>).result;
		expect(Object.keys(r).sort()).toEqual(['clamped', 'frame', 'hz', 'input_frame', 'vpk']);
		expect(Object.keys(r.clamped).sort()).toEqual(['hz', 'vpk']);
		expect(r.frame).toBeGreaterThanOrEqual(r.input_frame);
	});

	it.skipIf(!has('response_explorer_sweep.json'))('an Explorer sweep is answered with the rate used and both frames', () => {
		const r = (readJson('response_explorer_sweep.json') as ResponseOk<ExplorerSweepResult>).result;
		expect(Object.keys(r).sort()).toEqual(['clamped', 'direction', 'frame', 'input_frame', 'rate_oct_per_min']);
		expect(['up', 'down']).toContain(r.direction);
		if (has('request_explorer_sweep.json')) {
			const p = (readJson('request_explorer_sweep.json') as Request<{ rate_oct_per_min: number }>).params;
			expect(r.clamped).toBe(r.rate_oct_per_min < p.rate_oct_per_min);
		}
	});

	it.skipIf(!has('event_explorer_state.json'))('the Explorer state event reads in the level card words', () => {
		const ev = readJson('event_explorer_state.json') as Event<ExplorerState>;
		expect(ev.topic).toBe(EXPLORER_TOPICS.state);
		const s = ev.data;
		const words = explorerLevelStatus(s).text;
		if (s.state === 'running' && s.phase === 'steady') expect(words).toMatch(/^Holding \d+\.\d Hz at \d\.\d{3} V pk$/);
		if (s.state === 'running' && s.phase === 'sweeping') expect(words).toMatch(/^Sweeping (up|down) at [\d.]+ oct\/min$/);
		expect(Object.keys(s.health ?? {}).sort()).toEqual(s.health ? ['batch_gap_ms', 'blocks_dropped', 'budget_ms', 'driver_warnings', 'engine_ticks', 'health_faults', 'pass_ms', 'ring_overruns', 'worst_tick_ms'] : []);
	});

	it.skipIf(!has('event_explorer_live.bin'))('the Explorer live frame carries the Survey and Run views and the numbers the level card reads', () => {
		const ev = decodeFrame(readBin('event_explorer_live.bin')) as unknown as Event<ExplorerLive>;
		expect(ev.topic).toBe(EXPLORER_TOPICS.live);
		const l = ev.data;
		expect(l.views.map((v) => [v.view, v.title])).toEqual([
			['spectrum', 'Survey'],
			['reading', 'Run']
		]);
		expect((l.views[1] as ReadingView).rows.map((r) => r.label)).toEqual(RUN_ROWS);
		expect(l.views[0].x).toMatchObject({ label: 'Frequency', unit: 'Hz', scale: 'log' });
		for (const k of ['frequency_hz', 'drive_vpk', 'max_drive_vpk', 'channels', 'control', 'peak', 'pipeline_frames', 'pass_ms', 'measured_hz']) expect(k in l).toBe(true);
	});

	it.skipIf(!has('event_explorer_data.bin'))("an Explorer data frame's plots are quantities the shell draws, channels and first frames only where they apply", () => {
		const ev = decodeFrame(readBin('event_explorer_data.bin')) as unknown as Event<ExplorerData>;
		expect(ev.topic).toBe(EXPLORER_TOPICS.data);
		expect('views' in ev.data).toBe(false);
		for (const p of ev.data.plots) {
			expect(QUANTITIES).toContain(p.quantity);
			expect(p.channels === null).toBe(p.quantity !== 'spectrum');
			expect(p.first_frame === null).toBe(p.quantity !== 'time' || p.views.length === 0);
			for (const v of p.views) expect(['spectrum', 'reading']).toContain(v.view);
		}
	});

	it.skipIf(!has('request_explorer_subscribe.json'))('an Explorer subscription asks only for quantities the shell knows, each key only where it applies', () => {
		const p = (readJson('request_explorer_subscribe.json') as Request<ExplorerSubscribeParams>).params;
		if ('plots' in p) {
			for (const plot of p.plots) expect(QUANTITIES).toContain(plot.quantity);
			keysAsTheServiceTakes(p.plots, false);
		}
	});

	it('every Explorer page the shell offers subscribes with each key only where it applies', () => {
		for (const kind of EXPLORER_PAGES.kinds) {
			const sent = explorerSubscribeParams(explorerPagePlots({ id: 'p', name: kind, kind }), 10) as { plots: { quantity: ExplorerLiveQuantity; channels?: unknown; points?: number }[] };
			keysAsTheServiceTakes(sent.plots, true);
		}
	});

	it.skipIf(!has('response_explorer_subscribe.json'))('the Explorer subscription is answered with every default filled in', () => {
		const r = (readJson('response_explorer_subscribe.json') as ResponseOk<ExplorerSubscribeResult>).result;
		expect([...r.quantities].sort()).toEqual([...QUANTITIES].sort());
		for (const p of r.plots ?? []) {
			if (p.quantity === 'spectrum') expect(p.channels).toBeDefined();
			if (p.quantity === 'time') expect(typeof p.points).toBe('number');
			if (p.quantity === 'survey') expect('points' in p).toBe(true);
		}
	});

	it.skipIf(!has('response_explorer_survey.json'))('the Explorer survey answers its points sorted by frequency and its highest point', () => {
		const r = (readJson('response_explorer_survey.json') as ResponseOk<ExplorerSurveyResult>).result;
		expect(Object.keys(r).sort()).toEqual(['channel', 'peak', 'points', 'revision', 'unit']);
		if (r.points) {
			const f = r.points.map((p) => p[0]);
			expect(f).toEqual([...f].sort((a, b) => a - b));
			if (r.points.length) expect(r.peak?.level).toBe(Math.max(...r.points.map((p) => p[1])));
		}
	});
});

describe("the mock control service's Explorer frames have the service's form", () => {
	const until = async (cond: () => boolean, ms = 8000) => {
		const t0 = Date.now();
		while (!cond()) {
			if (Date.now() - t0 > ms) throw new Error('timed out waiting');
			await new Promise((r) => setTimeout(r, 5));
		}
	};

	it.skipIf(!has('event_explorer_data.bin') || !has('event_explorer_live.bin') || !has('request_explorer_subscribe.json'))('the Explorer live views and every quantity of a data frame', async () => {
		const server = createControlServer({ speed: 20 });
		const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(server, 1) });
		const acq = new AcquireState(client, { session: null, local: null, checkDelayMs: 0 });
		client.connect();
		await client.whenOpen();
		await acq.sync();
		await acq.claim();
		const explorer = new ExplorerStore(acq, { local: null, checkDelayMs: 0 });
		await explorer.sync();
		const config = testConfig();
		explorer.setup = config ? fromExplorerConfig(config) : { ...explorer.setup, max_drive_vpk: 0.5 };
		expect(await explorer.apply()).toBe(true);
		const want = (readJson('request_explorer_subscribe.json') as Request<ExplorerSubscribeParams>).params as { rate_hz: number; plots: unknown[] };
		let data: ExplorerData | null = null;
		client.on(EXPLORER_TOPICS.data, (d) => (data = d as unknown as ExplorerData));
		const observer = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(server, 1) });
		observer.connect();
		await observer.whenOpen();
		let observedLive: ExplorerLive | null = null;
		observer.on(EXPLORER_TOPICS.live, (d) => (observedLive = d as unknown as ExplorerLive));
		await client.request('explorer.subscribe', { rate_hz: want.rate_hz, plots: want.plots });
		expect(await explorer.start()).toBe(true);
		await explorer.setVpk(Math.min(0.2, explorer.setup.max_drive_vpk));
		await until(() => !!data && data.plots.every((p) => p.views.length > 0) && !!observedLive && (observedLive.views[0].series?.length ?? 0) > 0, 10000);
		const theirs = (decodeFrame(readBin('event_explorer_data.bin')) as unknown as Event<ExplorerData>).data;
		const mine = data as unknown as ExplorerData;
		expect(Object.keys(mine).sort()).toEqual(Object.keys(theirs).sort());
		for (const p of theirs.plots) {
			const m = mine.plots.find((x) => x.id === p.id)!;
			expect(m.quantity).toBe(p.quantity);
			if (p.views.length) expect(m.views.map(shapeOf)).toEqual(p.views.map(shapeOf));
		}
		const live = (decodeFrame(readBin('event_explorer_live.bin')) as unknown as Event<ExplorerLive>).data;
		expect(Object.keys(observedLive as unknown as ExplorerLive).sort()).toEqual(Object.keys(live).sort());
		expect((observedLive as ExplorerLive | null)?.views.map(shapeOf)).toEqual(live.views.map(shapeOf));
		await explorer.abort();
	}, 20000);
});
