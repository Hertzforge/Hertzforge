/**
 * The Playback goldens (schema/golden/) through the shell's codec, types, setup, views and words.
 * The files are read from the folder itself, so this suite, the Rust suite and the Python suite
 * all read the same frames: a golden added there is checked here without a list to update. Each
 * check that names a golden runs when that golden is in the folder.
 */
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { decodeFrame, encodeFrame, withShape } from './codec';
import type { Dtype, Event, Request, ResponseErr, SpectrumView } from './types';
import { isFullCheck, isRun, PLAYBACK_TOPICS, type PlaybackConfig, type PlaybackConfigureParams, type PlaybackInvalidDetail, type PlaybackLive, type PlaybackState } from './playback';
import { fromPlaybackConfig, PLAYBACK_SECTIONS, playbackByKey, playbackSectionOf, toPlaybackConfig } from '$lib/acquire/playback/playbackSetup';
import { timeView } from '$lib/acquire/playback/playbackViews.svelte';
import { PLAYBACK_OUTCOME_WORDS, playbackStatus, progressWords } from '$lib/acquire/playback/playbackWords';

const DIR = fileURLToPath(new URL('../../../../schema/golden/', import.meta.url));
const golden = (name: string) => `${DIR}${name}`;
const has = (name: string) => existsSync(golden(name));
const readJson = (name: string) => JSON.parse(readFileSync(golden(name), 'utf-8'));
const readBin = (name: string) => new Uint8Array(readFileSync(golden(name)));

const PLAYBACK = readdirSync(DIR).filter((f) => /(^|_)playback(_|\.)/.test(f) && statSync(golden(f)).isFile());
const TEXT = PLAYBACK.filter((f) => f.endsWith('.json') && !f.endsWith('.expected.json')).sort();
const BIN = PLAYBACK.filter((f) => f.endsWith('.bin')).sort();
/** The state's fields, as PROTOCOL lists them. */
const STATE_KEYS = ['ceiling_v', 'class', 'drive', 'drives', 'end', 'fs_hz', 'inputs', 'level_db', 'overloads', 'phase', 'progress', 'record', 'rolls', 'run_id', 'source', 'state', 'title'];

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

const testConfig = () => (readJson('request_playback_configure.json') as Request<PlaybackConfigureParams>).params.config;

describe('Playback goldens in the folder', () => {
	it('are Playback text frames or binary frames with their expected form', () => {
		expect(TEXT.length).toBeGreaterThan(0);
		for (const f of BIN) expect(PLAYBACK).toContain(f.replace(/\.bin$/, '.expected.json'));
	});

	for (const name of TEXT) {
		it(`${name} decodes and re-encodes as the same compact Playback text frame`, () => {
			const text = readFileSync(golden(name), 'utf-8');
			const message = decodeFrame(text);
			expect(message).toEqual(JSON.parse(text));
			expect(encodeFrame(message)).toBe(JSON.stringify(JSON.parse(text)));
		});
	}

	for (const name of BIN) {
		it(`${name} is a Playback binary frame that decodes to its expected form`, () => {
			const bin = readBin(name);
			expect(new TextDecoder().decode(bin.slice(0, 4))).toBe('SDS1');
			expect(decodeFrame(bin)).toEqual(toTyped(readJson(name.replace(/\.bin$/, '.expected.json'))));
		});
	}
});

describe('Playback goldens through the shell', () => {
	it.skipIf(!has('request_playback_configure.json'))('a Playback configuration request survives the setup draft and back, every field it gives', () => {
		const config = testConfig();
		const back = toPlaybackConfig(fromPlaybackConfig(config));
		// the run record's words are sent only when given: an empty note is not sent
		const { notes, ...rest } = config;
		expect(notes).toBe('');
		expect(back).toEqual(rest);
	});

	it.skipIf(!has('runs/playback/setup.json'))("a golden run's setup opens in the draft and sends the drive, the map and the numbers it ran with", () => {
		const config = readJson('runs/playback/setup.json').config as PlaybackConfig;
		const sent = toPlaybackConfig(fromPlaybackConfig(config));
		for (const k of ['title', 'drive', 'map', 'level_db', 'ceiling_v', 'pre_roll_s', 'post_roll_s', 'operator', 'article', 'specification', 'metadata'] as const) expect(sent[k], k).toEqual(config[k]);
	});

	it.skipIf(!has('event_playback_state.json'))('the Playback state event has every field and reads in the card words', () => {
		const ev = readJson('event_playback_state.json') as Event<PlaybackState>;
		expect(ev.topic).toBe(PLAYBACK_TOPICS.state);
		const s = ev.data;
		expect(isRun(s)).toBe(true);
		if (!isRun(s)) return;
		expect(Object.keys(s).sort()).toEqual(STATE_KEYS);
		expect(Object.keys(s.progress).sort()).toEqual(['buffered_s', 'elapsed_s', 'fraction', 'played', 'played_s', 'samples']);
		expect(playbackStatus(s.state, s.phase, s.end)).toBe('Playing the drive file');
		expect(progressWords(s.progress, s.fs_hz)).toBe('0.25 s of 0.50 s played (50 %), 0.25 s read ahead');
	});

	it.skipIf(!has('event_playback_live.bin'))("the live frame draws the inputs and the drives on the station's clock, each in its unit", () => {
		const ev = decodeFrame(readBin('event_playback_live.bin')) as unknown as Event<PlaybackLive>;
		expect(ev.topic).toBe(PLAYBACK_TOPICS.live);
		const l = ev.data;
		const names = (i: number) => `Row ${i}`;
		const [inputs] = timeView(l, 'inputs', 'all', names) as SpectrumView[];
		const [drives] = timeView(l, 'drives', 'all', names) as SpectrumView[];
		expect([inputs.title, inputs.y.unit, inputs.series.map((s) => s.name)]).toEqual(['Inputs', 'g', ['Row 1']]);
		expect([drives.title, drives.y.unit, drives.series.map((s) => s.name)]).toEqual(['Drives', 'V', ['Row 2']]);
		const x = inputs.series[0].x;
		expect(x[0]).toBe(l.time.t0_s);
		expect(x[1] - x[0]).toBeCloseTo(1 / l.time.rate_hz, 12);
		expect(Array.from(drives.series[0].y)).toEqual(Array.from(l.time.channels[1].values));
		// a plot showing chosen rows draws only those
		expect(timeView(l, 'inputs', [7], names)).toEqual([]);
	});

	it.skipIf(!has('response_error_playback_config_invalid.json'))("an invalid Playback configuration's findings land on the section they name", () => {
		const err = readJson('response_error_playback_config_invalid.json') as ResponseErr;
		expect([err.error.code, err.error.reason]).toEqual(['bad_request', 'config_invalid']);
		const items = (err.error.detail as unknown as PlaybackInvalidDetail).errors;
		expect(items.map((i) => i.reason)).toEqual(['peak_over_ceiling']);
		const keys = Object.keys(playbackByKey(items));
		expect(keys.map(playbackSectionOf)).toEqual(['check']);
		expect(PLAYBACK_SECTIONS.map((s) => s.id)).toContain('check');
	});

	it.skipIf(!has('runs/playback/setup.json'))("the golden run's check is a full report, and its end has words of its own", () => {
		const setup = readJson('runs/playback/setup.json');
		expect(isFullCheck(setup.check)).toBe(true);
		const end = readJson('runs/playback/end.json').end;
		expect(PLAYBACK_OUTCOME_WORDS[end.outcome as keyof typeof PLAYBACK_OUTCOME_WORDS]).toBe('Completed');
	});
});
