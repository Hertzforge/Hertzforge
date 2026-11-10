/**
 * The Recorder card's words for each state, phase and trigger mode, a saved recording, the disk's
 * backlog, a write fault, the end and where the recordings are, a mark, and the refusals' facts:
 * the words the operator reads, from the service's numbers only.
 */
import { describe, expect, it } from 'vitest';
import type { RecorderRecording } from '$lib/protocol/recorder';
import {
	backlogWords,
	bytesWords,
	endWords,
	faultWords,
	markWords,
	RECORDER_OUTCOME_WORDS,
	RECORDER_PHASE_WORDS,
	RECORDER_STATE_WORDS,
	recorderCardStatus,
	recorderModeFact,
	recorderOutcomeTone,
	recorderSpaceFacts,
	recorderStateFact,
	recordingFacts,
	savedWords,
	spanWords,
	type RecorderCardRun,
	type RecorderCardSetup
} from './recorderWords';

const GRID = { requested_fs_hz: 12800, fs_hz: 12800, block_frames: 256, batch_frames: 512, group_delay_frames: 38.44, actual: true };
const TRIGGER = { armed: true, pre_frames: 6400, held_frames: 6400, fires: 1, last_station_frame: 6422, saved: 1, left: 4, holdoff_left_s: 0 };
const run = (more: Partial<RecorderCardRun>): RecorderCardRun => ({ state: 'running', phase: 'monitoring', mode: 'level', trigger: TRIGGER, recording: null, grid: GRID, ...more });
const LEVEL: RecorderCardSetup = { row: { index: 1, unit: 'g' }, level: 0.25, slope: 'rising' };
const REC: RecorderRecording = {
	name: 'rec_0002',
	folder: 'recordings/rec_0002',
	trigger: 'level',
	requested_fs_hz: 12800,
	fs_hz: 12800,
	group_delay_frames: 38.44,
	rows: [1, 2, 3, 4],
	first_station_frame: 25622,
	trigger_station_frame: 32022,
	pre_frames: 6400,
	pre_frames_asked: 6400,
	frames: 19200,
	seconds: 1.5,
	bytes: 307200,
	gap_free: true,
	gaps: [],
	marks: 0,
	complete: true,
	fault: null
};

describe('the Recorder words', () => {
	it("say what a Recorder run is doing in the card's own words", () => {
		const text = (r: RecorderCardRun | null, s: RecorderCardSetup | null = LEVEL) => recorderCardStatus(r, s).text;
		expect(text(run({}))).toBe('Monitoring: nothing is being saved');
		expect(text(run({ phase: 'armed', trigger: { ...TRIGGER, held_frames: 6400 } }))).toBe('Armed: waiting for row 1 to rise through 0.250 g (pre-window 0.50 s of 0.50 s)');
		expect(text(run({ phase: 'armed', trigger: { ...TRIGGER, held_frames: 3200 } }), { ...LEVEL, level: -0.25, slope: 'falling' })).toBe('Armed: waiting for row 1 to fall through -0.250 g (pre-window 0.25 s of 0.50 s)');
		expect(text(run({ phase: 'armed', trigger: { ...TRIGGER, holdoff_left_s: 1.2 } }))).toBe('Armed: holding off 1.20 s, then waiting for row 1 to rise through 0.250 g (pre-window 0.50 s of 0.50 s)');
		expect(text(run({ mode: 'manual', phase: 'armed', trigger: { ...TRIGGER, pre_frames: 12800, held_frames: 6400 } }))).toBe('Armed: Record begins with the pre-window (pre-window 0.50 s of 1.00 s)');
		const recording = { name: 'rec_0002', trigger: 'level' as const, first_station_frame: 0, trigger_station_frame: 6400, pre_frames: 6400, pre_frames_asked: 6400, frames_fed: 11776, frames_on_disk: 10240, target_frames: 19200, seconds: 0.42, length_s: 1, backlog_s: 0.12 };
		expect(text(run({ phase: 'recording', recording }))).toBe('Recording rec_0002: 0.42 s of 1.00 s');
		expect(text(run({ mode: 'manual', phase: 'recording', recording: { ...recording, seconds: 4.2, length_s: null, target_frames: null } }))).toBe('Recording rec_0002: 4.20 s, until cut');
		expect(text(run({ state: 'starting', phase: 'opening' }))).toBe('Starting: the station is opening, nothing is recorded');
		expect(text(run({ state: 'stopping', phase: 'closing' }))).toBe('Stopping: the recordings are being finished');
		expect(text(run({ state: 'configured', phase: 'idle' }))).toBe('Set up, ready to start');
		expect(text(null)).toBe('Not set up');
	});

	it('name how a Recorder run ended and where its recordings are, and read only the operator stop as a plain end', () => {
		const end = (outcome: keyof typeof RECORDER_OUTCOME_WORDS, recordings = 6) => ({ outcome, message: '', at_s: 41.2, recordings });
		expect(recorderCardStatus(run({ state: 'finished', phase: 'stopped' }), null, end('operator_stop'))).toEqual({ mark: 'ended', text: 'Stopped by the Operator', tone: 'info' });
		expect(recorderCardStatus(run({ state: 'finished', phase: 'faulted' }), null, end('write_failed')).text).toBe('Stopped: A Write Failed');
		for (const o of Object.keys(RECORDER_OUTCOME_WORDS) as (keyof typeof RECORDER_OUTCOME_WORDS)[]) expect(recorderOutcomeTone(o)).toBe(o === 'operator_stop' ? 'info' : 'fail');
		expect(endWords(end('operator_stop'), '/home/op/Hertzforge_Data/recorder')).toBe('Stopped: 6 recordings in /home/op/Hertzforge_Data/recorder');
		expect(endWords(end('safe_stop', 1), 'recorder-2')).toBe('Stopped: 1 recording in recorder-2');
		expect(endWords(end('operator_abort', 2), null)).toBe('Aborted: 2 recordings');
		expect(endWords(end('write_failed', 3), 'r')).toBe('Ended: 3 recordings in r');
	});

	it('say what a Recorder run saved, how far the disk is behind, and what a write fault left', () => {
		expect(savedWords(REC)).toBe('Saved rec_0002: 1.50 s, no gaps');
		expect(savedWords({ ...REC, gap_free: false, gaps: [{ at_frame: 4096, dropped_frames: 2048 }] })).toBe('Saved rec_0002: 1.50 s, 1 gap, 2048 frames missing');
		expect(backlogWords(0.3)).toBe('Writing 0.3 s behind the inputs');
		expect([backlogWords(0.12), backlogWords(null)]).toEqual([null, null]);
		expect(faultWords('the disk refused the write (no space left on the device).', { name: 'rec_0003', seconds: 48.21 })).toBe('Could not write: the disk refused the write (no space left on the device). rec_0003 holds 48.2 s.');
		expect(faultWords('an I/O error', null)).toBe('Could not write: an I/O error.');
	});

	it('put a Recorder mark and a recording in words, at station time', () => {
		expect(markWords({ kind: 'limit', row: 1, axis: 'accel', value: 0.5, limit: 0.4, station_frame: 6528, recording: 'rec_0001' }, 12800)).toBe('Row 1 passed its acceleration limit of 0.4 g (0.5 g) at 0.510 s, in rec_0001');
		expect(markWords({ kind: 'overload', row: 2, value: 0.4512, level: 0.45, station_frame: 100, recording: null }, null, 'g')).toBe('Row 2 reached 0.4512 g, over 0.9 of its range (0.45 g) at station frame 100');
		const facts = recordingFacts({ ...REC, fs_hz: 8533.33, requested_fs_hz: 8192, pre_frames: 3840 });
		expect(facts.find((f) => f.label === 'Rate')?.value).toBe('8533.33 Hz (asked 8192 Hz)');
		expect(facts.find((f) => f.label === 'Pre-window')?.value).toBe('3840 of 6400 frames');
		expect(facts.find((f) => f.label === 'Size')?.value).toBe('300 kiB');
		expect(recordingFacts({ ...REC, fault: 'no space' }).at(-1)).toEqual({ label: 'Fault', value: 'no space' });
	});

	it('format the Recorder numbers in their units', () => {
		expect([bytesWords(1073741824), bytesWords(204800), bytesWords(512), bytesWords(null)]).toEqual(['1 GiB', '200 kiB', '512 bytes', '-']);
		expect([spanWords(40), spanWords(720), spanWords(519045), spanWords(null)]).toEqual(['40 s', '12 min', '144 h 10 min', '-']);
	});

	it("name a Recorder refusal's state, mode and space in the Recorder's own words", () => {
		expect(recorderStateFact({ state: 'configured', phase: 'idle' })).toBe('State: Ready to Start, Idle');
		expect(recorderStateFact({ state: 'running', phase: 'recording' })).toBe(`State: Running, ${RECORDER_PHASE_WORDS.recording}`);
		expect(recorderStateFact(undefined)).toBeNull();
		expect(recorderModeFact({ mode: 'level' })).toBe('Trigger: Level');
		expect(recorderSpaceFacts({ free_bytes: 536870912, floor_bytes: 1073741824 })).toEqual(['Free: 512 MiB', 'Floor: 1 GiB']);
		expect(Object.keys(RECORDER_STATE_WORDS)).toEqual(['unconfigured', 'configured', 'starting', 'running', 'stopping', 'finished']);
	});
});
