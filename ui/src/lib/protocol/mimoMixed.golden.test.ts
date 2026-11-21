/**
 * The MIMO Mixed Mode goldens (schema/golden/) through the shell's codec, types, setup and mock.
 * The files are read from the folder itself, so a golden added there is checked here without a
 * list to update. The mock is held to the same frames: the members of the state, a part's row, a
 * drive's row, the live frame and the configure answer, and the refusals by name and detail, so
 * the screens read the mock and the control service alike.
 */
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { decodeFrame, encodeFrame } from './codec';
import type { ChannelRow, Event, Request, ResponseOk } from './types';
import type { MimoMixedConfigureParams, MimoMixedState } from './mimoMixed';
import { defaultMmixSetup, fitChannels, toMmixConfig } from '$lib/acquire/mimomixed/mmixSetup';

const DIR = fileURLToPath(new URL('../../../../schema/golden/', import.meta.url));
const ALL = existsSync(DIR) ? readdirSync(DIR).filter((f) => /mimo_mixed/.test(f)) : [];
const TEXT = ALL.filter((f) => f.endsWith('.json') && !f.endsWith('.expected.json')).sort();
const BIN = ALL.filter((f) => f.endsWith('.bin')).sort();
const readJson = (name: string) => JSON.parse(readFileSync(`${DIR}${name}`, 'utf-8'));
const keys = (o: object) => Object.keys(o).sort();

/** Every member a state carries, as the shell's type names them (`record` once a folder is claimed). */
const STATE_KEYS = ['alarm', 'at_level_s', 'clock', 'drives', 'end', 'floor', 'floor_equalized', 'floor_error_db', 'grid', 'held', 'identification', 'law', 'level_db', 'levels_db', 'limiting', 'limits', 'mode', 'override', 'overrides', 'parts', 'phase', 'prediction', 'remaining_at_level_s', 'run_id', 'scale', 'source', 'state', 'station', 't_s', 'title'];
const LIVE_KEYS = ['alarm', 'at_level_s', 'class', 'drives', 'floor_equalized', 'floor_error_db', 'held', 'level_db', 'limiting', 'loop', 'mode', 'parts', 'phase', 'remaining_at_level_s', 'run_id', 'scale', 'source', 'state', 't_s', 'views'];

function row(index: number, role: ChannelRow['role']): ChannelRow {
	return { index, enabled: true, description: '', role, physical: null, interface: 'single_ended', iepe_bias_v: null, units: role === 'drive' ? 'V' : 'g', sensitivity_mv_per_eu: role === 'drive' ? 1000 : 100, range_max: 10, polarity: 'positive', control_group: null, abort: { enabled: false } } as unknown as ChannelRow;
}

/** The golden configuration's table: controls 1 to 4, drives 5 to 8. */
const TABLE: ChannelRow[] = [...[1, 2, 3, 4].map((i) => row(i, 'control')), ...[5, 6, 7, 8].map((i) => row(i, 'drive'))];

async function mock() {
	const { createControlServer } = await import('./mock/control');
	const { mockSocketFactory } = await import('./mock/socket');
	const { ServiceClient } = await import('./client.svelte');
	const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(createControlServer({ speed: 40 }), 1) });
	const { AcquireState } = await import('$lib/acquire/store.svelte');
	const acq = new AcquireState(client, { session: null, local: null, checkDelayMs: 0 });
	client.connect();
	await client.whenOpen();
	await acq.sync();
	await acq.claim();
	acq.rows = TABLE.map((r) => ({ ...r }));
	expect(await acq.applyTable()).toBe(true);
	return client;
}

/** A refusal's reason and detail, as the client throws it. */
async function refusal(p: Promise<unknown>): Promise<{ reason?: string; detail?: Record<string, unknown> }> {
	try {
		await p;
	} catch (e) {
		return e as { reason?: string; detail?: Record<string, unknown> };
	}
	throw new Error('not refused');
}

describe('the MIMO Mixed Mode goldens', () => {
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

	it.skipIf(!ALL.includes('event_mimo_mixed_state.json'))('the state carries every member the shell reads', () => {
		const s = readJson('event_mimo_mixed_state.json').data as MimoMixedState;
		expect(keys(s).filter((k) => k !== 'record')).toEqual(STATE_KEYS);
	});

	it.skipIf(!ALL.includes('event_mimo_mixed_live.expected.json'))('the live frame carries every member the shell reads', () => {
		expect(keys(readJson('event_mimo_mixed_live.expected.json').data)).toEqual(LIVE_KEYS);
	});

	it.skipIf(!ALL.includes('request_mimo_mixed_configure.json'))("the draft's configuration has the members of the service's, each part a body by kind", () => {
		const golden = (readJson('request_mimo_mixed_configure.json').params as MimoMixedConfigureParams).config as unknown as Record<string, unknown>;
		const d = defaultMmixSetup();
		fitChannels(d, TABLE);
		const wire = toMmixConfig(d, null, { floor: null, tones: null }) as unknown as Record<string, unknown>;
		// what the shell leaves to the service's defaults: the seed, the metadata, the article mass and
		// the simulator's structure (the standard one of the table's size); what the golden leaves to
		// them and the shell states: the identification and the record's streams; the specification
		// is sent only when a document is named
		const left = ['seed', 'metadata', 'article_mass_lb', 'simulator', 'identification', 'record', 'specification'];
		expect(keys(wire).filter((k) => !left.includes(k))).toEqual(keys(golden).filter((k) => !left.includes(k)));
		expect(keys(wire)).toEqual(expect.arrayContaining(['identification', 'record']));
		const parts = wire.parts as { id: string; body: { kind: string } }[];
		expect(parts[0]).toMatchObject({ id: 'floor', body: { kind: 'floor' } });
		expect(parts.slice(1).every((p) => p.body.kind === 'tone' || p.body.kind === 'narrowband')).toBe(true);
		const drive = (wire.drives as Record<string, unknown>[])[0];
		const g = (golden.drives as Record<string, unknown>[])[0];
		expect(keys(drive).filter((k) => k !== 'abort_vpk')).toEqual(keys(g).filter((k) => k !== 'moving_mass'));
	});
});

describe("the mock's MIMO Mixed Mode, against the goldens", () => {
	it("takes the golden configuration and answers with the service's members", async () => {
		const client = await mock();
		const req = readJson('request_mimo_mixed_configure.json').params;
		const golden = readJson('response_mimo_mixed_configure.json').result;
		const res = await client.request<Record<string, unknown> & { state: MimoMixedState; derived: Record<string, unknown> }>('mimo_mixed.configure', req);
		expect(keys(res)).toEqual(keys(golden));
		expect(keys(res.state).filter((k) => k !== 'record')).toEqual(STATE_KEYS);
		expect(keys(res.derived)).toEqual(keys(golden.derived));
		expect(keys(res.derived.law as object)).toEqual(['floor', 'tones']);
		expect(keys((res.derived.law as Record<string, object>).floor)).toEqual(keys(golden.derived.law.floor));
		expect(res.state.parts.map(keys)).toEqual(golden.state.parts.map(keys));
		expect(res.state.parts.map((p) => [p.id, p.kind, p.amplitude, p.unit])).toEqual(golden.state.parts.map((p: MimoMixedState['parts'][number]) => [p.id, p.kind, p.amplitude, p.unit]));
		expect(res.state.drives.map(keys)).toEqual(golden.state.drives.map(keys));
		// absent abort_vpk: the peak ceiling plus 1 dB
		expect(res.state.drives.map((d) => d.abort_vpk)).toEqual(golden.state.drives.map((d: { abort_vpk: number }) => d.abort_vpk));
		expect([res.state.state, res.state.phase, res.state.mode, res.state.levels_db]).toEqual([golden.state.state, golden.state.phase, golden.state.mode, golden.state.levels_db]);
		const s = await client.request<MimoMixedState>('mimo_mixed.state');
		expect(keys(s).filter((k) => k !== 'record')).toEqual(STATE_KEYS);
	});

	it('refuses a drive the table does not have as the service does', async () => {
		const client = await mock();
		const req = structuredClone(readJson('request_mimo_mixed_configure.json').params);
		req.config.drives[0].channel_index = 12;
		const golden = readJson('response_error_mimo_mixed_config_invalid.json').error;
		const e = await refusal(client.request('mimo_mixed.configure', req));
		expect(e.reason).toBe(golden.reason);
		const errors = (e.detail as { errors: unknown[] }).errors;
		expect(errors).toEqual(golden.detail.errors);
	});

	it('refuses the four verbs it does not build, by name', async () => {
		const client = await mock();
		const golden = readJson('response_error_mimo_mixed_not_built.json').error;
		expect((await refusal(client.request('mimo_mixed.adjust', { key: 'x', value: 1 }))).reason).toBe(golden.reason);
		expect((await refusal(client.request('mimo_mixed.exclude', {}))).reason).toBe('exclude_not_built');
		expect((await refusal(client.request('mimo_mixed.identification.save', {}))).reason).toBe('identification_save_not_built');
		expect((await refusal(client.request('mimo_mixed.identification.load', {}))).reason).toBe('identification_load_not_built');
	});

	it('answers the golden subscription as the service does, and refuses a quantity the wire does not name', async () => {
		const client = await mock();
		const req = readJson('request_mimo_mixed_subscribe.json').params;
		expect(await client.request('mimo_mixed.subscribe', req)).toEqual(readJson('response_mimo_mixed_subscribe.json').result);
		expect((await refusal(client.request('mimo_mixed.subscribe', { plots: [{ id: 'x', quantity: 'identification' }] }))).reason).toBe('bad_params');
		expect((await refusal(client.request('mimo_mixed.subscribe', { plots: [{ id: 'x', quantity: 'control', channels: 'all' }] }))).reason).toBe('bad_params');
	});

	it('switches the mode before arm, as the golden asks, and refuses the floor off with its detail', async () => {
		const client = await mock();
		await client.request('mimo_mixed.configure', readJson('request_mimo_mixed_configure.json').params);
		const auto = await client.request<{ mode: string }>('mimo_mixed.mode', { mode: 'auto' });
		expect(auto).toEqual({ mode: 'auto' });
		expect(await client.request('mimo_mixed.mode', readJson('request_mimo_mixed_mode.json').params)).toEqual({ mode: 'manual' });
		expect((await refusal(client.request('mimo_mixed.mode', { mode: 'automatic' }))).reason).toBe('bad_params');
		const golden = readJson('response_error_mimo_mixed_part_refused.json').error;
		const e = await refusal(client.request('mimo_mixed.part', { id: 'floor', on: false }));
		expect([e.reason, e.detail]).toEqual([golden.reason, golden.detail]);
		expect((await refusal(client.request('mimo_mixed.override', { ack: 'x' }))).reason).toBe('wrong_state');
	});
});
