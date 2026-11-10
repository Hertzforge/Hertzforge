/**
 * The Sine goldens (schema/golden/) through the shell's codec, types, setup rules and words. The
 * files are read from the folder itself, so this suite, the Rust suite and the Python suite all
 * read the same frames: a golden added there is checked here without a list to update. Each check
 * that names a golden runs when that golden is in the folder. The last part holds the mock control
 * service's frames to the service's form.
 */
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { decodeFrame, encodeFrame, withShape } from './codec';
import type { Dtype, Event, ReadingView, Request, ResponseErr, ResponseOk, SpectrumView, View } from './types';
import type { LawsListResult } from './mimo';
import {
	SINE_TOPICS,
	type SineAdjustParams,
	type SineCheckRequirement,
	type SineConfigureParams,
	type SineConfigureResult,
	type SineData,
	type SineLawCheckCase,
	type SineLawsCheckResult,
	type SineLevelParams,
	type SineLive,
	type SineLiveQuantity,
	type SineProfileImportResult,
	type SineProfileValidateResult,
	type SineState,
	type SineSubscribeParams,
	type SineSubscribeResult
} from './sine';
import { ServiceClient } from './client.svelte';
import { createControlServer } from './mock/control';
import { mockSocketFactory } from './mock/socket';
import { lawsOfWorld } from '$lib/acquire/law/lawForm';
import { AcquireState } from '$lib/acquire/store.svelte';
import { defaultPulseScreen, pulseSubscription } from '$lib/acquire/pulse/pulseScreen';
import type { PlotPanel } from '$lib/acquire/screen/screen.svelte';
import { SINE_CHANNEL_QUANTITIES, SINE_PLOT_TYPES, SINE_QUANTITIES, SINE_SCREEN, SINE_TIME_QUANTITIES } from '$lib/acquire/sine/sineLayout';
import { fromSineConfig, profileDraftOf, profileOf, sineByKey, sineSectionOf, toSineConfig, defaultSineSetup } from '$lib/acquire/sine/sineSetup';
import { SineStore } from '$lib/acquire/sine/sineStore.svelte';
import { errorWords, progressOf, sineLevelStatus, sweepWords } from '$lib/acquire/sine/sineWords';

const DIR = fileURLToPath(new URL('../../../../schema/golden/', import.meta.url));
const golden = (name: string) => `${DIR}${name}`;
const has = (name: string) => existsSync(golden(name));
const readJson = (name: string) => JSON.parse(readFileSync(golden(name), 'utf-8'));
const readBin = (name: string) => new Uint8Array(readFileSync(golden(name)));

/** Every Sine golden in the folder: its text frames and its binary frames. */
const SINE = readdirSync(DIR).filter((f) => /(^|_)sine(_|\.)/.test(f));
const TEXT = SINE.filter((f) => f.endsWith('.json') && !f.endsWith('.expected.json')).sort();
const BIN = SINE.filter((f) => f.endsWith('.bin')).sort();
const QUANTITIES: readonly SineLiveQuantity[] = SINE_QUANTITIES;
const REQUIREMENTS: SineCheckRequirement[] = ['within', 'through', 'within_away', 'survive', 'each_rung_within', 'limit_held', 'settles'];

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

/** A plot's channels and point count: each only where the quantity takes it, as the service has it. */
function keysAsTheServiceTakes(plots: { quantity: SineLiveQuantity; channels?: unknown; points?: number }[]) {
	for (const p of plots) {
		expect('channels' in p && p.channels !== undefined).toBe(SINE_CHANNEL_QUANTITIES.includes(p.quantity));
		expect('points' in p && p.points !== undefined).toBe(SINE_TIME_QUANTITIES.includes(p.quantity));
	}
}

/** The form of a view the shell compares with the service's: titles, axes, series names and roles. */
function shapeOf(v: View): unknown {
	if (v.view === 'spectrum') return { view: v.view, title: v.title, x: v.x, y: v.y, series: v.series.map((s) => [s.name, s.role]), link: v.link ?? null };
	if (v.view === 'reading') return { view: v.view, title: v.title, rows: (v as ReadingView).rows.map((r) => r.label) };
	return { view: v.view };
}

const testConfig = () => (has('request_sine_configure.json') ? (readJson('request_sine_configure.json') as Request<SineConfigureParams>).params.config : null);

describe('Sine goldens in the folder', () => {
	it('are text frames or binary frames with their expected form', () => {
		for (const f of BIN) expect(SINE).toContain(f.replace(/\.bin$/, '.expected.json'));
		for (const f of SINE) expect(f).toMatch(/\.(json|bin)$/);
	});

	for (const name of TEXT) {
		it(`${name} decodes and re-encodes as the same compact text frame`, () => {
			const text = readFileSync(golden(name), 'utf-8');
			const message = decodeFrame(text);
			expect(message).toEqual(JSON.parse(text));
			expect(encodeFrame(message)).toBe(JSON.stringify(JSON.parse(text)));
		});
	}

	// The service writes these frames; the shell reads them. The values are what must agree, not
	// the bytes (the service may write a binary frame with no arrays, or 412.0 for a whole number).
	for (const name of BIN) {
		it(`${name} is a binary frame that decodes to its expected form`, () => {
			const bin = readBin(name);
			expect(new TextDecoder().decode(bin.slice(0, 4))).toBe('SDS1');
			const expected = toTyped(readJson(name.replace(/\.bin$/, '.expected.json'))) as Record<string, unknown>;
			expect(decodeFrame(bin)).toEqual(expected);
		});
	}
});

describe('Sine goldens through the shell', () => {
	it.skipIf(!has('request_sine_configure.json'))('a configuration request survives the setup draft and back, every field', () => {
		const config = testConfig()!;
		const law = has('response_laws_list_sine.json')
			? (lawsOfWorld((readJson('response_laws_list_sine.json') as ResponseOk<LawsListResult>).result.laws, 'sine').find((l) => l.id === config.control?.law?.id) ?? null)
			: null;
		const back = toSineConfig(fromSineConfig(config), law);
		if (law) expect(back).toEqual(config);
		else expect({ ...back, control: { ...back.control, law: config.control?.law } }).toEqual(config);
	});

	it.skipIf(!has('request_sine_configure.json'))("the shell's own default setup asks for the same kinds of fields the service's example has", () => {
		const config = testConfig()!;
		const mine = toSineConfig({ ...defaultSineSetup(), limits: { ...defaultSineSetup().limits, drive_vpk_ceiling: 0.5, drive_vpk_abort: 0.8 } }, null);
		expect(Object.keys(mine).sort()).toEqual(Object.keys(config).sort());
		for (const k of ['test', 'acquisition', 'schedule', 'identification', 'control', 'limits', 'cola', 'capture'] as const) expect(Object.keys(mine[k] ?? {}).sort()).toEqual(Object.keys(config[k] ?? {}).sort());
	});

	it.skipIf(!has('response_laws_list_sine.json'))('the Sine world lists its built-in law first, with a form the shell draws and its live parameters', () => {
		const list = (readJson('response_laws_list_sine.json') as ResponseOk<LawsListResult>).result;
		const laws = lawsOfWorld(list.laws, 'sine');
		expect(laws.length).toBe(list.laws.length);
		expect([laws[0].id, laws[0].builtin, laws[0].check.hardware]).toEqual(['servo_blend', true, true]);
		for (const l of laws) {
			for (const p of l.params) expect(['number', 'integer', 'toggle', 'choice']).toContain(p.kind);
			for (const d of l.diagnostics) expect(d.layout).toBeNull();
			expect(Array.isArray(l.live)).toBe(true);
			// a live parameter is one of the law's number parameters
			for (const k of l.live ?? []) expect(l.params.find((p) => p.key === k)?.kind).toMatch(/^(number|integer)$/);
		}
		expect(laws[0].live).toEqual(['blend_w']);
	});

	it.skipIf(!has('response_sine_configure.json'))('the configure result is a configured state with what the service derived', () => {
		const res = (readJson('response_sine_configure.json') as ResponseOk<SineConfigureResult>).result;
		expect([res.state.state, res.state.phase]).toEqual(['configured', 'idle']);
		expect(sineLevelStatus(res.state, null).text).toBe('Set up, ready to arm');
		expect(res.derived.stop_ramp_db_per_s).toBe(120);
		expect(res.derived.grid.actual).toBe(false);
		expect(res.derived.law.live).toEqual(['blend_w']);
		expect(['ok', 'advisory', 'refused']).toContain(res.derived.tracking.verdict);
	});

	it.skipIf(!has('response_error_sine_config_invalid.json'))("an invalid configuration's findings land on the fields and sections they name", () => {
		const err = readJson('response_error_sine_config_invalid.json') as ResponseErr;
		expect([err.error.code, err.error.reason]).toEqual(['bad_request', 'config_invalid']);
		const items = (err.error.detail as { errors: { reason: string; field?: string | null; message: string }[] }).errors;
		const keys = Object.keys(sineByKey(items));
		expect(keys.length).toBe(items.length);
		for (const k of keys) expect(k).not.toMatch(/^config\.|\[/);
		const sections = Object.fromEntries(items.map((it) => [it.reason, sineSectionOf(Object.keys(sineByKey([it]))[0])]));
		expect(sections).toMatchObject({ tick_out_of_range: 'control', abort_below_ceiling: 'limits', limit_on_control_channel: 'limits' });
		expect(Object.keys(sineByKey(items.filter((it) => it.reason === 'limit_on_control_channel')))).toEqual(['limits.channels.0.channel']);
	});

	it.skipIf(!has('event_sine_state.json'))('the state event reads in the level card words', () => {
		const ev = readJson('event_sine_state.json') as Event<SineState>;
		expect(ev.topic).toBe(SINE_TOPICS.state);
		const s = ev.data;
		const words = sineLevelStatus(s, null, testConfig()).text;
		if (s.state === 'running' && s.test === 'sweep' && !s.held && !s.notch) expect(words).toMatch(new RegExp(`^Sweeping ${s.direction} at [\\d.]+ oct/min at [-+]?\\d+\\.\\d dB$`));
		if (s.sweeps) expect(sweepWords(s)).toBe(`Sweep ${s.sweeps.done + 1}${s.sweeps.of !== null ? ` of ${s.sweeps.of}` : ''}, ${s.direction}`);
		const p = progressOf(s);
		if (s.elapsed_s !== null && s.remaining_s !== null) expect(p).toBeCloseTo(s.elapsed_s / (s.elapsed_s + s.remaining_s), 9);
	});

	it.skipIf(!has('event_sine_live.bin'))('the live frame carries the numbers the level card reads, and one sweep rate', () => {
		const ev = decodeFrame(readBin('event_sine_live.bin')) as unknown as Event<SineLive>;
		expect(ev.topic).toBe(SINE_TOPICS.live);
		const l = ev.data;
		expect(['sweep_rate_oct_per_min', 'sweep_rate_hz_per_s'].filter((k) => k in l)).toHaveLength(1);
		expect(l.views.map((v) => v.title)).toEqual(['Control', 'Drive', 'Run']);
		expect(l.views.map((v) => v.view)).toEqual(['spectrum', 'spectrum', 'reading']);
		const state = has('event_sine_state.json') ? (readJson('event_sine_state.json') as Event<SineState>).data : null;
		if (state && state.run_id === l.run_id) {
			const words = sineLevelStatus(state, l, testConfig()).text;
			expect(words).toBe(`Sweeping ${l.direction} at ${Math.abs(l.sweep_rate_oct_per_min ?? 0)} oct/min at ${l.level_db! >= 0 ? '' : '-'}${Math.abs(l.level_db!).toFixed(1)} dB`);
		}
		if (l.measured !== null && l.target !== null) expect(errorWords(l, l.channels.find((c) => c.role === 'control')?.unit)).toMatch(/^[\d.]+ \S+ against [\d.]+ \S+(, [+-][\d.]+ dB)?$/);
		for (const v of l.views) if (v.view === 'spectrum') for (const s of (v as SpectrumView).series) expect([...s.y].every(Number.isFinite)).toBe(true);
	});

	it.skipIf(!has('event_sine_data.bin'))("a data frame's plots are quantities the shell draws, channels resolved only where they apply", () => {
		const ev = decodeFrame(readBin('event_sine_data.bin')) as unknown as Event<SineData>;
		expect(ev.topic).toBe(SINE_TOPICS.data);
		for (const p of ev.data.plots) {
			expect(QUANTITIES).toContain(p.quantity);
			expect(p.channels === null).toBe(!SINE_CHANNEL_QUANTITIES.includes(p.quantity));
			for (const v of p.views) expect(['spectrum', 'reading']).toContain(v.view);
		}
	});

	it.skipIf(!has('request_sine_subscribe.json'))('a subscription asks only for quantities the shell knows, channels and a point count only where they apply', () => {
		const p = (readJson('request_sine_subscribe.json') as Request<SineSubscribeParams>).params;
		if ('plots' in p) {
			for (const plot of p.plots) expect(QUANTITIES).toContain(plot.quantity);
			keysAsTheServiceTakes(p.plots);
		}
		// and so does every plot type the shell offers, on its first pages and added on its own
		const first = defaultPulseScreen(SINE_SCREEN, SINE_PLOT_TYPES).pages.flatMap((pg) => pg.panels.filter((x): x is PlotPanel => x.kind === 'plot'));
		const one = SINE_PLOT_TYPES.map((t, i): PlotPanel => ({ ...first[0], id: `pl-${i}`, plot: { ...first[0].plot, quantity: t.id } }));
		for (const plots of [first, one]) {
			const sent = pulseSubscription(SINE_PLOT_TYPES, plots, 5) as { plots: { quantity: SineLiveQuantity }[] };
			keysAsTheServiceTakes(sent.plots);
		}
		expect(SINE_PLOT_TYPES.map((t) => t.id)).toEqual([...SINE_QUANTITIES]);
	});

	it.skipIf(!has('response_sine_subscribe.json'))('the subscription is answered with every default filled in', () => {
		const r = (readJson('response_sine_subscribe.json') as ResponseOk<SineSubscribeResult>).result;
		expect([...r.quantities].sort()).toEqual([...QUANTITIES].sort());
		if (r.plots) keysAsTheServiceTakes(r.plots);
	});

	it.skipIf(!has('request_sine_level.json'))('a level change names a step or a level of the schedule', () => {
		const p = (readJson('request_sine_level.json') as Request<SineLevelParams>).params;
		expect(Object.keys(p)).toHaveLength(1);
		expect(typeof ('delta_db' in p ? p.delta_db : p.to_db)).toBe('number');
	});

	it.skipIf(!has('request_sine_adjust.json'))('an adjust names a live parameter by key and a number', () => {
		const p = (readJson('request_sine_adjust.json') as Request<SineAdjustParams>).params;
		expect([typeof p.key, typeof p.value]).toEqual(['string', 'number']);
	});

	it.skipIf(!has('response_sine_profile_import.json'))('a profile read from rows is one the setup takes, rows and bands as they came', () => {
		const r = (readJson('response_sine_profile_import.json') as ResponseOk<SineProfileImportResult>).result;
		if (!r.profile) {
			expect(r.errors.length).toBeGreaterThan(0);
			return;
		}
		const back = profileOf(profileDraftOf(r.profile, defaultSineSetup().profile));
		expect(back.rows).toEqual(r.profile.rows);
		expect([back.alarm_db, back.abort_db]).toEqual([r.profile.alarm_db, r.profile.abort_db]);
	});

	it.skipIf(!has('response_sine_profile_validate.bin'))('the profile check draws the profile, its velocity and its displacement, with its kinematics', () => {
		const res = decodeFrame(readBin('response_sine_profile_validate.bin')) as unknown as ResponseOk<SineProfileValidateResult>;
		const r = res.result;
		expect(r.views.map((v) => v.title)).toEqual(['Profile', 'Velocity', 'Displacement']);
		expect((r.views[0] as SpectrumView).series.map((s) => s.name)).toEqual(['Profile', 'Alarm high', 'Alarm low', 'Abort high', 'Abort low']);
		if (r.kinematics) expect(['pass', 'advisory', 'exceeded', 'not_evaluated']).toContain(r.kinematics.verdict);
	});

	it.skipIf(!has('response_laws_check_sine.json'))("a law check's cases name a requirement the shell knows, with every field the shell reads", () => {
		const r = (readJson('response_laws_check_sine.json') as ResponseOk<SineLawsCheckResult>).result;
		expect(r.law.world).toBe('sine');
		const fields: (keyof SineLawCheckCase)[] = ['name', 'plant', 'requirement', 'passed', 'detail', 'end', 'trip', 'ticks', 'judged_ticks', 'worst_error_db', 'worst_error_hz', 'worst_away_error_db', 'rungs', 'worst_limit_over_db', 'worst_release_overshoot_db', 'calls', 'misses', 'refused', 'capped_ticks', 'worst_call_ms', 'budget_ms', 'max_drive_vpk'];
		for (const c of r.report.cases) {
			expect(REQUIREMENTS).toContain(c.requirement);
			expect(Object.keys(c).sort()).toEqual([...fields].sort());
		}
	});
});

describe("the mock control service's Sine frames have the service's form", () => {
	const until = async (cond: () => boolean, ms = 8000) => {
		const t0 = Date.now();
		while (!cond()) {
			if (Date.now() - t0 > ms) throw new Error('timed out waiting');
			await new Promise((r) => setTimeout(r, 5));
		}
	};

	it.skipIf(!has('event_sine_data.bin') || !has('response_laws_list_sine.json'))('the built-in law, the live views and every quantity of a data frame', async () => {
		const server = createControlServer({ speed: 20 });
		const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(server, 1) });
		const acq = new AcquireState(client, { session: null, local: null, checkDelayMs: 0 });
		client.connect();
		await client.whenOpen();
		await acq.sync();
		await acq.claim();
		// the golden's table: a control row, and two monitors; channel 3 a limit channel
		const rows = acq.rows.map((r) => ({ ...r }));
		rows.push({ ...rows[0], index: 3, description: 'Fixture', role: 'monitor', physical: null });
		acq.rows = rows;
		expect(await acq.applyTable()).toBe(true);
		const sine = new SineStore(acq, { local: null, checkDelayMs: 0 });
		await sine.sync();
		const service = (readJson('response_laws_list_sine.json') as ResponseOk<LawsListResult>).result.laws[0];
		expect(sine.law).toEqual(service);
		sine.setup.limits.drive_vpk_ceiling = 0.5;
		sine.setup.limits.drive_vpk_abort = 0.8;
		sine.setup.identification.review = false;
		sine.setup.limits.channels = [{ channel: 3, shared_name: '', profile: { label: '', rows: [{ freq_hz: 10, quantity: 'accel_g', value: 30, own_bands: false, alarm_db: [1.5, 1.5], abort_db: [3, 3] }, { freq_hz: 3000, quantity: 'accel_g', value: 30, own_bands: false, alarm_db: [1.5, 1.5], abort_db: [3, 3] }], alarm_db: [1.5, 1.5], abort_db: [3, 3] } }];
		expect(await sine.apply()).toBe(true);
		const want = (readJson('request_sine_subscribe.json') as Request<SineSubscribeParams>).params as { rate_hz: number; plots: unknown[] };
		const plots = want.plots;
		let data: SineData | null = null;
		client.on(SINE_TOPICS.data, (d) => (data = d as unknown as SineData));
		const observer = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(server, 1) });
		observer.connect();
		await observer.whenOpen();
		let observedLive: SineLive | null = null;
		observer.on(SINE_TOPICS.live, (d) => (observedLive = d as unknown as SineLive));
		await client.request('sine.subscribe', { rate_hz: 5, plots });
		expect(await sine.start()).toBe(true);
		await until(() => !!data && data.phase === 'engaged' && data.plots.every((p) => p.views.length > 0) && (data.plots[0].views[0] as SpectrumView).series[0].x.length > 1 && !!observedLive && observedLive.phase === 'engaged');
		const theirs = (decodeFrame(readBin('event_sine_data.bin')) as unknown as Event<SineData>).data;
		const mine = data as unknown as SineData;
		expect(Object.keys(mine).sort()).toEqual(Object.keys(theirs).sort());
		for (const p of theirs.plots) {
			const m = mine.plots.find((x) => x.id === p.id)!;
			expect([p.quantity, m.views.length]).toEqual([m.quantity, p.views.length]);
			expect(m.views.map(shapeOf)).toEqual(p.views.map(shapeOf));
		}
		// A connection on the default subscription gets `sine.live`; this one subscribed to plots.
		const live = (decodeFrame(readBin('event_sine_live.bin')) as unknown as Event<SineLive>).data;
		expect((observedLive as SineLive | null)?.views.map(shapeOf)).toEqual(live.views.map(shapeOf));
		await sine.abort();
	}, 15000);
});
