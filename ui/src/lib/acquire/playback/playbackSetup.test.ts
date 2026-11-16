/**
 * The Playback draft: its defaults, the configuration it sends, what the editor names before
 * anything is sent, where a service finding lands, and the map a new drive file takes.
 */
import { describe, expect, it } from 'vitest';
import type { ChannelRow } from '$lib/protocol/types';
import { defaultPlaybackSetup, fitMap, fromPlaybackConfig, localErrors, mergePlaybackSetup, playbackItemKey, playbackRows, playbackSectionOf, toPlaybackConfig } from './playbackSetup';
import { playbackBarActions, playbackPrimary, type PlaybackVerbContext } from './playbackVerbs';
import { endsOnly, residual, verdict } from './playbackWords';

const SHA = 'c7bf34a8e0068f902999239fcd548bb768c8bf17de58a8724f69a87fa24a8954';
const row = (index: number, role: ChannelRow['role'], enabled = true) => ({ index, role, enabled, description: `R${index}`, range_max: 10, units: role === 'drive' ? 'V' : 'g' }) as unknown as ChannelRow;

describe('the Playback draft', () => {
	it('starts at the protocol defaults and names what is missing before anything is sent', () => {
		const d = defaultPlaybackSetup();
		expect([d.level_db, d.pre_roll_s, d.post_roll_s, d.capture.run_name]).toEqual([-12, 0.5, 2, 'playback']);
		const e = localErrors(d);
		expect(Object.keys(e).sort()).toEqual(['ceiling_v', 'drive']);
		d.drive = { folder: '/data/drives/road', sha256: SHA };
		d.map = [2, null];
		d.ceiling_v = 1;
		expect(localErrors(d)).toEqual({ map: ['Choose the drive row file channel 1 plays on.'] });
		d.map = [2, 3];
		expect(localErrors(d)).toEqual({});
	});

	it('sends every field explicit, the run record words only when given, and opens back the same', () => {
		const d = defaultPlaybackSetup();
		Object.assign(d, { title: ' Road ', drive: { folder: '/d', sha256: SHA }, map: [3, 4], ceiling_v: 2 });
		const c = toPlaybackConfig(d);
		expect(Object.keys(c).sort()).toEqual(['capture', 'ceiling_v', 'drive', 'level_db', 'map', 'metadata', 'post_roll_s', 'pre_roll_s', 'record', 'title']);
		expect(c.title).toBe('Road');
		d.record = { ...d.record, operator: 'A. Operator', clause: '4.2', document: 'Procedure' };
		const c2 = toPlaybackConfig(d);
		expect([c2.operator, c2.specification]).toEqual(['A. Operator', { document: 'Procedure', clause: '4.2' }]);
		expect(toPlaybackConfig(fromPlaybackConfig(c2))).toEqual(c2);
	});

	it('reads a stored draft back with every part filled', () => {
		const m = mergePlaybackSetup({ title: 'x', drive: { folder: '/d', sha256: '' } } as never);
		expect(m.title).toBe('x');
		expect(m.map).toEqual([]);
		expect(m.capture.run_name).toBe('playback');
	});

	it("maps a new file's channels onto the free enabled drive rows in table order, keeping the rows already chosen", () => {
		const { drives, inputs } = playbackRows([row(1, 'control'), row(2, 'drive'), row(3, 'drive', false), row(4, 'drive'), row(5, 'drive')]);
		expect(inputs.map((r) => r.index)).toEqual([1]);
		expect(drives.map((r) => r.index)).toEqual([2, 4, 5]);
		expect(fitMap([], 2, drives)).toEqual([2, 4]);
		expect(fitMap([5], 3, drives)).toEqual([5, 2, 4]);
		expect(fitMap([2, 4, 5], 1, drives)).toEqual([2]);
		expect(fitMap([], 4, drives)).toEqual([2, 4, 5, null]);
	});

	it('puts each finding on its section: the table, the drive, the check, the level or the records', () => {
		const at = (reason: string, field?: string) => playbackSectionOf(playbackItemKey({ reason, field }));
		expect(at('no_input_channel')).toBe('station');
		expect(at('drive_pin_mismatch', 'drive')).toBe('drive');
		expect(at('map_repeated_row', 'map')).toBe('drive');
		expect(at('ends_not_zero', 'drive')).toBe('check');
		expect(at('peak_over_ceiling', 'level_db')).toBe('check');
		expect(at('ceiling_over_range', 'ceiling_v')).toBe('level');
		expect(at('out_of_range', 'config.pre_roll_s')).toBe('level');
		expect(at('out_of_range', 'capture.run_name')).toBe('capture');
	});
});

describe('the Playback verbs', () => {
	const ctx = (over: Partial<PlaybackVerbContext>): PlaybackVerbContext => ({ connected: true, controller: true, run: null, setup: 'none', setupErrors: 0, busy: null, pending: null, ...over });

	it('applies, starts once configured, stops and aborts while live, and offers a new run once finished', () => {
		expect(playbackPrimary(ctx({}))).toBe('apply');
		const configured = ctx({ run: { state: 'configured', phase: 'idle' }, setup: 'applied' });
		expect(playbackPrimary(configured)).toBe('start');
		expect(playbackBarActions(configured).start.enabled).toBe(true);
		expect(playbackBarActions(configured).arm.visible).toBe(false);
		const live = ctx({ run: { state: 'running', phase: 'playing' }, setup: 'applied', controller: false });
		const a = playbackBarActions(live);
		// any screen may stop or abort; an observer may not apply
		expect([a.stop.enabled, a.abort.enabled, a.apply.visible]).toEqual([true, true, false]);
		expect(playbackBarActions(ctx({ run: { state: 'finished', phase: 'done' }, setup: 'applied' })).reset.enabled).toBe(true);
		expect(playbackBarActions(ctx({ setupErrors: 2 })).apply.reason).toBe('Fix the 2 errors in the setup first.');
		expect(playbackBarActions(ctx({ run: { state: 'configured', phase: 'idle' }, setup: 'changed' })).start.reason).toMatch(/Apply it first/);
	});
});

describe('the drive check in words', () => {
	it('says a residual against its peak, a pass, each fault, and whether only the ends are refused', () => {
		expect(residual({ end: 0.0001, peak: 0.01 }, 'V s')).toBe('1.00e-4 V s of 0.01 V s (1 % of peak)');
		const c = { pass: false, note: null, faults: [{ text: 'The velocity ends at 5 % of its peak.' }, { text: 'The last sample is not zero.' }] };
		expect(verdict(c as never)).toBe('The velocity ends at 5 % of its peak; The last sample is not zero.');
		expect(verdict({ pass: false, note: null, faults: [{ text: 'starts at 0.05 V, more than 0.0027 V from zero' }, { text: 'ends at 0.05 V' }] } as never)).toBe('Starts at 0.05 V, more than 0.0027 V from zero; ends at 0.05 V.');
		expect(verdict({ pass: true, note: 'all zero', faults: [] } as never)).toBe('Pass: all zero');
		expect(endsOnly([{ reason: 'ends_not_zero' }])).toBe(true);
		expect(endsOnly([{ reason: 'ends_not_zero' }, { reason: 'peak_over_ceiling' }])).toBe(false);
		expect(endsOnly([])).toBe(false);
	});
});
