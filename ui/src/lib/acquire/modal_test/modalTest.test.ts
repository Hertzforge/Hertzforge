/**
 * The Modal Test's pure parts: the Measure screen's keys and their guard, the counter, tiles and
 * flags read from the service's counts, the verbs the screen offers, the draft and the
 * configuration it sends, where a service finding lands, and the traces by role.
 */
import { describe, expect, it } from 'vitest';
import type { ModalCounts, ModalLatest, ModalTestLive, ModalTestRun, ModalTile } from '$lib/protocol/modal_test';
import type { ChannelRow } from '$lib/protocol/types';
import { MODAL_KEY_WORDS, modalKeyAction, takesTyping, type KeyLike } from './modalTestKeys';
import { defaultModalSetup, fromModalConfig, localErrors, setModalMode, modalItemKey, modalRows, modalSectionOf, modalSections, parsePoints, pointsText, toModalConfig } from './modalTestSetup';
import { loadModalSetup, saveModalSetup } from './modalTestStore.svelte';
import { modalBarActions, modalMeasureVerbs, modalPrimary, type ModalVerbContext } from './modalTestVerbs';
import { exponentialWeights, forceNotes, forceSpectrumView, forceWindowWeights, hitViews, referenceSpectraView, secondImpactWords, tracesOf } from './modalTestViews';
import { counterWords, MODAL_OUTCOME_HELP, MODAL_OUTCOME_WORDS, modalStateHint, modalStateWords, num, pct, refusalWords, reportLines, targetFlag, tileWords, verdictWords } from './modalTestWords';

const el = (tagName: string, extra: Record<string, unknown> = {}) => ({ tagName, getAttribute: (k: string) => (extra[k] as string) ?? null, ...extra }) as unknown as EventTarget;
const key = (k: string, more: Partial<KeyLike> = {}): KeyLike => ({ key: k, target: el('BODY'), ...more });

const counts = (accepted: number, rejected: number, asked = 5): ModalCounts => ({
	unit: 'hits',
	point: { index: 0, accepted, rejected, asked },
	points: [{ index: 0, accepted, rejected, taken: accepted + rejected, measured: accepted >= asked }],
	totals: { accepted: 17, taken: 20, points_measured: 3, points_planned: 8 }
});

const tile = (over: Partial<ModalTile> = {}): ModalTile => ({ n: 1, seq: 1, status: 'accepted', reason: null, overload: false, double_hit: false, start_frame: 0, ...over });

const ctx = (over: Partial<ModalVerbContext> = {}, run: Partial<ModalTestRun> | null = {}): ModalVerbContext => ({
	connected: true,
	controller: true,
	setup: 'applied',
	setupErrors: 0,
	busy: null,
	pending: null,
	run: run === null ? null : ({ state: 'idle', mode: 'impact', point: 0, counts: counts(0, 0), latest: null, ...run } as ModalVerbContext['run']),
	...over
});

describe('modal test keys', () => {
	it('modal test keys: each key asks for its action', () => {
		const want: [string, string][] = [
			['Enter', 'accept'],
			['a', 'accept'],
			['A', 'accept'],
			['Backspace', 'reject'],
			['r', 'reject'],
			['R', 'reject'],
			['u', 'undo'],
			['U', 'undo'],
			['ArrowLeft', 'previous'],
			['ArrowRight', 'next'],
			[' ', 'arm']
		];
		for (const [k, a] of want) expect([k, modalKeyAction(key(k))]).toEqual([k, a]);
		expect(modalKeyAction(key('x'))).toBeNull();
		expect(modalKeyAction(key('ArrowUp'))).toBeNull();
		expect(Object.keys(MODAL_KEY_WORDS).sort()).toEqual(['accept', 'arm', 'next', 'previous', 'reject', 'undo']);
	});

	it('modal test keys: never fire while typing, with a modifier held, or on repeats', () => {
		for (const t of [el('INPUT', { type: 'text' }), el('INPUT', { type: 'number' }), el('TEXTAREA'), el('SELECT'), el('DIV', { isContentEditable: true })]) {
			expect(takesTyping(t)).toBe(true);
			for (const k of ['Enter', 'a', 'Backspace', 'r', 'u', 'ArrowLeft', 'ArrowRight', ' ']) expect(modalKeyAction(key(k, { target: t }))).toBeNull();
		}
		expect(takesTyping(el('INPUT', { type: 'checkbox' }))).toBe(false);
		expect(takesTyping(el('BUTTON'))).toBe(false);
		expect(modalKeyAction(key('a', { metaKey: true }))).toBeNull();
		expect(modalKeyAction(key('r', { ctrlKey: true }))).toBeNull();
		expect(modalKeyAction(key('u', { altKey: true }))).toBeNull();
		expect(modalKeyAction(key('a', { repeat: true }))).toBeNull();
	});

	it('modal test keys: Enter and Space on a focused control are the control own, the letters still act', () => {
		const b = el('BUTTON');
		expect(modalKeyAction(key('Enter', { target: b }))).toBeNull();
		expect(modalKeyAction(key(' ', { target: b }))).toBeNull();
		expect(modalKeyAction(key('a', { target: b }))).toBe('accept');
		expect(modalKeyAction(key('ArrowRight', { target: el('INPUT', { type: 'checkbox' }) }))).toBe('next');
	});

	it('modal test keys: accept and reject act on the latest hit in both accept modes', () => {
		// The verbs do not depend on the accept mode: in automatic mode the keys override the verdict.
		const latest = (status: ModalTile['status']) => ({ ...tile({ status }), point: 0, report: null }) as ModalLatest;
		const auto = modalMeasureVerbs(ctx({}, { state: 'armed', latest: latest('accepted'), counts: counts(1, 0) }));
		expect([auto.accept.enabled, auto.reject.enabled, auto.undo.enabled]).toEqual([false, true, true]);
		const manual = modalMeasureVerbs(ctx({}, { state: 'captured', latest: latest('pending') }));
		expect([manual.accept.enabled, manual.reject.enabled]).toEqual([true, true]);
		expect(manual.arm.reason).toBe('Accept or reject the hit first.');
	});
});

describe('modal test counter, tiles and flags', () => {
	it('modal test counter: every number is the service count', () => {
		expect(counterWords(counts(3, 1))).toEqual({ point: '3 of 5', rejected: '1 rejected', dataset: '17 accepted hits in the dataset', points: '3 of 8 points measured' });
		expect(counterWords({ ...counts(2, 0, 10), unit: 'frames' })?.dataset).toBe('17 accepted frames in the dataset');
		expect(counterWords(null)).toBeNull();
	});

	it('modal test tiles: status, reason and every flag in words', () => {
		const t = tileWords(tile({ status: 'rejected', reason: 'double_hit', double_hit: true, target_verdict: 'high', target_diff_pct: 26.4, tip_warning: true }));
		expect([t.status, t.tone, t.reason]).toEqual(['Rejected', 'fail', 'Double Hit']);
		expect(t.flags.map((f) => [f.word, f.kind])).toEqual([
			['Double Hit', 'double'],
			['High +26%', 'high'],
			['Tip', 'tip']
		]);
		expect(tileWords(tile({ status: 'pending' })).status).toBe('Waiting');
		expect(tileWords(tile({ status: 'undone' })).tone).toBe('muted');
		expect(tileWords(tile({ overload: true, force_window_cut: true })).flags.map((f) => f.word)).toEqual(['Overload', 'Window Cuts Pulse']);
	});

	it('modal test flags: the target flag reads the service percent and never rejects', () => {
		expect(targetFlag('high', 26)).toEqual({ word: 'High +26%', tone: 'high' });
		expect(targetFlag('low', -8)).toEqual({ word: 'Low −8%', tone: 'low' });
		expect(targetFlag('on_target', 3)).toEqual({ word: 'On Target', tone: 'on' });
		expect(targetFlag(null, null)).toBeNull();
		// a High flag on an accepted hit stays accepted
		const t = tileWords(tile({ target_verdict: 'high', target_diff_pct: 40 }));
		expect([t.status, t.reason]).toEqual(['Accepted', null]);
		expect([pct(0), pct(null), num(1234.567, 'N'), num(0.0001)]).toEqual(['0%', '-', '1235 N', '1.00e-4']);
	});

	it('modal test verdict and report lines in words', () => {
		const l = (over: Partial<ModalLatest>) => ({ ...tile(), point: 0, report: null, n: 3, ...over }) as ModalLatest;
		expect(verdictWords(l({ status: 'accepted' }))).toBe('Hit 3 Accepted');
		expect(verdictWords(l({ status: 'rejected', reason: 'overload' }))).toBe('Hit 3 Rejected: Overload');
		expect(verdictWords(l({ status: 'pending' }))).toBe('Hit 3 Captured: Accept or Reject');
		expect(verdictWords(l({ status: 'accepted' }), 'frames')).toBe('Frame 3 Accepted');
		const lines = reportLines({ peak: 210, double_hit: { ratio: 0.12, peak: 25.2, frame: 40 }, overloaded: [0, 2], tip_drop_db: 24, tip_warning: true, no_response: [false, true], force_window_cut: false } as never, 'N');
		expect(lines).toEqual([
			'Peak force 210 N.',
			'Double hit: a second impact of 25.2 N, 12% of the peak.',
			'Overload on the force, response 2.',
			'Force spectrum falls 24 dB across the band, more than the tip limit.',
			'A response stayed inside its noise: check the sensor and its cable.'
		]);
	});

	it('modal test report: a response not decayed names its row and how loud the block ends', () => {
		const base = { peak: 18, double_hit: null, overloaded: [], tip_drop_db: 6, tip_warning: false, no_response: [false], force_window_cut: null };
		const quiet = reportLines({ ...base, decay: { row: 3, ratio: 0.0004, warning: false } } as never, 'N');
		expect(quiet).toHaveLength(2);
		const loud = reportLines({ ...base, decay: { row: 3, ratio: 0.083, warning: true } } as never, 'N');
		expect(loud[2]).toBe('Row 3 has not died away by the end of the block: it ends at 8.3% of its loudest. A longer block or an exponential window helps.');
	});
});

describe('modal test verbs', () => {
	it('modal test bar: apply, start and reset by state; stop and abort need no control', () => {
		expect(modalPrimary(ctx({}, null))).toBe('apply');
		expect(modalPrimary(ctx({}, { state: 'configured' }))).toBe('start');
		expect(modalPrimary(ctx({ setup: 'changed' }, { state: 'configured' }))).toBe('apply');
		expect(modalPrimary(ctx({}, { state: 'finished' }))).toBe('reset');
		const live = modalBarActions(ctx({ controller: false }, { state: 'armed' }));
		expect([live.stop.enabled, live.abort.enabled, live.apply.visible, live.start.visible]).toEqual([true, true, false, false]);
		expect(modalBarActions(ctx({ setupErrors: 2 }, null)).apply.reason).toBe('Fix the 2 errors in the setup first.');
	});

	it('modal test measure verbs: impact arms, shaker drives, an observer can only end', () => {
		const idle = modalMeasureVerbs(ctx());
		expect([idle.arm.enabled, idle.disarm.visible, idle.drive_start.visible, idle.accept.reason]).toEqual([true, false, false, 'No hit at this point yet.']);
		const armed = modalMeasureVerbs(ctx({}, { state: 'armed' }));
		expect([armed.arm.visible, armed.disarm.enabled]).toEqual([false, true]);
		const arming = modalMeasureVerbs(ctx({}, { state: 'arming' }));
		expect([arming.arm.visible, arming.disarm.enabled]).toEqual([false, true]);
		const done = modalMeasureVerbs(ctx({}, { state: 'point_done', counts: counts(5, 0) }));
		expect([done.arm.enabled, done.next.enabled, done.undo.enabled]).toEqual([false, true, true]);
		const shaking = modalMeasureVerbs(ctx({}, { state: 'measuring', mode: 'shaker' }));
		expect([shaking.drive_stop.enabled, shaking.next.reason, shaking.arm.visible, shaking.accept.visible]).toEqual([true, 'Stop driving first.', false, false]);
		const observer = modalMeasureVerbs(ctx({ controller: false }, { state: 'armed' }));
		expect([observer.disarm.reason, observer.next.reason, observer.end.enabled]).toEqual(['Take control to disarm.', 'Take control to move to the next point.', true]);
	});
});

const row = (index: number, analysis_role: string | null, role = 'input', node: number | null = index, direction = 'Z+') =>
	({ index, role, enabled: true, description: `R${index}`, range_max: 10, units: 'g', analysis_role, node, direction }) as unknown as ChannelRow;

describe('modal test setup', () => {
	it('modal test points: typed or pasted, each word that cannot be read named', () => {
		expect(parsePoints('2\n3, 4 Z+\t5\tY-; 6x+').points).toEqual([{ node: 2 }, { node: 3 }, { node: 4, direction: 'Z+' }, { node: 5, direction: 'Y-' }, { node: 6, direction: 'X+' }]);
		expect(parsePoints('2 q 3').problems).toEqual(['"q" is not a node number, or a node and a direction such as 4 Z+.']);
		expect(pointsText([{ node: 2 }, { node: 4, direction: 'Z+' }])).toBe('2\n4 Z+');
	});

	it('modal test config: the mode own settings only, roving when on, and opens back the same', () => {
		const d = defaultModalSetup();
		const c = toModalConfig(d);
		expect([c.mode, c.impact?.trigger_level_pct, c.impact?.target, c.shaker, c.roving, c.band]).toEqual(['impact', 10, null, undefined, undefined, undefined]);
		d.roving = { ...d.roving, on: true, channels: [2], text: '2\n3' };
		d.impact.target = { on: true, peak: 200, tolerance_pct: 15 };
		const c2 = toModalConfig(d);
		expect(c2.roving).toEqual({ channels: [2], points: [{ node: 2 }, { node: 3 }] });
		expect(c2.impact?.target).toEqual({ peak: 200, tolerance_pct: 15 });
		expect(toModalConfig(fromModalConfig(c2))).toEqual(c2);
		d.mode = 'shaker';
		d.shaker.drives = [{ row: 5, level_vrms: 0.2 }];
		const c3 = toModalConfig(d);
		expect([c3.impact, c3.shaker?.drives, c3.shaker?.source]).toEqual([undefined, [{ row: 5, level_vrms: 0.2 }], 'burst_random']);
	});

	it('modal test editor: what is missing before anything is sent', () => {
		const d = defaultModalSetup();
		expect(localErrors(d)).toEqual({});
		d.roving.on = true;
		expect(Object.keys(localErrors(d)).sort()).toEqual(['roving.channels', 'roving.points']);
		d.roving.on = false;
		d.mode = 'shaker';
		expect(localErrors(d)).toEqual({ 'shaker.drives': ['Choose at least one drive row.'] });
	});

	it('modal test findings land in their section, the plan refusals in DOFs', () => {
		expect(modalItemKey({ reason: 'no_reference', channel: 0 })).toBe('dofs');
		expect(modalItemKey({ reason: 'reference_count' })).toBe('dofs');
		expect(modalItemKey({ reason: 'out_of_range', field: 'config.impact.force_window' })).toBe('impact.force_window');
		expect(modalItemKey({ reason: 'source_refused', field: 'config.shaker.source' })).toBe('shaker.source');
		expect(modalItemKey({ reason: 'out_of_range', field: 'config.shaker.drives[1].level_vrms' })).toBe('shaker.drives.1.level_vrms');
		expect(modalSectionOf('impact.force_window')).toBe('excitation');
		expect(modalSectionOf('roving.points')).toBe('roving');
		expect(modalSectionOf('capture.run_name')).toBe('capture');
		expect(modalSectionOf('block')).toBe('acquisition');
		expect(modalSections('shaker').find((s) => s.id === 'excitation')?.label).toBe('Shaker');
	});

	it('modal test rows: references, responses and drives from the table', () => {
		const r = modalRows([row(0, 'reference'), row(1, 'response'), row(2, null), row(3, 'response'), row(4, null, 'drive')]);
		expect([r.references.map((x) => x.index), r.responses.map((x) => x.index), r.unassigned.map((x) => x.index), r.drives.map((x) => x.index)]).toEqual([[0], [1, 3], [2], [4]]);
	});

	it('modal test decay threshold: the service default unless given, and kept by the setup file and the draft', () => {
		const d = defaultModalSetup();
		expect(Number.isNaN(d.decay_warn_pct)).toBe(true);
		expect('decay_warn_pct' in toModalConfig(d)).toBe(false);
		expect(Number.isNaN(loadModalSetup(saveModalSetup(d)).decay_warn_pct)).toBe(true);
		d.decay_warn_pct = 0.5;
		const c = toModalConfig(d);
		expect(c.decay_warn_pct).toBe(0.5);
		expect(fromModalConfig(c).decay_warn_pct).toBe(0.5);
		expect(toModalConfig(fromModalConfig(c))).toEqual(c);
		expect(loadModalSetup(saveModalSetup(d)).decay_warn_pct).toBe(0.5);
		// a draft saved before the field reads back with the default
		const old = JSON.parse(saveModalSetup(d));
		delete old.decay_warn_pct;
		expect(Number.isNaN(loadModalSetup(JSON.stringify(old)).decay_warn_pct)).toBe(true);
	});

	it('modal test mode change: a plan that roves the hammer is cleared for shaker mode, with a note', () => {
		const rows = [row(1, 'reference'), row(3, 'response'), row(4, 'response')];
		const hammer = defaultModalSetup();
		hammer.roving = { ...hammer.roving, on: true, channels: [1], how: 'fill', fill: { start: 1, step: 1, count: 3 } };
		const note = setModalMode(hammer, 'shaker', rows);
		expect(hammer.mode).toBe('shaker');
		expect(note).toBe("The roving plan was cleared: it moved row 1, a reference, and a shaker's force row does not rove. In Roving, choose the responses that rove and their points.");
		expect(hammer.roving).toEqual(defaultModalSetup().roving);
		expect(toModalConfig(hammer).roving).toBeUndefined();
		// a plan that roves the responses stays, and so does any plan on the way back to impact
		const accel = defaultModalSetup();
		accel.roving = { ...accel.roving, on: true, channels: [3, 4], text: '5\n6' };
		expect(setModalMode(accel, 'shaker', rows)).toBeNull();
		expect(accel.roving.channels).toEqual([3, 4]);
		expect(setModalMode(accel, 'impact', rows)).toBeNull();
		expect(accel.roving.channels).toEqual([3, 4]);
		expect(setModalMode(accel, 'impact', rows)).toBeNull();
	});
});

describe('modal test traces', () => {
	const dofs = { references: [{ index: 2, node: 1, direction: 'Z+', unit: 'N' }], responses: [{ index: 0, node: 3, direction: 'Z+', unit: 'g' }, { index: 5, node: 4, direction: 'Z+', unit: 'g' }] };
	it('modal test traces: by the roles the service sends', () => {
		const ch = [{ values: [1], role: 'response' as const }, { values: [2], role: 'reference' as const }, { values: [3], role: 'drive' as const }];
		expect(tracesOf(ch, 'reference', dofs).map((c) => c.values[0])).toEqual([2]);
		expect(tracesOf(ch, 'drive', dofs).map((c) => c.values[0])).toEqual([3]);
	});
	it('modal test traces: without roles, in table order with drives after', () => {
		// table order: row 0 (response), row 2 (reference), row 5 (response), then a drive
		const ch = [{ values: [10] }, { values: [20] }, { values: [50] }, { values: [99] }];
		expect(tracesOf(ch, 'reference', dofs).map((c) => c.values[0])).toEqual([20]);
		expect(tracesOf(ch, 'response', dofs).map((c) => c.values[0])).toEqual([10, 50]);
		expect(tracesOf(ch, 'drive', dofs).map((c) => c.values[0])).toEqual([99]);
	});
});

describe('modal test after review', () => {
	it('modal test states: Arming first, Waiting for Hit only once the service says armed', () => {
		expect(modalStateWords('arming', 'impact')).toBe('Arming');
		expect(modalStateWords('armed', 'impact')).toBe('Waiting for Hit');
		expect(modalStateHint('arming', 'impact', 'auto')).toMatch(/pre-trigger/);
	});

	it('modal test clear point: its own verb, not while a hit waits or the shakers drive, and only with something to clear', () => {
		expect(modalMeasureVerbs(ctx({}, { state: 'idle', counts: counts(2, 0) })).clear.enabled).toBe(true);
		expect(modalMeasureVerbs(ctx({}, { state: 'idle', counts: counts(0, 0) })).clear.reason).toBe('Nothing accepted at this point to clear.');
		expect(modalMeasureVerbs(ctx({}, { state: 'captured', counts: counts(2, 0) })).clear.reason).toBe('Accept or reject the hit first.');
		expect(modalMeasureVerbs(ctx({}, { state: 'measuring', mode: 'shaker', counts: counts(2, 0) })).clear.reason).toBe('Stop driving first.');
	});

	it('modal test outcomes and the ending refusal in words for the person at the structure', () => {
		for (const o of ['run_panicked', 'run_not_started', 'source_failed'] as const) {
			expect(MODAL_OUTCOME_WORDS[o]).toBeTruthy();
			expect(MODAL_OUTCOME_HELP[o]).toMatch(/\.$/);
		}
		expect(refusalWords({ reason: 'ending', message: 'refused: ending' })).toBe('The test is ending: nothing new starts after End Test or Abort.');
		expect(refusalWords({ reason: 'busy', message: 'A hit waits.' })).toBe('A hit waits.');
	});

	it('modal test second impact: where and how large, from the service report', () => {
		expect(secondImpactWords({ frame: 512, peak: 7.5, ratio: 0.41 }, 1024, 'lbf')).toBe('Second impact at 0.5 s into the block, 7.5 lbf: 41% of the peak.');
		expect(secondImpactWords(null, 1024, 'lbf')).toBeNull();
	});

	it('modal test windows: drawn from the counts and tau the hit was taken with', () => {
		const w = forceWindowWeights({ pre_frames: 2, width_frames: 3, fall_frames: 2 }, 8);
		expect(w).toEqual([1, 1, 1, 1, 1, 1, 0.5, 0]);
		const e = exponentialWeights(0.5, 2, 3);
		expect(e[0]).toBe(1);
		expect(e[2]).toBeCloseTo(Math.exp(-2), 12);
	});

	it('modal test live hit: the own block, the windows over it, the second impact and the force spectrum', () => {
		const dofs = { references: [{ index: 1, node: 1, direction: 'Z+', unit: 'lbf' }], responses: [{ index: 2, node: 4, direction: 'Z+', unit: 'g' }] };
		const live = {
			run_id: 'r',
			state: 'armed',
			point: 0,
			time: { rate_hz: 1024, channels: [] },
			frf: { frequency_hz: [], averages: 0, pairs: [], coherence: [] },
			hit: {
				point: 0, n: 2, seq: 4, start_frame: 1536, start_s: 1.5, samples: 4, decimation: 1, step_s: 1 / 1024,
				channels: [{ index: 1, role: 'reference', unit: 'lbf', values: [0, 9, 1, 0] }, { index: 2, role: 'response', unit: 'g', values: [0, 1, -1, 0] }],
				windows: { force: { pre_frames: 1, width_frames: 2, fall_frames: 1 }, exponential: { tau_s: 0.125, end: 0.0625 } },
				force_spectrum: { frequency_hz: [0, 2, 4, 6], unit: 'lbf^2/Hz', values: [1, 1, 0.5, 0.1], tip_drop_db: 9.5 }
			}
		} as unknown as ModalTestLive;
		const latest = { ...tile(), seq: 4, point: 0, n: 2, report: { double_hit: { frame: 2, peak: 3, ratio: 0.33 } } } as unknown as ModalLatest;
		const v = hitViews(live, latest, dofs, 0);
		expect(v.force?.y.unit).toBe('lbf');
		expect(v.force?.series[0].x[1]).toBeCloseTo(1 / 1024, 12);
		expect(v.forceRegions.map((r) => r.level)).toEqual([0, 1]);
		expect(v.responseRegions).toHaveLength(1);
		expect(v.second).toBeCloseTo(2 / 1024, 12);
		expect(forceSpectrumView(live)?.readouts?.[0].text).toBe('9.5 dB');
		// a soft tip is said in the header line beside the roll-off, never over the plot's axes
		expect(forceSpectrumView(live, true)?.readouts).toEqual([{ label: 'Tip roll-off', text: '9.5 dB, Soft Tip', status: 'warn' }]);
		expect(hitViews({ ...live, hit: null } as ModalTestLive, latest, dofs, 0).force).toBeNull();
		const shaker = { ...live, hit: undefined, references: { frequency_hz: [1, 2], averages: 3, autospectra: [{ index: 1, unit: 'lbf^2/Hz', values: [1, 2] }] } } as unknown as ModalTestLive;
		expect(referenceSpectraView(shaker, dofs)?.series[0].name).toBe('1Z+ (row 1)');
	});

	it('modal test force labels: inside the drawing area beside the second impact, near its top', () => {
		expect(forceNotes(null, 1, false)).toEqual([]);
		const [early] = forceNotes(0.08, 1, false);
		expect(early).toEqual({ id: 'double-hit', text: 'Double Hit', fx: 0.1, fy: 0.03 });
		// late in the block the label sits on the marker's left, so it stays inside the plot
		expect(forceNotes(0.9, 1, false)[0].fx).toBeCloseTo(0.6, 12);
		const both = forceNotes(0.08, 1, true);
		expect(both.map((n) => [n.id, n.fy])).toEqual([['double-hit', 0.03], ['overload', 0.2]]);
		for (const n of [...both, ...forceNotes(1.5, 1, true)]) {
			expect(n.fx).toBeGreaterThanOrEqual(0);
			expect(n.fx).toBeLessThanOrEqual(0.7);
			expect(n.fy).toBeLessThan(0.5);
		}
	});
});
