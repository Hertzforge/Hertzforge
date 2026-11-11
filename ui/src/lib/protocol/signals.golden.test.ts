/**
 * The Signals goldens (schema/golden/) through the shell's codec and types, and the mock control
 * service's frames held to the service's form. The files are read from the folder itself, so this
 * suite, the Rust suite and the Python suite all read the same frames.
 */
import { readdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { decodeFrame, encodeFrame, withShape } from './codec';
import type { Dtype, Event, Request, ResponseOk, SpectrumView } from './types';
import { SIGNALS_METHODS, SIGNALS_TOPICS, type SignalsData, type SignalsSubscribeParams, type SignalsSubscribeResult } from './signals';
import { ServiceClient } from './client.svelte';
import { createControlServer } from './mock/control';
import { mockSocketFactory } from './mock/socket';

const DIR = fileURLToPath(new URL('../../../../schema/golden/', import.meta.url));
const golden = (name: string) => `${DIR}${name}`;
const readJson = (name: string) => JSON.parse(readFileSync(golden(name), 'utf-8'));
const readBin = (name: string) => new Uint8Array(readFileSync(golden(name)));

const SIGNALS = readdirSync(DIR).filter((f) => /(^|_)signals(_|\.)/.test(f));

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
 * A view's shape: its title, axes and each series' role and arrays, but `y_min`, which comes with
 * the run's rate (an envelope only where a window holds more samples than its points).
 */
const shapeOf = (v: SpectrumView) => ({
	title: v.title.replace(/\d+/g, '#'),
	x: v.x,
	y: { ...v.y, unit: typeof v.y.unit },
	series: v.series.map((s) => ({ role: s.role, keys: Object.keys(s).filter((k) => k !== 'y_min').sort() }))
});

const until = async (cond: () => boolean, ms = 8000) => {
	const t0 = Date.now();
	while (!cond()) {
		if (Date.now() - t0 > ms) throw new Error('timed out waiting');
		await new Promise((r) => setTimeout(r, 5));
	}
};

describe('the Signals goldens', () => {
	it('are all here: the subscription and its answer, the unsubscribe and its answer, a data frame', () => {
		expect(SIGNALS.sort()).toEqual([
			'event_signals_data.bin',
			'event_signals_data.expected.json',
			'request_signals_subscribe.json',
			'request_signals_unsubscribe.json',
			'response_signals_subscribe.json',
			'response_signals_unsubscribe.json'
		]);
	});

	it('decode to their expected JSON and encode back to the same bytes', () => {
		const bytes = readBin('event_signals_data.bin');
		const decoded = decodeFrame(bytes) as unknown as Event<SignalsData>;
		expect(decoded).toEqual(toTyped(readJson('event_signals_data.expected.json')));
		expect(new Uint8Array(encodeFrame(decoded as unknown as Record<string, unknown>) as ArrayBuffer)).toEqual(bytes);
		expect(decoded.topic).toBe(SIGNALS_TOPICS.data);
	});

	it('read as the types say: a station clock, the envelope beyond the points, units never overlaid', () => {
		const d = (decodeFrame(readBin('event_signals_data.bin')) as unknown as Event<SignalsData>).data;
		expect(d.t_s).toBe(d.frames / d.fs_hz);
		const [raw, env] = d.plots;
		const x = raw.views[0].series[0].x;
		expect(x.length).toBe(raw.window_s * d.fs_hz);
		expect(x[0]).toBe(raw.first_frame / d.fs_hz);
		expect(raw.views[0].series[0].y_min).toBeUndefined();
		expect(env.views.map((v) => [v.title, v.y.unit])).toEqual([
			['Channels 1, 3', 'g'],
			['Channel 2: Drive', 'V']
		]);
		for (const v of env.views) for (const s of v.series) for (let i = 0; i < s.y.length; i++) expect(s.y[i]).toBeGreaterThanOrEqual(s.y_min![i]);
		const req = readJson('request_signals_subscribe.json') as Request<SignalsSubscribeParams>;
		expect(req.method).toBe(SIGNALS_METHODS.subscribe);
		const res = (readJson('response_signals_subscribe.json') as ResponseOk<SignalsSubscribeResult>).result;
		expect(res.plots.map((p) => p.id)).toEqual(req.params.plots.map((p) => p.id));
		expect(res.channels).toEqual(d.channels);
		expect((readJson('request_signals_unsubscribe.json') as Request<Record<string, never>>).method).toBe(SIGNALS_METHODS.unsubscribe);
	});

	it("the mock's answer and frames take the service's form", async () => {
		const server = createControlServer({ speed: 20 });
		const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(server, 1) });
		client.connect();
		await client.whenOpen();
		// The goldens' table: rows 1 (Table), 2 (Drive) and 3 (Fixture).
		const table = (await client.request('channels.get', {})) as { revision: number; channels: Record<string, unknown>[] };
		const [control, drive] = table.channels;
		await client.request('session.claim', { name: 'Shell' });
		await client.request('channels.set', {
			revision: table.revision,
			channels: [{ ...control, description: 'Table' }, drive, { ...control, index: 3, description: 'Fixture', role: 'monitor' }]
		});
		let data: SignalsData | null = null;
		client.on(SIGNALS_TOPICS.data, (d) => (data = d as unknown as SignalsData));
		const req = readJson('request_signals_subscribe.json') as Request<SignalsSubscribeParams>;
		const answer = (await client.request(SIGNALS_METHODS.subscribe, req.params as unknown as Record<string, unknown>)) as SignalsSubscribeResult;
		const theirs = (readJson('response_signals_subscribe.json') as ResponseOk<SignalsSubscribeResult>).result;
		expect(Object.keys(answer)).toEqual(Object.keys(theirs));
		expect(answer.plots).toEqual(theirs.plots);
		await client.request('recorder.configure', { config: {} });
		await client.request('recorder.start', {});
		await until(() => data !== null && (data as SignalsData).plots.every((p) => p.views.every((v) => v.series.length > 0)));
		const mine = data as unknown as SignalsData;
		const golden = (decodeFrame(readBin('event_signals_data.bin')) as unknown as Event<SignalsData>).data;
		expect(Object.keys(mine)).toEqual(Object.keys(golden));
		expect(mine.channels).toEqual(golden.channels);
		for (const p of golden.plots) {
			const m = mine.plots.find((x) => x.id === p.id)!;
			expect(Object.keys(m)).toEqual(Object.keys(p));
			expect(m.channels).toEqual(p.channels);
			expect(m.window_s).toBe(p.window_s);
			expect(m.views.map(shapeOf)).toEqual(p.views.map(shapeOf));
			const points = req.params.plots.find((q) => q.id === p.id)!.points!;
			for (const v of m.views) for (const s of v.series) expect(s.y_min !== undefined).toBe(Math.round(m.window_s * mine.fs_hz) > points);
		}
		await client.request('recorder.abort', {});
	}, 20000);
});
