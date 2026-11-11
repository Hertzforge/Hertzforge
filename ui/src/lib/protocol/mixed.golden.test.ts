/**
 * The Mixed Mode goldens (schema/golden/) through the shell's codec, setup rules and screen. The
 * files are read from the folder itself, so a golden the service adds is checked here without a
 * list to update; each check runs when the golden it names is in the folder. Until the service's
 * goldens exist, the shell's types are held to PROTOCOL.md's examples (`mixed.test.ts`) and this
 * suite checks nothing but that the folder holds none of a wrong form.
 */
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { decodeFrame, encodeFrame, withShape } from './codec';
import type { Dtype, Request, ResponseOk } from './types';
import type { MixedConfigureParams, MixedConfigureResult, MixedData, MixedLive, MixedState, MixedSubscribeParams } from './mixed';
import { fromMixedConfig, mixedByKey, toMixedConfig } from '$lib/acquire/mixed/mixedSetup';
import { MIXED_PLOT_TYPES, MIXED_QUANTITIES } from '$lib/acquire/mixed/mixedLayout';

const DIR = fileURLToPath(new URL('../../../../schema/golden/', import.meta.url));
const golden = (name: string) => `${DIR}${name}`;
const has = (name: string) => existsSync(golden(name));
const readJson = (name: string) => JSON.parse(readFileSync(golden(name), 'utf-8'));
const readBin = (name: string) => new Uint8Array(readFileSync(golden(name)));

/** Every Mixed Mode golden in the folder: its text frames and its binary frames. */
const MIXED = existsSync(DIR) ? readdirSync(DIR).filter((f) => /(^|_)mixed(_|\.)/.test(f)) : [];
const TEXT = MIXED.filter((f) => f.endsWith('.json') && !f.endsWith('.expected.json')).sort();
const BIN = MIXED.filter((f) => f.endsWith('.bin')).sort();

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

describe('Mixed Mode goldens in the folder', () => {
	it('are text frames or binary frames with their expected form', () => {
		for (const f of BIN) expect(MIXED).toContain(f.replace(/\.bin$/, '.expected.json'));
		for (const f of MIXED) expect(f).toMatch(/\.(json|bin)$/);
	});

	for (const name of TEXT) {
		it(`${name} decodes and re-encodes as the same compact text frame`, () => {
			const text = readFileSync(golden(name), 'utf-8');
			expect(encodeFrame(decodeFrame(text))).toBe(JSON.stringify(JSON.parse(text)));
		});
	}

	for (const name of BIN) {
		it(`${name} is a binary frame that decodes to its expected form`, () => {
			const bin = readBin(name);
			expect(new TextDecoder().decode(bin.slice(0, 4))).toBe('SDS1');
			expect(decodeFrame(bin)).toEqual(toTyped(readJson(name.replace(/\.bin$/, '.expected.json'))));
		});
	}
});

describe('Mixed Mode goldens through the shell', () => {
	it.runIf(has('request_mixed_configure.json'))("the configuration reads into the setup and writes back with every part as it came", () => {
		const c = (readJson('request_mixed_configure.json') as Request<MixedConfigureParams>).params.config;
		const back = toMixedConfig(fromMixedConfig(c), { floor: null, tones: null });
		expect(back.parts.map((p) => p.id)).toEqual(c.parts.map((p) => p.id));
		expect(back.parts.length).toBe(c.parts.length);
	});

	it.runIf(has('response_mixed_configure.json'))('the configure result carries a state the store reads and derived values for every part', () => {
		const r = (readJson('response_mixed_configure.json') as ResponseOk<MixedConfigureResult>).result;
		expect(r.state.state).toBe('configured');
		expect(r.derived.parts.length).toBeGreaterThan(0);
	});

	it.runIf(has('response_error_mixed_config_invalid.json'))('every finding of a refused configuration lands on a field of the setup', () => {
		const e = readJson('response_error_mixed_config_invalid.json') as { error: { detail: { errors: { reason: string; message: string; field?: string }[] } } };
		const c = has('request_mixed_configure.json') ? (readJson('request_mixed_configure.json') as Request<MixedConfigureParams>).params.config : null;
		const keys = Object.keys(mixedByKey(c ? fromMixedConfig(c) : { tones: [], bands: [] }, e.error.detail.errors));
		expect(keys).not.toContain('setup');
	});

	it.runIf(has('event_mixed_state.json'))('the state event names every part with a row the parts table reads', () => {
		const s = readJson('event_mixed_state.json').data as MixedState;
		for (const p of s.parts) expect(p.id && p.kind && 'on' in p && 'enabled' in p).toBeTruthy();
	});

	it.runIf(has('event_mixed_live.bin'))('the live frame carries the four views in their order', () => {
		const l = decodeFrame(readBin('event_mixed_live.bin')) as unknown as { data: MixedLive };
		expect(l.data.views.map((v) => v.title)).toEqual(['Control', 'Residual', 'Drive', 'Run']);
	});

	it.runIf(has('event_mixed_data.bin'))('a data frame resolves per-part plots to part ids and spectra to stepped series with edges', () => {
		const d = (decodeFrame(readBin('event_mixed_data.bin')) as unknown as { data: MixedData }).data;
		for (const p of d.plots) {
			expect(MIXED_QUANTITIES).toContain(p.quantity);
			if (MIXED_PLOT_TYPES.find((t) => t.id === p.quantity)?.parts) expect(Array.isArray(p.parts)).toBe(true);
			for (const v of p.views) if (v.view === 'spectrum') for (const s of v.series) if (s.shape === 'steps') expect(s.edges!.length).toBe(s.y.length + 1);
		}
	});

	it.runIf(has('request_mixed_subscribe.json'))('a subscription the service took is one the screen sends', () => {
		const p = (readJson('request_mixed_subscribe.json') as Request<MixedSubscribeParams>).params;
		if ('plots' in p) for (const plot of p.plots) expect(MIXED_QUANTITIES).toContain(plot.quantity);
	});
});
