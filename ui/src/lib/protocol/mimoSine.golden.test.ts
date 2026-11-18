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
import type { MimoSineConfigureParams, MimoSineState } from './mimoSine';
import { canonical } from '$lib/acquire/mimo/mimoSetup';
import { fromMsineConfig, toMsineConfig } from '$lib/acquire/mimosine/msineSetup';

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
