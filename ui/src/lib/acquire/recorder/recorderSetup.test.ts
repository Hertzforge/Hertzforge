/**
 * The Recorder setup draft against the wire: every field explicit on the way out and read back
 * unchanged, the trigger row and level sent only where the wire requires them, the draft read back
 * from storage, what the editor says before anything is sent, and where each of the service's
 * findings lands.
 */
import { describe, expect, it } from 'vitest';
import type { RecorderConfig } from '$lib/protocol/recorder';
import {
	defaultRecorderSetup,
	fromRecorderConfig,
	localErrors,
	mergeRecorderSetup,
	RECORDER_SECTIONS,
	recorderByKey,
	recorderItemKey,
	recorderRows,
	recorderSectionOf,
	toRecorderConfig,
	type RecorderDraft
} from './recorderSetup';
import { loadRecorderSetup, saveRecorderSetup } from './recorderStore.svelte';

/** PROTOCOL's example, every field given. */
const EXAMPLE: RecorderConfig = {
	title: 'Fixture A, run 4',
	acquisition: { fs_hz: 12800 },
	trigger: { mode: 'level', row: 1, level: 0.25, slope: 'rising', hysteresis: null, pre_s: 0.5, record_s: 1.0, count: 5, holdoff_s: 0 },
	analysis: { block: 2048, window: 'hann', overlap_pct: null, averaging: 'linear', averages: 8, decimation: 1, frf: { reference: 2, response: 4 } },
	capture: { run_name: 'recorder', keep_free_gb: 1 },
	metadata: [{ key: 'run_number', label: 'Run Number', value: '4' }]
};

describe('the Recorder setup draft', () => {
	it('sends every Recorder wire field and reads each back unchanged', () => {
		expect(toRecorderConfig(fromRecorderConfig(EXAMPLE))).toEqual(EXAMPLE);
		const keys = (c: RecorderConfig) => ({ top: Object.keys(c).sort(), trigger: Object.keys(c.trigger ?? {}).sort(), analysis: Object.keys(c.analysis ?? {}).sort(), capture: Object.keys(c.capture ?? {}).sort() });
		const level = { ...defaultRecorderSetup(), trigger: { ...defaultRecorderSetup().trigger, mode: 'level' as const, row: 1, level: 0.25, record_s: 1 } };
		expect(keys(toRecorderConfig(level))).toEqual(keys(EXAMPLE));
	});

	it('carries each Recorder field to the wire and back: change one and only that one changes', () => {
		const base = toRecorderConfig(fromRecorderConfig(EXAMPLE));
		const cases: [(d: RecorderDraft) => void, (c: RecorderConfig) => void][] = [
			[(d) => (d.title = 'Fixture B'), (c) => (c.title = 'Fixture B')],
			[(d) => (d.acquisition.fs_hz = 51200), (c) => (c.acquisition!.fs_hz = 51200)],
			[(d) => (d.trigger.row = 3), (c) => (c.trigger!.row = 3)],
			[(d) => (d.trigger.level = -0.4), (c) => (c.trigger!.level = -0.4)],
			[(d) => (d.trigger.slope = 'falling'), (c) => (c.trigger!.slope = 'falling')],
			[(d) => (d.trigger.hysteresis = 0.02), (c) => (c.trigger!.hysteresis = 0.02)],
			[(d) => (d.trigger.pre_s = 2), (c) => (c.trigger!.pre_s = 2)],
			[(d) => (d.trigger.record_s = 3), (c) => (c.trigger!.record_s = 3)],
			[(d) => (d.trigger.count = null), (c) => (c.trigger!.count = null)],
			[(d) => (d.trigger.holdoff_s = 3), (c) => (c.trigger!.holdoff_s = 3)],
			[(d) => (d.analysis.block = 4096), (c) => (c.analysis!.block = 4096)],
			[(d) => (d.analysis.window = 'flattop'), (c) => (c.analysis!.window = 'flattop')],
			[(d) => (d.analysis.overlap_pct = 25), (c) => (c.analysis!.overlap_pct = 25)],
			[(d) => (d.analysis.averaging = 'peak_hold'), (c) => (c.analysis!.averaging = 'peak_hold')],
			[(d) => (d.analysis.averages = 16), (c) => (c.analysis!.averages = 16)],
			[(d) => (d.analysis.decimation = 4), (c) => (c.analysis!.decimation = 4)],
			[(d) => (d.analysis.frf = null), (c) => (c.analysis!.frf = null)],
			[(d) => (d.capture.run_name = 'fixture_b'), (c) => (c.capture!.run_name = 'fixture_b')],
			[(d) => (d.capture.keep_free_gb = 20), (c) => (c.capture!.keep_free_gb = 20)],
			[(d) => (d.metadata[0].value = '5'), (c) => (c.metadata![0].value = '5')]
		];
		for (const [edit, expected] of cases) {
			const d = fromRecorderConfig(EXAMPLE);
			edit(d);
			const want = structuredClone(base);
			expected(want);
			// out to the wire, and back from it: the configuration the service holds reads back the same
			expect(toRecorderConfig(d)).toEqual(want);
			expect(toRecorderConfig(fromRecorderConfig(want))).toEqual(want);
		}
	});

	it('sends the Recorder trigger row and level only in level mode, and keeps them on the screen otherwise', () => {
		const d = fromRecorderConfig(EXAMPLE);
		d.trigger.mode = 'manual';
		const wire = toRecorderConfig(d);
		expect(wire.trigger).not.toHaveProperty('row');
		expect(wire.trigger).not.toHaveProperty('level');
		expect([d.trigger.row, d.trigger.level]).toEqual([1, 0.25]);
		d.trigger.mode = 'level';
		expect(toRecorderConfig(d).trigger).toMatchObject({ row: 1, level: 0.25 });
		const manual: RecorderConfig = { trigger: { mode: 'manual', pre_s: 1 } };
		expect(toRecorderConfig(fromRecorderConfig(manual)).trigger).toEqual({ mode: 'manual', slope: 'rising', hysteresis: null, pre_s: 1, record_s: null, count: 1, holdoff_s: 0 });
	});

	it('fills a Recorder configuration that leaves fields out with the wire defaults', () => {
		expect(toRecorderConfig(fromRecorderConfig({}))).toEqual({
			title: '',
			acquisition: { fs_hz: 12800 },
			trigger: { mode: 'manual', slope: 'rising', hysteresis: null, pre_s: 1, record_s: null, count: 1, holdoff_s: 0 },
			analysis: { block: 2048, window: 'hann', overlap_pct: null, averaging: 'linear', averages: 8, decimation: 1, frf: null },
			capture: { run_name: 'recorder', keep_free_gb: 1 },
			metadata: []
		});
	});

	it('reads a Recorder draft back from storage, an emptied field kept empty, a missing part from the defaults', () => {
		const d = defaultRecorderSetup();
		d.trigger.pre_s = NaN;
		const back = loadRecorderSetup(saveRecorderSetup(d));
		expect(Number.isNaN(back.trigger.pre_s)).toBe(true);
		expect(Number.isNaN(back.trigger.level)).toBe(true);
		expect(mergeRecorderSetup({ title: 'x', trigger: { mode: 'level' } as never }).trigger).toEqual({ ...defaultRecorderSetup().trigger, mode: 'level' });
		expect(mergeRecorderSetup({ trigger: { mode: 'free_run' } as never }).trigger.mode).toBe('manual');
		expect(loadRecorderSetup('not json')).toEqual(defaultRecorderSetup());
	});

	it('names what the Recorder editor can see before sending: a level trigger without its row, level or length, an empty field', () => {
		const d = defaultRecorderSetup();
		expect(localErrors(d)).toEqual({});
		d.trigger.mode = 'level';
		expect(localErrors(d)).toEqual({
			'trigger.row': ['Choose the row the level is read on.'],
			'trigger.level': ["Give the level, in the row's unit."],
			'trigger.record_s': ['A level recording needs a length, so the trigger can re-arm after it.']
		});
		Object.assign(d.trigger, { row: 1, level: 0.25, record_s: 1, count: 2.5, holdoff_s: NaN });
		d.capture.run_name = ' ';
		d.analysis.frf = { reference: 1, response: NaN };
		expect(localErrors(d)).toEqual({
			'trigger.holdoff_s': ['Enter a number.'],
			'trigger.count': ['A whole number of recordings.'],
			'analysis.frf': ['Choose both rows of the pair.'],
			'capture.run_name': ['Name the run folder.']
		});
	});

	it("lands each Recorder finding on its field and section: an item's field, a refusal's param, the table's own", () => {
		expect(recorderItemKey({ reason: 'row_not_recorded', field: 'trigger.row' })).toBe('trigger.row');
		expect(recorderItemKey({ reason: 'out_of_range', field: 'config.trigger.record_s' })).toBe('trigger.record_s');
		expect(recorderItemKey({ reason: 'row_not_recorded', field: 'analysis.frf.response' })).toBe('analysis.frf');
		expect(recorderItemKey({ reason: 'frf_same_row', field: 'analysis.frf' })).toBe('analysis.frf');
		expect(recorderItemKey({ reason: 'out_of_range', field: 'metadata[0].key' })).toBe('metadata');
		for (const reason of ['no_input_channel', 'drive_channel_present']) expect(recorderItemKey({ reason })).toBe('station.channels');
		const where: [string, string][] = [
			['station.channels', 'station'],
			['acquisition.fs_hz', 'trigger'],
			['trigger.level', 'trigger'],
			['analysis.block', 'analysis'],
			['analysis.frf', 'analysis'],
			['capture.keep_free_gb', 'capture'],
			['metadata', 'capture'],
			['title', 'capture']
		];
		for (const [k, s] of where) expect(recorderSectionOf(k)).toBe(s);
		expect(RECORDER_SECTIONS.map((s) => s.id)).toEqual(['station', 'trigger', 'analysis', 'capture']);
		expect(recorderByKey([{ reason: 'not_power_of_two', field: 'analysis.block', message: 'm' }, { reason: 'not_power_of_two', field: 'analysis.block', message: 'm' }])).toEqual({ 'analysis.block': ['m'] });
	});

	it('names the Recorder rows as PROTOCOL does: every enabled input in table order, the drive rows apart', () => {
		const row = (index: number, role: 'control' | 'monitor' | 'reference' | 'drive', enabled = true) => ({ index, role, enabled }) as never;
		const r = recorderRows([row(3, 'monitor'), row(1, 'control'), row(2, 'drive'), row(4, 'reference'), row(5, 'monitor', false)]);
		expect(r.recorded.map((x) => (x as { index: number }).index)).toEqual([3, 1, 4]);
		expect(r.drives.map((x) => (x as { index: number }).index)).toEqual([2]);
	});
});
