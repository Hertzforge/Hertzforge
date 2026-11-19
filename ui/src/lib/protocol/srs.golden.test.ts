/**
 * The SRS shock goldens (schema/golden/) through the shell's codec, types and setup rules. The
 * files are read from the folder itself, so this suite, the Rust suite and the Python suite all
 * read the same frames: a golden added there is checked here without a list to update. Each check
 * that names a golden runs when that golden is in the folder.
 */
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { decodeFrame, encodeFrame, withShape } from './codec';
import type { Dtype, Event, Request, ResponseErr, ResponseOk, SpectrumView } from './types';
import type { LawsListResult } from './mimo';
import {
	SRS_TOPICS,
	type SrsCheckRequirement,
	type SrsConfigureParams,
	type SrsConfigureResult,
	type SrsData,
	type SrsLevelParams,
	type SrsLive,
	type SrsLawsCheckResult,
	type SrsPreviewResult,
	type SrsPulseEvent,
	type SrsQuantity,
	type SrsState,
	type SrsSubscribeParams,
	type SrsSubscribeResult,
	type SrsTableImportResult,
	type SrsTargetImportResult
} from './srs';
import { lawsOfWorld } from '$lib/acquire/law/lawForm';
import { pulseViewOf } from '$lib/acquire/pulse/pulseLayout';
import { SRS_QUANTITIES, SRS_TIME_QUANTITIES } from '$lib/acquire/srs/srsLayout';
import { fromSrsConfig, srsByKey, toSrsConfig } from '$lib/acquire/srs/srsSetup';
import { lastPulseWords, pulseWords, srsLevelStatus, worstBandWords } from '$lib/acquire/srs/srsWords';
import { db } from '$lib/acquire/words';

const DIR = fileURLToPath(new URL('../../../../schema/golden/', import.meta.url));
const golden = (name: string) => `${DIR}${name}`;
const has = (name: string) => existsSync(golden(name));
const readJson = (name: string) => JSON.parse(readFileSync(golden(name), 'utf-8'));
const readBin = (name: string) => new Uint8Array(readFileSync(golden(name)));

/** Every SRS shock golden in the folder: its text frames and its binary frames. */
const SRS = readdirSync(DIR).filter((f) => /(^|_)srs(_|\.)/.test(f));
const TEXT = SRS.filter((f) => f.endsWith('.json') && !f.endsWith('.expected.json')).sort();
const BIN = SRS.filter((f) => f.endsWith('.bin')).sort();
const QUANTITIES: readonly SrsQuantity[] = SRS_QUANTITIES;
const REQUIREMENTS: SrsCheckRequirement[] = ['within_by', 'each_level_within', 'limit_held'];

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

/** A plot's point count: only a time history carries one, as the service has it. */
function pointsOnTimeOnly(plots: { quantity: SrsQuantity; points?: number }[]) {
	for (const p of plots) expect('points' in p && p.points !== undefined).toBe(SRS_TIME_QUANTITIES.includes(p.quantity));
}

describe('SRS shock goldens in the folder', () => {
	it('are text frames or binary frames with their expected form', () => {
		for (const f of BIN) expect(SRS).toContain(f.replace(/\.bin$/, '.expected.json'));
		for (const f of SRS) expect(f).toMatch(/\.(json|bin)$/);
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

describe('SRS shock goldens through the shell', () => {
	it.skipIf(!has('request_srs_configure.json'))('a configuration request survives the setup draft and back, every field', () => {
		const req = readJson('request_srs_configure.json') as Request<SrsConfigureParams>;
		const config = req.params.config;
		const law = has('response_laws_list_shock_srs.json')
			? (lawsOfWorld((readJson('response_laws_list_shock_srs.json') as ResponseOk<LawsListResult>).result.laws, 'shock_srs').find((l) => l.id === config.control?.law?.id) ?? null)
			: null;
		const back = toSrsConfig(fromSrsConfig(config), law);
		if (law) expect(back).toEqual(config);
		else expect({ ...back, control: { ...back.control, law: config.control?.law } }).toEqual(config);
	});

	it.skipIf(!has('response_laws_list_shock_srs.json'))('the SRS world lists its built-in law first, with a form the shell draws', () => {
		const list = (readJson('response_laws_list_shock_srs.json') as ResponseOk<LawsListResult>).result;
		const laws = lawsOfWorld(list.laws, 'shock_srs');
		expect(laws.length).toBe(list.laws.length);
		expect([laws[0].id, laws[0].builtin, laws[0].check.hardware]).toEqual(['srs_ratio', true, true]);
		for (const p of laws[0].params) expect(['number', 'integer', 'toggle', 'choice']).toContain(p.kind);
		for (const d of laws[0].diagnostics) expect(d.layout).toBeNull();
	});

	it.skipIf(!has('response_srs_configure.json'))('the configure result is a configured state with what the service derived', () => {
		const res = (readJson('response_srs_configure.json') as ResponseOk<SrsConfigureResult>).result;
		expect([res.state.state, res.state.phase]).toEqual(['configured', 'idle']);
		expect(res.derived.target_g.length).toBe(res.derived.bands_hz.length);
		expect(srsLevelStatus(res.state).text).toBe('Set up, ready to arm');
	});

	it.skipIf(!has('response_error_srs_config_invalid.json'))("an invalid configuration's findings land on the fields they name", () => {
		const err = readJson('response_error_srs_config_invalid.json') as ResponseErr;
		expect([err.error.code, err.error.reason]).toEqual(['bad_request', 'config_invalid']);
		const items = (err.error.detail as { errors: { reason: string; field?: string | null; message: string }[] }).errors;
		const keys = Object.keys(srsByKey(items));
		expect(keys.length).toBeGreaterThan(0);
		for (const k of keys) expect(k).not.toMatch(/^config\.|\[/);
	});

	it.skipIf(!has('event_srs_state.json'))('the state event reads in the level card words', () => {
		const ev = readJson('event_srs_state.json') as Event<SrsState>;
		expect(ev.topic).toBe(SRS_TOPICS.state);
		const s = ev.data;
		const text = pulseWords(s, 7);
		if (s.state === 'running' && s.phase === 'waiting') expect(text).toBe(`Pulse ${s.pulses.fired + 1}${s.mode === 'manual' ? '' : ' of 7'} at ${db(s.level_db)}`);
		if (s.last_pulse) {
			// a row's index counts from 0: the last pulse is the one the count has just reached
			expect(s.last_pulse.index).toBe(s.pulses.fired - 1);
			expect(lastPulseWords(s.last_pulse, true)?.text).toMatch(new RegExp(`^Pulse ${s.last_pulse.index + 1}\\b`));
			if (s.last_pulse.worst_err_db !== null) expect(worstBandWords(s.last_pulse)).toMatch(/^Worst band /);
		}
	});

	it.skipIf(!has('request_srs_level.json'))('a level move names a step of one decibel or a level of the table', () => {
		const p = (readJson('request_srs_level.json') as Request<SrsLevelParams>).params;
		if ('rung' in p) expect(Number.isInteger(p.rung)).toBe(true);
		else expect([1, -1]).toContain(p.delta_db);
	});

	it.skipIf(!has('request_srs_fire.json'))('a fire asks for nothing more', () => {
		const req = readJson('request_srs_fire.json') as Request<Record<string, unknown>>;
		expect(req.method).toBe('srs.fire');
		expect(Object.keys(req.params ?? {})).toEqual([]);
	});

	it.skipIf(!has('request_srs_subscribe.json'))('a subscription asks only for quantities the shell knows, a point count on the time histories only', () => {
		const p = (readJson('request_srs_subscribe.json') as Request<SrsSubscribeParams>).params;
		if ('plots' in p) {
			for (const plot of p.plots) expect(QUANTITIES).toContain(plot.quantity);
			pointsOnTimeOnly(p.plots);
		}
	});

	it.skipIf(!has('response_srs_subscribe.json'))('the subscription is answered with every default filled in', () => {
		const r = (readJson('response_srs_subscribe.json') as ResponseOk<SrsSubscribeResult>).result;
		expect([...r.quantities].sort()).toEqual([...QUANTITIES].sort());
		if (r.plots) pointsOnTimeOnly(r.plots);
	});

	it.skipIf(!has('event_srs_live.bin'))('the live frame carries the pulse counts and the time to the next pulse', () => {
		const ev = decodeFrame(readBin('event_srs_live.bin')) as unknown as Event<SrsLive>;
		expect(ev.topic).toBe(SRS_TOPICS.live);
		expect(Object.keys(ev.data.pulses).sort()).toEqual(['at_level', 'fired', 'of_level', 'passed']);
	});

	it.skipIf(!has('event_srs_pulse.bin'))("a pulse event carries its row and the SRS page's view", () => {
		const ev = decodeFrame(readBin('event_srs_pulse.bin')) as unknown as Event<SrsPulseEvent>;
		expect(ev.topic).toBe(SRS_TOPICS.pulse);
		expect(Number.isInteger(ev.data.pulse.index)).toBe(true);
		expect(pulseViewOf(ev.data.views, 'srs')).not.toBeNull();
		expect(pulseViewOf(ev.data.views, 'pulse')).not.toBeNull();
	});

	it.skipIf(!has('event_srs_data.bin'))("a data frame's plots are quantities the shell draws, each with views", () => {
		const ev = decodeFrame(readBin('event_srs_data.bin')) as unknown as Event<SrsData>;
		expect(ev.topic).toBe(SRS_TOPICS.data);
		for (const p of ev.data.plots) {
			expect(QUANTITIES).toContain(p.quantity);
			for (const v of p.views) expect(['spectrum', 'reading', 'table']).toContain(v.view);
		}
	});

	it.skipIf(!has('response_srs_preview.bin'))('the preview draws the target, the synthesis, the pulse and its kinematics', () => {
		const res = decodeFrame(readBin('response_srs_preview.bin')) as unknown as ResponseOk<SrsPreviewResult>;
		const views = res.result.views as SpectrumView[];
		expect(views.map((v) => v.title)).toEqual(['Target', 'Synthesis', 'Pulse', 'Velocity', 'Displacement']);
		for (const v of views) expect(v.view).toBe('spectrum');
		expect(res.result.derived.target_g.length).toBe(res.result.derived.bands_hz.length);
		expect(res.result.derived.reference.peak_g).toBeGreaterThan(0);
	});

	it.skipIf(!has('response_srs_target_import.json'))('a target read from a file is one the setup takes', () => {
		const r = (readJson('response_srs_target_import.json') as ResponseOk<SrsTargetImportResult>).result;
		if (r.target?.kind === 'breakpoints') for (let i = 1; i < r.target.points.length; i++) expect(r.target.points[i][0]).toBeGreaterThan(r.target.points[i - 1][0]);
		if (!r.target) expect(r.errors.length).toBeGreaterThan(0);
	});

	it.skipIf(!has('response_srs_table_import.json'))('a component table read from a file is for one engine', () => {
		const r = (readJson('response_srs_table_import.json') as ResponseOk<SrsTableImportResult>).result;
		if (r.components) expect(['wavsyn', 'seds']).toContain(r.components.engine);
		else expect(r.errors.length).toBeGreaterThan(0);
	});

	it.skipIf(!has('response_laws_check_shock_srs.json'))("a law check's cases name a requirement the shell knows", () => {
		const r = (readJson('response_laws_check_shock_srs.json') as ResponseOk<SrsLawsCheckResult>).result;
		expect(r.law.world).toBe('shock_srs');
		for (const c of r.report.cases) expect(REQUIREMENTS).toContain(c.requirement);
	});
});
