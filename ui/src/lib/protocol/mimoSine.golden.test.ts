/**
 * The MIMO Sine goldens (schema/golden/) through the shell's codec, types and setup rules. The
 * files are read from the folder itself, so a golden added there is checked here without a list
 * to update; until the control service writes them, the checks that need one are skipped and the
 * mock's own answers stand in, checked to the same shapes.
 */
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { decodeFrame, encodeFrame } from './codec';
import type { Event, Request, ResponseOk } from './types';
import type { MimoSineConfigureParams, MimoSineLiveHead, MimoSineReport, MimoSineState } from './mimoSine';
import { canonical } from '$lib/acquire/mimo/mimoSetup';
import { defaultMsineSetup, fromMsineConfig, toMsineConfig } from '$lib/acquire/mimosine/msineSetup';
import { notchWords } from '$lib/acquire/mimosine/msineWords';

const DIR = fileURLToPath(new URL('../../../../schema/golden/', import.meta.url));
const ALL = existsSync(DIR) ? readdirSync(DIR).filter((f) => /mimo_sine/.test(f)) : [];
const TEXT = ALL.filter((f) => f.endsWith('.json') && !f.endsWith('.expected.json')).sort();
const BIN = ALL.filter((f) => f.endsWith('.bin')).sort();
const readJson = (name: string) => JSON.parse(readFileSync(`${DIR}${name}`, 'utf-8'));

/** Every member a state carries, as the shell's type names them. */
const STATE_KEYS = ['alarm', 'at_level_s', 'clock', 'direction', 'elapsed_s', 'end', 'feasibility', 'frequency_hz', 'grid', 'held', 'identification', 'law', 'level_db', 'levels_db', 'mode', 'notch', 'notch_scale', 'phase', 'phase_specified', 'remaining_s', 'request_held', 'review', 'run_id', 'rung', 'shape', 'source', 'state', 'station', 'steps', 'sweeps', 'test', 'title'];

describe('the MIMO Sine goldens', () => {
	it.skipIf(TEXT.length === 0)('every text golden decodes through the codec and encodes back the same', () => {
		for (const f of TEXT) {
			const text = readFileSync(`${DIR}${f}`, 'utf-8');
			const msg = decodeFrame(text) as Request | ResponseOk | Event;
			expect(JSON.parse(encodeFrame(msg) as string), f).toEqual(JSON.parse(text));
		}
	});

	it.skipIf(BIN.length === 0)('every binary golden decodes through the codec', () => {
		for (const f of BIN) expect(() => decodeFrame(new Uint8Array(readFileSync(`${DIR}${f}`))), f).not.toThrow();
	});

	it.skipIf(!ALL.includes('request_mimo_sine_configure.json'))("the configure request's setup opens in the draft and goes out the same", () => {
		const params = readJson('request_mimo_sine_configure.json').params as MimoSineConfigureParams;
		const again = toMsineConfig(fromMsineConfig(params.config), null);
		expect(canonical(toMsineConfig(fromMsineConfig(again), null))).toBe(canonical(again));
		expect(again.channels.controls.map((c) => c.channel)).toEqual(params.config.channels.controls.map((c) => c.channel));
	});

	it.skipIf(!ALL.includes('request_mimo_sine_configure.json'))("the draft holds every member of the notch the service takes, at the service's defaults", () => {
		const notch = (readJson('request_mimo_sine_configure.json').params as MimoSineConfigureParams).config.limits!.notch!;
		const draft = defaultMsineSetup().limits.notch;
		expect(Object.keys(draft).sort()).toEqual(Object.keys(notch).sort());
		expect(draft).toEqual(notch);
	});

	it.skipIf(!ALL.includes('event_mimo_sine_live.expected.json'))("the live frame's notch reads as the drive cut and each control's depth", () => {
		const l = readJson('event_mimo_sine_live.expected.json').data as MimoSineLiveHead;
		expect(l.controls.every((c) => typeof c.notch_depth_db === 'number')).toBe(true);
		const depth = Math.max(0, ...l.controls.map((c) => c.notch_depth_db));
		// the golden frame is taken while the drive vector is cut to 0.75 and no reference is held down
		expect([l.notch, l.notch_scale, depth]).toEqual([true, 0.75, 0]);
		expect(notchWords(l.notch, l.notch_scale, depth)).toBe('Drive cut to 0.75 (2.5 dB)');
	});

	it.skipIf(!ALL.includes('event_mimo_sine_report.json'))("the report gives each limit channel's worst margin and the time the notch was in", () => {
		const r = readJson('event_mimo_sine_report.json').data.report as MimoSineReport;
		for (const l of r.limits) expect(Object.keys(l).sort()).toEqual(['channel', 'notched_s', 'worst_margin_db', 'worst_margin_hz']);
		expect(r.limits.some((l) => l.notched_s > 0)).toBe(true);
	});

	it.skipIf(!ALL.includes('response_mimo_sine_state.json'))('the state carries every member the shell reads', () => {
		const s = readJson('response_mimo_sine_state.json').result as MimoSineState;
		expect(Object.keys(s).filter((k) => k !== 'record').sort()).toEqual(STATE_KEYS);
	});
});

describe("the mock's MIMO Sine state", () => {
	it('carries every member the shell reads, as the protocol names them', async () => {
		const { createControlServer } = await import('./mock/control');
		const { mockSocketFactory } = await import('./mock/socket');
		const { ServiceClient } = await import('./client.svelte');
		const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(createControlServer({}), 1) });
		client.connect();
		await client.whenOpen();
		const s = await client.request<MimoSineState>('mimo_sine.state');
		expect(Object.keys(s).sort()).toEqual(STATE_KEYS);
	});
});
