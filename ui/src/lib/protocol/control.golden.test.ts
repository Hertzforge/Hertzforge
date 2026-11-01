/**
 * The hardware and Random control goldens (schema/golden/) through the shell's codec and types.
 *
 * Each typed literal below is the golden's content written against the TypeScript types, so the
 * type check proves the types accept what the contract says, and the test proves the golden says
 * exactly that.
 */
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { decodeFrame, dtypeOf, encodeFrame, withShape } from './codec';
import type {
	Dtype,
	Event,
	RandomConfig,
	RandomLive,
	RandomState,
	ResponseErr,
	ResponseOk,
	StationList,
	ChannelTable,
	RandomLogEvent,
	SessionChanged
} from './types';

const golden = (name: string) => fileURLToPath(new URL(`../../../../schema/golden/${name}`, import.meta.url));
const readBin = (name: string) => new Uint8Array(readFileSync(golden(name)));
const readJson = (name: string) => JSON.parse(readFileSync(golden(name), 'utf-8'));

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

const TEXT = [
	'request_random_configure',
	'response_random_configure',
	'response_station_list',
	'response_channels_get',
	'event_random_state',
	'event_session_changed',
	'event_random_log',
	'response_error_controller_held',
	'response_error_table_invalid'
];

describe('random.live golden', () => {
	it('encodes byte for byte from its expected form', () => {
		const message = toTyped(readJson('event_random_live.expected.json')) as Record<string, unknown>;
		const bytes = encodeFrame(message) as Uint8Array;
		expect(Array.from(bytes)).toEqual(Array.from(readBin('event_random_live.bin')));
	});

	it('decodes into three views with every series stepped on the same lines', () => {
		const event = decodeFrame(readBin('event_random_live.bin')) as unknown as Event<RandomLive>;
		const live: RandomLive = event.data;
		expect(event.topic).toBe('random.live');
		expect([live.source, live.class]).toEqual(['HARDWARE', 'ni_real']);
		const [control, drive, run] = live.views;
		expect(control.series.map((s) => s.role)).toEqual(['measured', 'reference', 'tolerance', 'tolerance', 'abort', 'abort']);
		expect(drive.y.unit).toBe('V²/Hz');
		expect(run.view).toBe('reading');
		const lines = Array.from(control.series[0].x);
		for (const s of [...control.series, ...drive.series]) {
			expect(s.shape).toBe('steps');
			expect(dtypeOf(s.x)).toBe('f64');
			expect(Array.from(s.x)).toEqual(lines);
			expect(s.edges?.length).toBe(lines.length + 1);
		}
		expect(live.alarms).toEqual([{ kind: 'envelope', lines: 2, worst_db: 3.4 }]);
		expect(live.lines).toEqual({ in_band: 317, alarm: 2, abort: 0 });
	});
});

describe('control text goldens', () => {
	for (const name of TEXT) {
		it(`${name} decodes and re-encodes as the same compact text frame`, () => {
			const text = readFileSync(golden(`${name}.json`), 'utf-8');
			const message = decodeFrame(text);
			expect(message).toEqual(JSON.parse(text));
			expect(encodeFrame(message)).toBe(JSON.stringify(JSON.parse(text)));
		});
	}

	it('a refusal carries its reason and detail', () => {
		const err = decodeFrame(readFileSync(golden('response_error_controller_held.json'), 'utf-8')) as ResponseErr;
		expect(err.error).toMatchObject({ code: 'refused', reason: 'controller_held', detail: { holder: 'Shell on bench-pc' } });
		const invalid = decodeFrame(readFileSync(golden('response_error_table_invalid.json'), 'utf-8')) as ResponseErr;
		expect(invalid.error.reason).toBe('table_invalid');
		expect((invalid.error.detail?.errors as { reason: string }[]).map((e) => e.reason)).toEqual(['zero_sensitivity', 'physical_channel_shared']);
	});

	it('the configure request is a RandomConfig', () => {
		const config: RandomConfig = {
			title: 'Qualification Random',
			profile: {
				breakpoints: [
					{ freq_hz: 20, psd_g2_hz: 0.01 },
					{ freq_hz: 80, slope_db_oct: 3 },
					{ freq_hz: 350, psd_g2_hz: 0.04, alarm_db: [2, 2] },
					{ freq_hz: 2000, slope_db_oct: -3 }
				],
				alarm_db: [3, 3],
				abort_db: [6, 6]
			},
			acquisition: { fs_hz: 8192, n_pts: 2048, n_avg: 4 },
			schedule: { mode: 'auto', levels: [{ db: -12, dwell_s: 30 }, { db: -6, dwell_s: 30 }, { db: 0 }], full_level_s: 300 },
			control: {
				composite: 'weighted_avg',
				control_dof: 120,
				damping: 4,
				max_lines_out: 10,
				max_loops_out: 5,
				max_sigma: 5,
				drive_ceiling_vrms: 2.5,
				drive_abort_vrms: null,
				review_after_sysid: true,
				pretest: { averages: 4, max_vrms: 0.5, min_vrms: 0.01, shaped: false },
				level_ramp_s: 3,
				shutdown_ramp_s: 3
			},
			limits: [{ channel: 2, kind: 'response', rows: [{ freq_hz: 20, ceiling: 0.2 }, { freq_hz: 2000, ceiling: 0.2 }] }],
			feasibility: null
		};
		expect(readJson('request_random_configure.json').params.config).toEqual(config);
	});

	it('the aborted run state is a RandomState', () => {
		const state: RandomState = {
			run_id: 'r-0007',
			state: 'finished',
			phase: 'aborted',
			title: 'Qualification Random',
			source: 'HARDWARE',
			station: {
				kind: 'ni',
				class: 'ni_real',
				devices: [
					{ id: 'cDAQ1Mod1', serial: '01A2B3C4' },
					{ id: 'cDAQ1Mod2', serial: '01A2B3C5' }
				]
			},
			grid: { fs_hz: 51200, n_pts: 8192, df_hz: 6.25, lines: 317, loop_s: 0.64, actual: true },
			clock: 'shared',
			mode: 'auto',
			levels_db: [-12, -6, 0],
			level_db: null,
			full_level_s: 300,
			elapsed_s: 131.2,
			at_level_s: 42.5,
			remaining_at_level_s: null,
			alarm: false,
			feasibility: { verdict: 'not_evaluated', message: 'No shaker was given, so the profile was not checked against shaker ratings.' },
			end: {
				outcome: 'aborted',
				message: 'Envelope abort: 14 lines outside the abort band, worst +7.2 dB, at 0.0 dB.',
				at_s: 131.2,
				level_db: 0,
				trip: { kind: 'envelope', abort_lines: 14, worst_db: 7.2, at_level_db: 0, at_loop: 212 }
			}
		};
		expect((decodeFrame(readFileSync(golden('event_random_state.json'), 'utf-8')) as unknown as Event<RandomState>).data).toEqual(state);
	});

	it('the station list, channel table and session change match their types', () => {
		const stations = (readJson('response_station_list.json') as ResponseOk<StationList>).result;
		expect(stations.stations.map((s) => s.kind)).toEqual(['sim', 'ni']);
		const [, ni] = stations.stations;
		expect(ni.devices.map((d) => [d.id, d.parent, d.timing_family])).toEqual([
			['cDAQ1', null, 'cDAQ1'],
			['cDAQ1Mod1', 'cDAQ1', 'cDAQ1'],
			['cDAQ1Mod2', 'cDAQ1', 'cDAQ1']
		]);
		expect(stations.selected).toEqual({
			kind: 'ni',
			devices: ['cDAQ1Mod1', 'cDAQ1Mod2'],
			class: 'ni_real',
			source: 'HARDWARE',
			inputs: 4,
			outputs: 4,
			timing_family: 'cDAQ1'
		} satisfies StationList['selected']);

		const table = (readJson('response_channels_get.json') as ResponseOk<ChannelTable>).result;
		expect(table.channels.every((c) => c.interface === 'single_ended')).toBe(true);
		expect(table.report.warnings.map((w) => w.reason)).toEqual(['drive_channel_present']);

		const session: SessionChanged = { role: 'observer', controller: { name: 'Shell on bench-pc', present: false }, orphan_s: 21.5 };
		expect(readJson('event_session_changed.json').data).toEqual(session);

		const log: RandomLogEvent = {
			run_id: 'r-0007',
			entries: [{ seq: 41, at_unix: 1793577731.2, kind: 'trip', text: 'Envelope abort: 14 lines outside the abort band, worst +7.2 dB, at 0.0 dB.' }]
		};
		expect(readJson('event_random_log.json').data).toEqual(log);
	});
});
