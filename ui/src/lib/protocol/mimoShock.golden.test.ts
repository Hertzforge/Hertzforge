/**
 * The MIMO Shock goldens (schema/golden/) through the shell's codec, types and setup rules. The
 * files are read from the folder itself, so a golden added there is checked here without a list
 * to update. Each check that names a golden runs when that golden is in the folder.
 */
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { decodeFrame, encodeFrame, withShape } from './codec';
import type { Dtype, Event, Request, ResponseErr, ResponseOk, SpectrumView, TableView } from './types';
import {
	MIMO_SHOCK_TOPICS,
	type MimoShockConfigureParams,
	type MimoShockConfigureResult,
	type MimoShockData,
	type MimoShockLawEntry,
	type MimoShockLive,
	type MimoShockProceedParams,
	type MimoShockPulseEvent,
	type MimoShockPulseRow,
	type MimoShockQuantity,
	type MimoShockState,
	type MimoShockSubscribeParams,
	type MimoShockSubscribeResult
} from './mimoShock';
import { fromMsConfig, msByKey, toMsConfig } from '$lib/acquire/mimoshock/msSetup';
import { verdictGrid } from '$lib/acquire/mimoshock/msGrid';
import { msLevelStatus, needsAcknowledgement, scaledCrossings, unreachableControls } from '$lib/acquire/mimoshock/msWords';

const DIR = fileURLToPath(new URL('../../../../schema/golden/', import.meta.url));
const golden = (name: string) => `${DIR}${name}`;
const has = (name: string) => existsSync(golden(name));
const readJson = (name: string) => JSON.parse(readFileSync(golden(name), 'utf-8'));
const readBin = (name: string) => new Uint8Array(readFileSync(golden(name)));

const ALL = readdirSync(DIR).filter((f) => /mimo_shock/.test(f));
const TEXT = ALL.filter((f) => f.endsWith('.json') && !f.endsWith('.expected.json')).sort();
const BIN = ALL.filter((f) => f.endsWith('.bin')).sort();
const QUANTITIES: MimoShockQuantity[] = ['pulse', 'srs', 'drives', 'verdicts', 'ladder', 'identification', 'review', 'law', 'readings', 'pulses'];

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

/** Every field a pulse row carries, as the shell's type names them. */
function expectRow(row: MimoShockPulseRow) {
	expect(Number.isInteger(row.index)).toBe(true);
	expect(typeof row.passed).toBe('boolean');
	for (const d of row.drives) expect(Object.keys(d).sort()).toEqual(['channel', 'limit_scale', 'vpk']);
	for (const c of row.controls) {
		expect(typeof c.channel).toBe('number');
		expect(typeof c.passed).toBe('boolean');
		for (const r of c.reasons) expect(['tolerance', 'timing', 'polarity', 'duration']).toContain(r);
		// a failed control names why; a passed one names nothing
		expect(c.passed).toBe(c.reasons.length === 0);
		for (const k of ['error_db', 'over_test_db', 'delay_s', 'peak', 'peak_ratio', 'scaled_margin_db', 'absolute_margin_db', 'energy_db', 'rms_duration_ratio'] as const) expect(typeof c[k]).toBe('number');
		expect(c.srs === null || typeof c.srs.worst_err_db === 'number').toBe(true);
	}
	expect(typeof row.arrival_spread_s).toBe('number');
	expect(Array.isArray(row.warnings)).toBe(true);
	expect(typeof row.law.used).toBe('boolean');
}

describe('MIMO Shock goldens in the folder', () => {
	it('are text frames or binary frames with their expected form', () => {
		expect(ALL.length).toBeGreaterThan(0);
		for (const f of BIN) expect(ALL).toContain(f.replace(/\.bin$/, '.expected.json'));
		for (const f of ALL) expect(f).toMatch(/\.(json|bin)$/);
	});

	for (const name of TEXT) {
		it(`${name} decodes and re-encodes as the same compact text frame`, () => {
			const text = readFileSync(golden(name), 'utf-8');
			const message = decodeFrame(text);
			expect(message).toEqual(JSON.parse(text));
			expect(encodeFrame(message)).toBe(JSON.stringify(JSON.parse(text)));
		});
	}

	for (const name of BIN) {
		it(`${name} is a binary frame that decodes to its expected form`, () => {
			const bin = readBin(name);
			expect(new TextDecoder().decode(bin.slice(0, 4))).toBe('SDS1');
			const expected = toTyped(readJson(name.replace(/\.bin$/, '.expected.json'))) as Record<string, unknown>;
			expect(decodeFrame(bin)).toEqual(expected);
		});
	}
});

describe('MIMO Shock goldens through the shell', () => {
	for (const name of ['request_mimo_shock_configure.json', 'request_mimo_shock_configure_spectrum.json']) {
		it.skipIf(!has(name))(`${name}: the configuration survives the setup draft and back, every field`, () => {
			const req = readJson(name) as Request<MimoShockConfigureParams>;
			expect(req.method).toBe('mimo_shock.configure');
			const config = req.params.config;
			// The law's entry, as the catalog would give it: every parameter the configuration names.
			const law: MimoShockLawEntry | null = config.control?.law
				? ({ id: config.control.law.id, params: Object.keys(config.control.law.params ?? {}).map((key) => ({ key, label: key, help: '', kind: 'number', unit: '', min: -1e9, max: 1e9, step: 1, default: 0 })) } as unknown as MimoShockLawEntry)
				: null;
			expect(toMsConfig(fromMsConfig(config), law)).toEqual(config);
		});
	}

	it.skipIf(!has('response_mimo_shock_configure.json'))('the configure result is a configured state with what the service derived', () => {
		const res = (readJson('response_mimo_shock_configure.json') as ResponseOk<MimoShockConfigureResult>).result;
		expect([res.state.state, res.state.phase]).toEqual(['configured', 'idle']);
		expect(res.derived.controls.length).toBeGreaterThan(0);
		for (const c of res.derived.controls) expect(c.peak).toBeGreaterThan(0);
		expect(msLevelStatus(res.state).text).toBe('Set up, ready to start');
		expect(typeof res.state.scaled_aborts).toBe('boolean');
	});

	it.skipIf(!has('response_error_mimo_shock_config_invalid.json'))("an invalid configuration's findings land on the setup's own fields", () => {
		const err = readJson('response_error_mimo_shock_config_invalid.json') as ResponseErr;
		expect([err.error.code, err.error.reason]).toEqual(['bad_request', 'config_invalid']);
		const items = (err.error.detail as { errors: { reason: string; field?: string | null; message: string }[] }).errors;
		const keys = Object.keys(msByKey(items));
		expect(keys).toEqual(expect.arrayContaining(['pulses', 'verdict']));
		for (const k of keys) expect(k).not.toMatch(/^config\.|\[|^target\./);
	});

	it.skipIf(!has('response_error_mimo_shock_unreachable.json'))('the unreachable refusal names each control with its floor', () => {
		const err = readJson('response_error_mimo_shock_unreachable.json') as ResponseErr;
		expect([err.error.code, err.error.reason]).toEqual(['refused', 'unreachable_target']);
		const controls = (err.error.detail as { controls: MimoShockState['review'] extends infer R ? (R extends { controls: infer C } ? C : never) : never }).controls;
		expect(unreachableControls({ controls, hold: null, acknowledged: null })).toEqual(controls.filter((c) => !c.reachable_ok).map((c) => c.channel));
	});

	it.skipIf(!has('request_mimo_shock_proceed.json'))('a proceed past the hold carries the operator\'s own words', () => {
		const p = (readJson('request_mimo_shock_proceed.json') as Request<MimoShockProceedParams>).params;
		expect(p.acknowledge?.trim().length).toBeGreaterThan(0);
	});

	it.skipIf(!has('event_mimo_shock_state.json'))('the running state carries the last pulse, the review and the law in force', () => {
		const ev = readJson('event_mimo_shock_state.json') as Event<MimoShockState>;
		expect(ev.topic).toBe(MIMO_SHOCK_TOPICS.state);
		const s = ev.data;
		expect(s.last_pulse).not.toBeNull();
		expectRow(s.last_pulse!);
		expect(s.last_pulse!.index).toBe(s.pulses.fired - 1);
		expect(s.levels_db?.[s.rung ?? -1]).toBe(s.level_db);
		expect(needsAcknowledgement(s)).toBe(false);
		const grid = verdictGrid([s.last_pulse!], s.review?.controls.map((c) => c.channel) ?? []);
		expect(grid.cells.flat().filter((c) => c && !c.passed).map((c) => c!.channel)).toEqual(s.last_pulse!.controls.filter((c) => !c.passed).map((c) => c.channel));
	});

	it.skipIf(!has('event_mimo_shock_state_review.json'))('the review state holds for an acknowledgement when a floor is above tolerance', () => {
		const s = (readJson('event_mimo_shock_state_review.json') as Event<MimoShockState>).data;
		expect(s.state).toBe('review');
		expect(s.review?.hold?.reason).toBe('unreachable_target');
		expect(needsAcknowledgement(s)).toBe(true);
		expect(unreachableControls(s.review)).toEqual(s.review!.controls.filter((c) => !c.reachable_ok).map((c) => c.channel));
		expect(msLevelStatus(s).text).toMatch(/cannot reach/);
	});

	it.skipIf(!has('event_mimo_shock_state_unscaled.json'))('with the scaled aborts off, the state carries its standing notice and each margin', () => {
		const s = (readJson('event_mimo_shock_state_unscaled.json') as Event<MimoShockState>).data;
		expect(s.scaled_aborts).toBe(false);
		expect(s.notice?.length).toBeGreaterThan(0);
		expectRow(s.last_pulse!);
		expect(scaledCrossings(s.last_pulse!)).toEqual(s.last_pulse!.controls.filter((c) => c.scaled_margin_db > 0).map((c) => ({ channel: c.channel, over_db: c.scaled_margin_db })));
	});

	it.skipIf(!has('request_mimo_shock_subscribe.json'))('a subscription asks only for quantities the shell knows', () => {
		const p = (readJson('request_mimo_shock_subscribe.json') as Request<MimoShockSubscribeParams>).params;
		if ('plots' in p) for (const plot of p.plots) expect(QUANTITIES).toContain(plot.quantity);
	});

	it.skipIf(!has('response_mimo_shock_subscribe.json'))('the subscribe result lists the quantities the shell offers, no more', () => {
		const r = (readJson('response_mimo_shock_subscribe.json') as ResponseOk<MimoShockSubscribeResult>).result;
		expect([...r.quantities].sort()).toEqual([...QUANTITIES].sort());
	});

	it.skipIf(!has('event_mimo_shock_live.bin'))('the live frame carries the pulse counts and the time to the next pulse', () => {
		const ev = decodeFrame(readBin('event_mimo_shock_live.bin')) as unknown as Event<MimoShockLive>;
		expect(ev.topic).toBe(MIMO_SHOCK_TOPICS.live);
		expect(Object.keys(ev.data.pulses).sort()).toEqual(['at_level', 'fired', 'of_level', 'passed']);
	});

	for (const name of ['event_mimo_shock_pulse.bin', 'event_mimo_shock_pulse_spectrum.bin']) {
		it.skipIf(!has(name))(`${name}: a pulse event carries its row and a view per control and the drives`, () => {
			const ev = decodeFrame(readBin(name)) as unknown as Event<MimoShockPulseEvent>;
			expect(ev.topic).toBe(MIMO_SHOCK_TOPICS.pulse);
			expectRow(ev.data.pulse);
			const views = ev.data.views as (SpectrumView & { channel?: number })[];
			for (const c of ev.data.pulse.controls) expect(views.some((v) => v.title === `Pulse ${c.channel}` && v.channel === c.channel)).toBe(true);
			expect(views.at(-1)?.title).toBe('Drives');
			expect(views.at(-1)?.series.map((s) => s.name)).toEqual(ev.data.pulse.drives.map((d) => `Drive ${d.channel}`));
		});
	}

	it.skipIf(!has('event_mimo_shock_data.bin'))("a data frame's plots are quantities the shell draws, each with views", () => {
		const ev = decodeFrame(readBin('event_mimo_shock_data.bin')) as unknown as Event<MimoShockData>;
		expect(ev.topic).toBe(MIMO_SHOCK_TOPICS.data);
		for (const p of ev.data.plots) {
			expect(QUANTITIES).toContain(p.quantity);
			for (const v of p.views) expect(['spectrum', 'reading', 'table']).toContain(v.view);
		}
		const verdicts = ev.data.plots.find((p) => p.quantity === 'verdicts')?.views[0] as TableView | undefined;
		if (verdicts) for (const row of verdicts.rows) expect(['pass', 'fail']).toContain(row[1]);
	});
});
