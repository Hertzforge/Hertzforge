/**
 * The classical shock goldens (schema/golden/) through the shell's codec, types and setup rules.
 * The files are read from the folder itself, so this suite, the Rust suite and the Python suite
 * all read the same frames: a golden added there is checked here without a list to update. Each
 * check that names a golden runs when that golden is in the folder.
 */
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { decodeFrame, encodeFrame, withShape } from './codec';
import type { Dtype, Event, Request, ResponseErr, ResponseOk, SpectrumView } from './types';
import type { LawsListResult } from './mimo';
import {
	SHOCK_TOPICS,
	type ShockConfigureParams,
	type ShockConfigureResult,
	type ShockData,
	type ShockLevelParams,
	type ShockLive,
	type ShockPreviewResult,
	type ShockPulseEvent,
	type ShockQuantity,
	type ShockState,
	type ShockSubscribeParams
} from './shock';
import { lawsOfWorld } from '$lib/acquire/law/lawForm';
import { fromShockConfig, shockByKey, toShockConfig } from '$lib/acquire/shock/shockSetup';
import { lastPulseWords, pulseWords, shockLevelStatus } from '$lib/acquire/shock/shockWords';
import { db } from '$lib/acquire/words';

const DIR = fileURLToPath(new URL('../../../../schema/golden/', import.meta.url));
const golden = (name: string) => `${DIR}${name}`;
const has = (name: string) => existsSync(golden(name));
const readJson = (name: string) => JSON.parse(readFileSync(golden(name), 'utf-8'));
const readBin = (name: string) => new Uint8Array(readFileSync(golden(name)));

/** Every classical shock golden in the folder: its text frames and its binary frames. */
const SHOCK = readdirSync(DIR).filter((f) => /(^|_)shock(_|\.)/.test(f) && !/mimo_shock|shock_mimo/.test(f));
const TEXT = SHOCK.filter((f) => f.endsWith('.json') && !f.endsWith('.expected.json')).sort();
const BIN = SHOCK.filter((f) => f.endsWith('.bin')).sort();
const QUANTITIES: ShockQuantity[] = ['pulse', 'velocity', 'displacement', 'srs', 'spectrum', 'identification', 'law', 'readings', 'pulses'];

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

describe('classical shock goldens in the folder', () => {
	it('are text frames or binary frames with their expected form', () => {
		for (const f of BIN) expect(SHOCK).toContain(f.replace(/\.bin$/, '.expected.json'));
		for (const f of SHOCK) expect(f).toMatch(/\.(json|bin)$/);
	});

	for (const name of TEXT) {
		it(`${name} decodes and re-encodes as the same compact text frame`, () => {
			const text = readFileSync(golden(name), 'utf-8');
			const message = decodeFrame(text);
			expect(message).toEqual(JSON.parse(text));
			expect(encodeFrame(message)).toBe(JSON.stringify(JSON.parse(text)));
		});
	}

	// The service writes these frames; the shell reads them. Re-encoding is not compared byte for
	// byte: the service may write a binary frame with no arrays, or 412.0 for a whole number, where
	// the shell's own encoder (the mock's) writes text, or 412. The values are what must agree.
	for (const name of BIN) {
		it(`${name} is a binary frame that decodes to its expected form`, () => {
			const bin = readBin(name);
			expect(new TextDecoder().decode(bin.slice(0, 4))).toBe('SDS1');
			const expected = toTyped(readJson(name.replace(/\.bin$/, '.expected.json'))) as Record<string, unknown>;
			expect(decodeFrame(bin)).toEqual(expected);
		});
	}
});

describe('classical shock goldens through the shell', () => {
	it.skipIf(!has('request_shock_configure.json'))('a configuration request survives the setup draft and back, every field', () => {
		const req = readJson('request_shock_configure.json') as Request<ShockConfigureParams>;
		const config = req.params.config;
		const law = has('response_laws_list_shock_classical.json')
			? lawsOfWorld((readJson('response_laws_list_shock_classical.json') as ResponseOk<LawsListResult>).result.laws, 'shock_classical').find((l) => l.id === config.control?.law?.id) ?? null
			: null;
		const back = toShockConfig(fromShockConfig(config), law);
		if (law) expect(back).toEqual(config);
		else expect({ ...back, control: { ...back.control, law: config.control?.law } }).toEqual(config);
	});

	it.skipIf(!has('response_laws_list_shock_classical.json'))('the classical world lists its built-in law first', () => {
		const list = (readJson('response_laws_list_shock_classical.json') as ResponseOk<LawsListResult>).result;
		const laws = lawsOfWorld(list.laws, 'shock_classical');
		expect(laws.length).toBe(list.laws.length);
		expect([laws[0].id, laws[0].builtin, laws[0].check.hardware]).toEqual(['fd_correct', true, true]);
		for (const p of laws[0].params) expect(['number', 'integer', 'toggle', 'choice']).toContain(p.kind);
	});

	it.skipIf(!has('response_shock_configure.json'))('the configure result is a configured state with what the service derived', () => {
		const res = (readJson('response_shock_configure.json') as ResponseOk<ShockConfigureResult>).result;
		expect([res.state.state, res.state.phase]).toEqual(['configured', 'idle']);
		const q = res.derived.quiet_window;
		expect(q.inside).toBe(q.worst_frac <= q.limit_frac);
		expect(shockLevelStatus(res.state).text).toBe('Set up, ready to arm');
	});

	it.skipIf(!has('response_error_shock_config_invalid.json'))("an invalid configuration's findings land on the fields they name", () => {
		const err = readJson('response_error_shock_config_invalid.json') as ResponseErr;
		expect([err.error.code, err.error.reason]).toEqual(['bad_request', 'config_invalid']);
		const items = (err.error.detail as { errors: { reason: string; field?: string | null; message: string }[] }).errors;
		const keys = Object.keys(shockByKey(items));
		expect(keys.length).toBeGreaterThan(0);
		for (const k of keys) expect(k).not.toMatch(/^config\.|\[/);
	});

	it.skipIf(!has('event_shock_state.json'))('the state event reads in the level card words', () => {
		const ev = readJson('event_shock_state.json') as Event<ShockState>;
		expect(ev.topic).toBe(SHOCK_TOPICS.state);
		const s = ev.data;
		const text = pulseWords(s, 7);
		if (s.state === 'running' && s.phase === 'waiting') expect(text).toBe(`Pulse ${s.pulses.fired + 1}${s.mode === 'manual' ? '' : ' of 7'} at ${db(s.level_db)}`);
		if (s.last_pulse) {
			// a row's index counts from 0: the last pulse is the one the count has just reached
			expect(s.last_pulse.index).toBe(s.pulses.fired - 1);
			expect(lastPulseWords(s.last_pulse, true)?.text).toMatch(new RegExp(`^Pulse ${s.last_pulse.index + 1} `));
		}
	});

	it.skipIf(!has('request_shock_level.json'))('a level move names a step or a level of the table', () => {
		const p = (readJson('request_shock_level.json') as Request<ShockLevelParams>).params;
		expect('rung' in p ? Number.isInteger(p.rung) : Number.isFinite(p.delta_db)).toBe(true);
	});

	it.skipIf(!has('request_shock_subscribe.json'))('a subscription asks only for quantities the shell knows', () => {
		const p = (readJson('request_shock_subscribe.json') as Request<ShockSubscribeParams>).params;
		if ('plots' in p) for (const plot of p.plots) expect(QUANTITIES).toContain(plot.quantity);
	});

	it.skipIf(!has('event_shock_live.bin'))('the live frame carries the pulse counts and the time to the next pulse', () => {
		const ev = decodeFrame(readBin('event_shock_live.bin')) as unknown as Event<ShockLive>;
		expect(ev.topic).toBe(SHOCK_TOPICS.live);
		expect(Object.keys(ev.data.pulses).sort()).toEqual(['at_level', 'fired', 'of_level', 'passed']);
	});

	it.skipIf(!has('event_shock_pulse.bin'))("a pulse event carries its row and the pages' views", () => {
		const ev = decodeFrame(readBin('event_shock_pulse.bin')) as unknown as Event<ShockPulseEvent>;
		expect(ev.topic).toBe(SHOCK_TOPICS.pulse);
		expect(Number.isInteger(ev.data.pulse.index)).toBe(true);
		expect(ev.data.views.length).toBeGreaterThan(0);
	});

	it.skipIf(!has('event_shock_data.bin'))("a data frame's plots are quantities the shell draws, each with views", () => {
		const ev = decodeFrame(readBin('event_shock_data.bin')) as unknown as Event<ShockData>;
		expect(ev.topic).toBe(SHOCK_TOPICS.data);
		for (const p of ev.data.plots) {
			expect(QUANTITIES).toContain(p.quantity);
			for (const v of p.views) expect(['spectrum', 'reading', 'table']).toContain(v.view);
		}
	});

	it.skipIf(!has('response_shock_preview.bin'))('the preview draws the pulse, its velocity and displacement', () => {
		const res = decodeFrame(readBin('response_shock_preview.bin')) as unknown as ResponseOk<ShockPreviewResult>;
		const views = res.result.views as SpectrumView[];
		expect(views.length).toBe(3);
		for (const v of views) expect(v.view).toBe('spectrum');
		expect(res.result.derived.reference.peak_g).toBeGreaterThan(0);
	});
});
