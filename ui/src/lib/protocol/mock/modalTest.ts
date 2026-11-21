/**
 * Mock Modal Test (schema/PROTOCOL.md, "Modal Test"), on the mock control service's run slot and
 * controller seat.
 *
 * The references and responses are the channel table's rows by their analysis role, each with its
 * node and direction; the roving rows take each point's node in turn. Impact mode: an armed test
 * is arming for a moment (the pre-trigger filling), then takes a hit about every 1.5 s of station time; each is checked (an overload, a second impact over
 * the double-hit limit, the tip drop, a force against the target) and, in automatic mode, judged
 * then and there (an overload or a double hit is rejected, any other accepted), or in manual mode
 * held for the operator. `modal_test.sim_hit` makes the next hit clean, double or overloaded and
 * lands it at once when the test is armed. Shaker mode: a point drives frame by frame to its
 * averages and stops. Every count is kept here after every take, accept, reject and undo, and sent
 * with each answer and each state.
 *
 * It refuses what the service refuses, by the same reasons: content by item inside
 * `config_invalid` naming the field, a verb in a state it does not apply to by `wrong_state`, an
 * impact verb in shaker mode by `not_in_mode`, an observer by `not_controller` for everything but
 * end, abort and the reads, and another live run by `run_active`. It sends `modal_test.live` ten
 * times a second, as the service does: each trace's row, role and unit, the latest hit's own block
 * with its windows and force spectrum (impact), the reference autospectra (shaker). Every value is
 * synthetic, made only so the shell has something to draw; nothing here is the service's analysis.
 */

import type { ChannelRow, DataSource, ItemReason, ReportItem, StationRef } from '../types';
import type {
	ModalCheckImpact,
	ModalCheckShaker,
	ModalCounts,
	ModalHitKind,
	ModalLatest,
	ModalLiveFrame,
	ModalOutcome,
	ModalPoint,
	ModalRowDof,
	ModalRunState,
	ModalTestCheck,
	ModalTestConfig,
	ModalTestLive,
	ModalTestLogEntry,
	ModalTestRun,
	ModalTestState,
	ModalTile,
	ModalTrace
} from '../modal_test';
import type { ShockMockHost } from './shock';
import { MockError, type MockConnection, type MockMethod } from './socket';

const TICK_MS = 50;
const LIVE_MS = 100;
/** Station seconds between hits while armed. */
const HIT_EVERY_S = 1.5;
/** Station time an arm takes to fill the pre-trigger before the trigger is live. */
const ARMING_S = 0.3;
/** Station seconds per shaker frame. */
const FRAME_EVERY_S = 0.4;
const SHOWN = 1024;
/** The frames the mock station's inputs lag its drives by. */
const MOCK_INPUT_DELAY = 48;
const MODES_HZ = [42, 118, 236];
const ZETA = [0.012, 0.009, 0.015];

export interface ModalTestMock {
	isLive(): boolean;
	liveId(): string | null;
	detach(conn: MockConnection): void;
	seatExpired(): void;
	methods: Record<string, MockMethod>;
}

function refuse(reason: string, message: string, detail?: Record<string, unknown>): never {
	throw new MockError('refused', message, reason as never, detail);
}

function bad(param: string, message: string): never {
	throw new MockError('bad_request', message, 'bad_params', { param });
}

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);
const round = (v: number, d = 3) => Math.round(v * 10 ** d) / 10 ** d;

function rng(seed: number) {
	let s = seed >>> 0 || 1;
	return () => {
		s ^= s << 13;
		s ^= s >>> 17;
		s ^= s << 5;
		return (s >>> 0) / 4294967296;
	};
}

interface Hit extends ModalTile {
	point: number;
	report: ModalLatest['report'];
	kind: ModalHitKind;
}

interface Run {
	id: string;
	folder: string;
	config: ModalTestConfig;
	check: ModalTestCheck;
	state: Exclude<ModalRunState, 'none'>;
	source: DataSource;
	station: StationRef;
	point: number;
	plan: { references: ModalRowDof[]; responses: ModalRowDof[] }[];
	hits: Hit[];
	seq: number;
	nextKind: ModalHitKind | null;
	/** Station time since the test was armed (or the drive started) at this point. */
	clock: number;
	t: number;
	target: { peak: number; tolerance_pct: number } | null;
	end: ModalTestRun['end'];
	drivePhase: ModalTestRun['drive_phase'];
	log: ModalTestLogEntry[];
	rand: () => number;
	/** An end or an abort was asked: nothing new starts (`ending`). */
	ending: boolean;
}

const ITEM = (reason: string, field: string, message: string): ReportItem => ({ reason: reason as ItemReason, field, message });

export function createModalTestMock(host: ShockMockHost): ModalTestMock {
	let run: Run | null = null;
	let runCount = 0;
	let timer: ReturnType<typeof setInterval> | null = null;
	let liveAt = 0;
	let lastKey = '';

	const isLive = () => !!run && run.state !== 'configured' && run.state !== 'finished';

	function dof(r: ChannelRow, roves = false): ModalRowDof {
		return { index: r.index, node: r.node ?? null, direction: r.direction || null, unit: r.units, range: r.range_max, roves };
	}

	// ---- the check ----------------------------------------------------------------------------
	function checkOf(config: ModalTestConfig): { errors: ReportItem[]; warnings: ReportItem[]; check: ModalTestCheck | null; plan: Run['plan'] } {
		const errors: ReportItem[] = [];
		const warnings: ReportItem[] = [];
		const rows = host.rows().filter((r) => r.enabled);
		const refs = rows.filter((r) => r.role !== 'drive' && r.analysis_role === 'reference');
		const resps = rows.filter((r) => r.role !== 'drive' && r.analysis_role === 'response');
		const mode = config.mode;
		if (mode !== 'impact' && mode !== 'shaker') bad('config.mode', 'The mode is impact or shaker.');
		if (!(config.fs_hz >= 256 && config.fs_hz <= 204800)) errors.push(ITEM('out_of_range', 'config.fs_hz', 'The sample rate is 256 Hz to 204 800 Hz.'));
		const block = config.block ?? 2048;
		if (!(block >= 256 && block <= 65536 && (block & (block - 1)) === 0)) errors.push(ITEM('out_of_range', 'config.block', 'The block is a power of two, 256 to 65 536 samples.'));
		const averages = config.averages ?? (mode === 'impact' ? 5 : 30);
		if (!(averages >= 1 && averages <= 10000)) errors.push(ITEM('out_of_range', 'config.averages', 'The averages are 1 to 10 000.'));
		const onRow = (reason: string, channel: number | null, message: string): ReportItem => ({ reason: reason as ItemReason, channel, message });
		if (!refs.length) errors.push(onRow('no_reference', null, "No enabled input row has the analysis role reference: give the hammer's or each shaker's force row that role."));
		if (!resps.length) errors.push(onRow('no_response', null, 'No enabled input row has the analysis role response.'));
		if (mode === 'impact' && refs.length > 1) errors.push(onRow('reference_count', null, 'Impact mode takes one reference, the hammer row.'));
		for (const r of [...refs, ...resps]) if (r.node === null || r.node === undefined || !r.direction) errors.push(onRow('dof_missing', r.index, `Row ${r.index} has no node and direction: give it its DOF.`));
		const fs = config.fs_hz;
		const band = { lo_hz: config.band?.lo_hz ?? 0, hi_hz: config.band?.hi_hz ?? round(fs / 2.56, 3) };
		if (band.hi_hz > fs / 2) errors.push(ITEM('out_of_range', 'config.band.hi_hz', `The band ends at most at half the sample rate, ${fs / 2} Hz.`));
		if (band.lo_hz >= band.hi_hz) errors.push(ITEM('out_of_range', 'config.band.lo_hz', 'The band starts below where it ends.'));
		const df = fs / block;
		// The plan: one measurement, or one per roving point.
		// A roving row is an enabled reference or response; in impact mode the hammer may rove (a
		// roving hammer survey), in shaker mode a force row does not.
		const rovers = new Set(config.roving?.channels ?? []);
		if (config.roving && !rovers.size) errors.push(ITEM('roving_invalid', 'config.roving.channels', 'A roving plan names at least one channel that roves.'));
		for (const c of rovers) {
			const isRef = refs.some((r) => r.index === c);
			if (!isRef && !resps.some((r) => r.index === c)) errors.push({ ...ITEM('roving_invalid', 'config.roving.channels', `Row ${c} roves but is not an enabled reference or response row.`), channel: c });
			else if (isRef && mode === 'shaker') errors.push(ITEM('roving_invalid', 'config.roving.channels', "A shaker's force row does not rove: rove the responses."));
		}
		let points: ModalPoint[] = [];
		const p = config.roving?.points;
		if (rovers.size && p) {
			if (Array.isArray(p)) points = p;
			else {
				const start = p.start ?? 1;
				const step = p.step ?? 1;
				points = Array.from({ length: Math.max(0, p.count) }, (_, k) => ({ node: start + k * step }));
			}
			if (!points.length) errors.push(ITEM('roving_invalid', 'config.roving.points', 'A roving plan names at least one point.'));
			if (rovers.size > 1 && points.some((q) => q.direction)) errors.push(ITEM('roving_invalid', 'config.roving.points', 'A point gives a direction only when exactly one channel roves; with several, each keeps its own direction.'));
		} else if (!rovers.size) warnings.push(ITEM('no_roving', 'config.roving', 'No row roves: the test measures one point.'));
		const at = (r: ChannelRow, pt: ModalPoint | null): ModalRowDof => (pt && rovers.has(r.index) ? { ...dof(r, true), node: pt.node, direction: pt.direction ?? r.direction ?? null } : dof(r));
		const plan: Run['plan'] = (points.length ? points : [null]).map((pt) => ({
			references: refs.map((r) => at(r, pt)),
			responses: resps.map((r) => at(r, pt))
		}));
		if (averages === 1) warnings.push(ITEM('one_average', 'config.averages', 'One average gives no coherence: every line reads 1.'));
		let impact: ModalCheckImpact | undefined;
		let shaker: ModalCheckShaker | undefined;
		if (mode === 'impact') {
			const c = config.impact ?? {};
			const hammer = refs[0];
			const pre = Math.round(((c.pretrigger_pct ?? 5) / 100) * block);
			if (pre === 0) warnings.push(ITEM('no_pretrigger', 'config.impact.pretrigger_pct', 'No pretrigger: the start of the pulse may be lost.'));
			const fwOn = c.force_window?.on ?? true;
			const width = Math.round(((c.force_window?.width_pct ?? 10) / 100) * block);
			const fw = fwOn ? { pre_frames: pre, width_frames: width, fall_frames: Math.max(1, Math.round(((c.force_window?.fall_pct ?? 10) / 100) * width)) } : null;
			const ewOn = c.exponential_window?.on ?? false;
			const end = (c.exponential_window?.end_pct ?? 10) / 100;
			const T = block / fs;
			const tau = T / -Math.log(end);
			if (ewOn) {
				const f1 = MODES_HZ[0];
				warnings.push(ITEM('exponential_window_damping', 'config.impact.exponential_window', `The exponential window (tau ${round(tau, 4)} s) adds 1/(2 pi f_n tau) to every measured damping ratio: ${round(1 / (2 * Math.PI * f1 * tau), 5)} at ${f1} Hz. The results record tau so it can be taken out.`));
			}
			if (fw && fw.pre_frames + fw.width_frames + fw.fall_frames > block) errors.push(ITEM('out_of_range', 'config.impact.force_window', `The force window runs past the block: ${fw.pre_frames} + ${fw.width_frames} + ${fw.fall_frames} samples against a block of ${block}.`));
			if (c.target && !(c.target.peak > 0)) errors.push(ITEM('out_of_range', 'config.impact.target.peak', 'The target peak is above zero.'));
			impact = {
				drives: 'none: the hammer is the excitation',
				trigger: (() => {
					const polarity = c.polarity ?? 'positive';
					const level = round(((polarity === 'negative' ? -1 : 1) * (c.trigger_level_pct ?? 10) * (hammer?.range_max ?? 100)) / 100, 4);
					const unit = hammer?.units ?? '';
					return { level, unit, polarity, watches: `the force rising through ${level} ${unit} (${polarity} hammer); live once ${pre} pre-trigger frames are in`, pretrigger_frames: pre };
				})(),
				holdoff_blocks: c.holdoff_blocks ?? 1,
				accept: c.accept ?? 'auto',
				auto_advance: c.auto_advance ?? true,
				checks: {
					overload: 'rejects',
					double_hit: `rejects a second impact over ${c.double_hit_pct ?? 5}% of the peak`,
					tip_warn_db: c.tip_warn_db ?? 20,
					target: c.target ? { peak: c.target.peak, tolerance_pct: c.target.tolerance_pct ?? 20, unit: hammer?.units ?? '', rejects: false } : null
				},
				force_window: fw,
				exponential_window: ewOn ? { tau_s: round(tau, 6), inv_tau_per_s: round(1 / tau, 4), end, added_damping: '1/(2 pi f_n tau)' } : null
			};
		} else {
			const s = config.shaker ?? { drives: [] };
			const source = s.source ?? 'burst_random';
			const driveRows = host.rows().filter((r) => r.enabled && r.role === 'drive');
			if (!s.drives.length) errors.push(ITEM('no_drive', 'config.shaker.drives', 'No shaker drive: choose a drive row for each shaker.'));
			for (const d of s.drives) if (!driveRows.some((r) => r.index === d.row)) errors.push({ ...ITEM('not_a_drive', 'config.shaker.drives', `Row ${d.row} is not a drive row.`), channel: d.row });
			if (s.drives.length > 1 && (source === 'pseudo_random' || source === 'periodic_chirp' || source === 'burst_chirp'))
				errors.push(ITEM('source_refused', 'config.shaker.source', `The ${source.replace('_', ' ')} source drives one shaker only: ${source === 'pseudo_random' ? 'it repeats one frame, so every average sees the same forces and the force matrix cannot be inverted.' : 'a chirp on several shakers is not offered in this version.'}`));
			if (s.drives.length > 1 && averages < 2 * s.drives.length) errors.push(ITEM('too_few_averages', 'config.averages', `${averages} averages for ${s.drives.length} references: a test with several shakers takes at least twice as many averages as references (${2 * s.drives.length}).`));
			if (s.drives.length > 1 && (config.estimator ?? 'H1') !== 'H1') errors.push(ITEM('estimator_refused', 'config.estimator', `${config.estimator} is not offered with several references; matrix H1 is used.`));
			if (s.drives.length && refs.length && refs.length < s.drives.length) errors.push(ITEM('reference_per_shaker', 'channels', 'Each shaker needs its own force reference row.'));
			const ceiling = s.ceiling_v ?? 1;
			const crest = source.includes('chirp') ? 1.41 : 3.5;
			const drives = s.drives.map((d) => ({ row: d.row, level_vrms: d.level_vrms ?? 0.05, peak_v: round((d.level_vrms ?? 0.05) * crest, 4), range_v: 10 }));
			for (const d of drives) if (d.peak_v > ceiling) errors.push({ ...ITEM('over_ceiling', 'config.shaker.drives', `Row ${d.row}'s drive peaks at ${d.peak_v} V, over the ${ceiling} V ceiling.`), channel: d.row });
			const repeats = source === 'pseudo_random' || source === 'periodic_random' || source === 'periodic_chirp';
			shaker = {
				source,
				crest,
				drives,
				ceiling_v: ceiling,
				burst_pct: source.startsWith('burst') ? (s.burst_pct ?? 50) : null,
				ramp_s: s.ramp_s ?? 1,
				delay_frames: repeats ? (s.delay_frames ?? 2) : 0,
				overlap_pct: source === 'random' ? (s.overlap_pct ?? 50) : null,
				seed: s.seed ?? 1,
				window: source === 'random' ? 'hann' : 'uniform',
				stop: { depth_db: 40, ramp_s: 3, then: 'zero' },
				seconds_per_point: round((averages * block) / fs, 2)
			};
		}
		if (errors.length) return { errors, warnings, check: null, plan };
		const st = host.station();
		const check: ModalTestCheck = {
			source: st.source,
			mode,
			fs_hz: fs,
			block,
			frame_s: round(block / fs, 6),
			df_hz: round(df, 6),
			band: { ...band, first_line: Math.ceil(band.lo_hz / df), last_line: Math.floor(band.hi_hz / df), lines: Math.floor(band.hi_hz / df) - Math.ceil(band.lo_hz / df) + 1 },
			estimator: refs.length > 1 ? 'matrix H1' : (config.estimator ?? 'H1'),
			averages,
			references: refs.map((r) => dof(r, rovers.has(r.index))),
			responses: resps.map((r) => dof(r, rovers.has(r.index))),
			points: plan.map((x, k) => ({ index: k, ...x })),
			impact,
			shaker
		};
		return { errors, warnings, check, plan };
	}

	// ---- the state ----------------------------------------------------------------------------
	function countsOf(r: Run): ModalCounts {
		const per = r.plan.map((_, k) => {
			const at = r.hits.filter((h) => h.point === k);
			const accepted = at.filter((h) => h.status === 'accepted').length;
			return { index: k, accepted, rejected: at.filter((h) => h.status === 'rejected').length, taken: at.length, measured: accepted >= r.check.averages };
		});
		const here = per[r.point];
		return {
			unit: r.config.mode === 'impact' ? 'hits' : 'frames',
			point: { index: r.point, accepted: here.accepted, rejected: here.rejected, asked: r.check.averages },
			points: per,
			totals: { accepted: per.reduce((n, p) => n + p.accepted, 0), taken: per.reduce((n, p) => n + p.taken, 0), points_measured: per.filter((p) => p.measured).length, points_planned: per.length }
		};
	}

	const tile = (h: Hit): ModalTile => {
		const { point: _p, report: _r, kind: _k, ...t } = h;
		return t;
	};

	function stateOf(r: Run | null): ModalTestState {
		if (!r) return { run_id: null, state: 'none' };
		const at = r.hits.filter((h) => h.point === r.point);
		const last = r.hits[r.hits.length - 1];
		return {
			run_id: r.id,
			state: r.state,
			mode: r.config.mode,
			source: r.source,
			station_class: r.station.class,
			title: r.config.title ?? '',
			drive_phase: r.drivePhase,
			point: r.point,
			point_dofs: r.plan[r.point],
			counts: countsOf(r),
			hits: at.map(tile),
			latest: last ? { ...tile(last), point: last.point, report: last.report } : null,
			record: { folder: r.folder, streams: r.config.record?.streams ?? true, problem: null },
			end: r.end
		} as ModalTestRun;
	}

	function emit() {
		const s = stateOf(run);
		const key = JSON.stringify(s);
		if (key === lastKey) return;
		lastKey = key;
		host.broadcast('modal_test.state', s as unknown as Record<string, unknown>);
	}

	function note(r: Run, kind: string, text: string) {
		const e: ModalTestLogEntry = { seq: r.log.length + 1, at_unix: Date.now() / 1000, kind, text };
		r.log.push(e);
		host.broadcast('modal_test.log', { run_id: r.id, entries: [e] });
	}

	const answer = (r: Run) => ({ ok: true, counts: countsOf(r), state: stateOf(r) });

	// ---- the hits -----------------------------------------------------------------------------
	function hitPeak(r: Run, kind: ModalHitKind): number {
		const range = r.check.references[0]?.range ?? 100;
		if (kind === 'overload') return range * 1.02;
		const base = r.target?.peak ?? r.check.impact?.checks.target?.peak ?? range * 0.4;
		return round(base * (0.75 + 0.6 * r.rand()), 3);
	}

	function takeHit(r: Run) {
		const kind = r.nextKind ?? (r.rand() < 0.12 ? 'double' : 'clean');
		r.nextKind = null;
		const imp = r.check.impact!;
		const peak = hitPeak(r, kind);
		const overloaded = kind === 'overload';
		const ratio = kind === 'double' ? round(0.3 + 0.2 * r.rand(), 3) : 0;
		const pre = imp.trigger.pretrigger_frames;
		const fs = r.check.fs_hz;
		const second = kind === 'double' ? pre + Math.round(0.012 * fs) : 0;
		const pulseEnd = pre + Math.max(2, Math.round(0.003 * fs));
		const fw = imp.force_window;
		const windowEnd = fw ? fw.pre_frames + fw.width_frames : r.check.block;
		const cut = fw && pulseEnd > windowEnd ? { pulse_end_frame: pulseEnd, window_end_frame: windowEnd } : null;
		const tipDrop = round(8 + 6 * r.rand(), 1);
		const t = imp.checks.target;
		const target = r.target ?? (t ? { peak: t.peak, tolerance_pct: t.tolerance_pct } : null);
		const diff = target ? round(((peak - target.peak) / target.peak) * 100, 1) : undefined;
		const verdict = target && diff !== undefined ? (diff > target.tolerance_pct ? 'high' : diff < -target.tolerance_pct ? 'low' : 'on_target') : undefined;
		const reasons = [...(overloaded ? ['overload'] : []), ...(kind === 'double' ? ['double_hit'] : [])];
		const warnings = [...(cut ? ['force_window_cuts_pulse'] : []), ...(tipDrop > imp.checks.tip_warn_db ? ['tip_roll_off'] : []), ...(verdict === 'high' ? ['force_high'] : verdict === 'low' ? ['force_low'] : [])];
		const auto = imp.accept === 'auto';
		const status: ModalTile['status'] = auto ? (reasons.length ? 'rejected' : 'accepted') : 'pending';
		const at = r.hits.filter((h) => h.point === r.point);
		const h: Hit = {
			n: at.length + 1,
			seq: ++r.seq,
			status,
			reason: status === 'rejected' ? (overloaded ? 'overload' : 'double_hit') : null,
			overload: overloaded,
			double_hit: kind === 'double',
			start_frame: Math.round(r.t * fs),
			peak,
			tip_warning: tipDrop > imp.checks.tip_warn_db,
			force_window_cut: !!cut,
			...(verdict ? { target_verdict: verdict, target_diff_pct: diff } : {}),
			point: r.point,
			kind,
			report: {
				peak,
				peak_frame: pre + 2,
				overloaded: overloaded ? [0] : [],
				double_hit: kind === 'double' ? { frame: second, peak: round(peak * ratio, 3), ratio } : null,
				tip_drop_db: tipDrop,
				tip_warning: tipDrop > imp.checks.tip_warn_db,
				no_response: r.plan[r.point].responses.map(() => false),
				force_window_cut: cut,
				rejected: (reasons[0] as 'overload' | 'double_hit' | undefined) ?? null,
				reasons,
				warnings,
				...(target ? { target: target.peak, target_tolerance_pct: target.tolerance_pct, target_diff_pct: diff, target_verdict: verdict } : {})
			}
		};
		r.hits.push(h);
		note(r, 'hit', `Point ${r.point + 1}, hit ${h.n}: ${status === 'pending' ? 'captured' : status}${h.reason ? ` (${h.reason.replace('_', ' ')})` : ''}, peak ${peak}.`);
		if (!auto) r.state = 'captured';
		else settle(r);
	}

	/** After an accept: the point has its averages, or the test stays armed. */
	function settle(r: Run) {
		const c = countsOf(r);
		if (c.point.accepted >= c.point.asked) {
			note(r, 'point', `Point ${r.point + 1} measured: ${c.point.accepted} accepted.`);
			const auto = r.config.mode === 'impact' && r.check.impact?.auto_advance;
			const nextOpen = c.points.findIndex((p, k) => k > r.point && !p.measured);
			if (auto && nextOpen >= 0) {
				r.point = nextOpen;
				r.state = 'arming';
				r.clock = 0;
			} else {
				r.state = 'point_done';
				r.drivePhase = r.config.mode === 'shaker' ? 'zeroed' : 'idle';
			}
		} else if (r.config.mode === 'impact') r.state = 'arming';
	}

	function latestHere(r: Run): Hit | null {
		const last = r.hits[r.hits.length - 1];
		return last && last.point === r.point ? last : null;
	}

	// ---- the loop -----------------------------------------------------------------------------
	function tick() {
		const r = run;
		if (!r) return;
		const dt = (TICK_MS / 1000) * host.speed;
		if (isLive()) r.t += dt;
		if (r.state === 'starting') {
			r.state = 'idle';
			note(r, 'state', 'The station is open; the test is at its first point.');
		} else if (r.state === 'arming') {
			r.clock += dt;
			if (r.clock >= ARMING_S) {
				r.state = 'armed';
				r.clock = 0;
			}
		} else if (r.state === 'armed') {
			r.clock += dt;
			if (r.clock >= HIT_EVERY_S || r.nextKind) {
				r.clock = 0;
				takeHit(r);
			}
		} else if (r.state === 'measuring') {
			r.clock += dt;
			if (r.drivePhase === 'ramping' && r.clock >= (r.check.shaker?.ramp_s ?? 1)) {
				r.drivePhase = 'driving';
				r.clock = 0;
			} else if (r.drivePhase === 'driving' && r.clock >= FRAME_EVERY_S) {
				r.clock = 0;
				const at = r.hits.filter((h) => h.point === r.point);
				r.hits.push({ n: at.length + 1, seq: ++r.seq, status: 'accepted', reason: null, overload: false, double_hit: false, start_frame: Math.round(r.t * r.check.fs_hz), point: r.point, kind: 'clean', report: null });
				settle(r);
			}
		} else if (r.state === 'stopping') {
			r.clock += dt;
			if (r.clock >= (r.config.mode === 'shaker' ? 1 : 0.1)) finish(r, 'ended', 'The test ended.');
		}
		emit();
		const now = Date.now();
		if (isLive() && now - liveAt >= LIVE_MS) {
			liveAt = now;
			host.broadcast('modal_test.live', liveOf(r) as unknown as Record<string, unknown>);
		}
	}

	function ensureTimer() {
		if (!timer) timer = setInterval(tick, TICK_MS);
	}

	function finish(r: Run, outcome: ModalOutcome, message: string) {
		r.state = 'finished';
		r.drivePhase = r.config.mode === 'shaker' ? 'zeroed' : 'idle';
		r.end = { outcome, message, emergency_stop: false, at_s: round(r.t, 2) };
		note(r, 'end', message);
		emit();
		if (timer) {
			clearInterval(timer);
			timer = null;
		}
	}

	// ---- the live frame -----------------------------------------------------------------------
	function modeAt(dofs: ModalRowDof, k: number): number {
		const n = dofs.node ?? 1;
		return Math.sin(((k + 1) * Math.PI * n) / 9) + 0.1;
	}

	function frfOf(r: Run, f: number, resp: ModalRowDof, ref: number): { re: number; im: number } {
		let re = 0;
		let im = 0;
		for (let k = 0; k < MODES_HZ.length; k++) {
			const wk = 2 * Math.PI * MODES_HZ[k];
			const w = 2 * Math.PI * f;
			const a = (modeAt(resp, k) * (ref === 0 ? 1 : 0.6 + 0.2 * k)) * wk * wk * 2e-3;
			const dr = wk * wk - w * w;
			const di = 2 * ZETA[k] * wk * w;
			const den = dr * dr + di * di;
			re += (a * dr) / den;
			im += (-a * di) / den;
		}
		return { re: re * (2 * Math.PI * f) ** 2, im: im * (2 * Math.PI * f) ** 2 };
	}

	function liveOf(r: Run): ModalTestLive {
		const fs = r.check.fs_hz;
		const dofs = r.plan[r.point];
		const nref = dofs.references.length;
		const nresp = dofs.responses.length;
		const n = Math.min(SHOWN, r.check.block);
		const t0 = r.t;
		const noise = rng(Math.floor(t0 * 1000) + 7);
		const lines = Math.min(400, r.check.band.lines);
		const f = new Float32Array(lines);
		const hi = r.check.band.hi_hz;
		const lo = Math.max(r.check.band.lo_hz, hi / lines);
		for (let i = 0; i < lines; i++) f[i] = lo + ((hi - lo) * i) / (lines - 1);
		const counts = countsOf(r);
		const avg = counts.point.accepted;
		const scatter = avg > 0 ? 0.25 / Math.sqrt(avg) : 0;
		const pairs: ModalTestLive['frf']['pairs'] = [];
		const coherence: ModalTestLive['frf']['coherence'] = [];
		if (avg > 0)
			for (let ri = 0; ri < nresp; ri++) {
				const cv = new Float32Array(lines);
				for (let qi = 0; qi < nref; qi++) {
					const mag = new Float32Array(lines);
					const ph = new Float32Array(lines);
					for (let i = 0; i < lines; i++) {
						const h = frfOf(r, f[i], dofs.responses[ri], qi);
						const m = Math.hypot(h.re, h.im);
						mag[i] = m * (1 + scatter * (noise() - 0.5));
						ph[i] = (Math.atan2(h.im, h.re) * 180) / Math.PI;
					}
					pairs.push({ response: ri, reference: qi, magnitude: mag, phase_deg: ph });
				}
				for (let i = 0; i < lines; i++) {
					const near = Math.min(...MODES_HZ.map((m) => Math.abs(f[i] - (m + 80) / 2)));
					cv[i] = Math.max(0, Math.min(1, 0.99 - (avg === 1 ? 0 : 0.5 * Math.exp(-near / 4) + 0.04 * noise())));
				}
				coherence.push({ response: ri, values: avg === 1 ? new Float32Array(lines).fill(1) : cv });
			}
		const impact = r.config.mode === 'impact';
		// The most recent frames: in impact mode a small noise between hits; in shaker mode the drive.
		const channels: ModalTrace[] = [];
		const make = (amp: number, freq: number) => {
			const v = new Float32Array(n);
			for (let i = 0; i < n; i++) v[i] = impact ? amp * 0.004 * (noise() - 0.5) : amp * (Math.sin(2 * Math.PI * freq * (t0 + i / fs)) * 0.6 + (noise() - 0.5) * 0.8);
			return v;
		};
		// every measured channel in table order, then the drives
		const measured = [...dofs.references.map((d) => ({ d, role: 'reference' as const })), ...dofs.responses.map((d) => ({ d, role: 'response' as const }))].sort((a, b) => a.d.index - b.d.index);
		measured.forEach(({ d, role }, k) => channels.push({ values: role === 'reference' ? make(20, 61) : make(2 + k, 118), index: d.index, role, unit: d.unit }));
		if (!impact) (r.check.shaker?.drives ?? []).forEach((d) => channels.push({ values: make(d.level_vrms * 1.4, 61), index: d.row, role: 'drive', unit: 'V' }));
		const out: ModalTestLive = { run_id: r.id, state: r.state, point: r.point, time: { rate_hz: fs, channels }, frf: { frequency_hz: f, averages: avg, pairs, coherence } };
		const last = latestHere(r);
		if (impact && last?.report) {
			const rep = last.report;
			const pre = r.check.impact!.trigger.pretrigger_frames;
			const width = Math.max(4, Math.round(0.003 * fs));
			const force = new Float32Array(n);
			const pulse = (at: number, peak: number) => {
				for (let i = 0; i < width && at + i < n; i++) force[at + i] += peak * Math.sin((Math.PI * i) / width);
			};
			const seed = rng(last.seq * 31 + 3);
			for (let i = 0; i < n; i++) force[i] = rep.peak * 0.003 * (seed() - 0.5);
			pulse(pre, Math.min(rep.peak, (r.check.references[0]?.range ?? 100) * (rep.overloaded.length ? 1 : 0.999)));
			if (rep.double_hit) pulse(rep.double_hit.frame, rep.peak * rep.double_hit.ratio);
			const hch: ModalTrace[] = [{ values: force, index: dofs.references[0]?.index, role: 'reference', unit: dofs.references[0]?.unit }];
			dofs.responses.forEach((d, k) => {
				const v = new Float32Array(n);
				for (let i = 0; i < n; i++) {
					const tt = (i - pre) / fs;
					if (tt < 0) {
						v[i] = 0.002 * (seed() - 0.5);
						continue;
					}
					let s = 0;
					for (let m = 0; m < MODES_HZ.length; m++) s += modeAt(d, m) * Math.exp(-ZETA[m] * 2 * Math.PI * MODES_HZ[m] * tt) * Math.sin(2 * Math.PI * MODES_HZ[m] * tt);
					if (rep.double_hit) {
						const t2 = (i - rep.double_hit.frame) / fs;
						if (t2 >= 0) for (let m = 0; m < MODES_HZ.length; m++) s += rep.double_hit.ratio * modeAt(d, m) * Math.exp(-ZETA[m] * 2 * Math.PI * MODES_HZ[m] * t2) * Math.sin(2 * Math.PI * MODES_HZ[m] * t2);
					}
					v[i] = (rep.peak / 40) * (1 + 0.2 * k) * s;
				}
				hch.push({ values: v, index: d.index, role: 'response', unit: d.unit });
			});
			const fsv = new Float32Array(lines);
			const fc = 1 / (width / fs);
			for (let i = 0; i < lines; i++) {
				const x = f[i] / fc;
				const sinc = x === 0 ? 1 : Math.abs(Math.cos(Math.PI * x) / (1 - 4 * x * x));
				const dbl = rep.double_hit ? Math.abs(1 + rep.double_hit.ratio * Math.cos((2 * Math.PI * f[i] * (rep.double_hit.frame - pre)) / fs)) : 1;
				fsv[i] = Math.max(1e-9, (rep.peak * width) / fs) ** 2 * Math.max(1e-6, sinc * sinc) * dbl * dbl;
			}
			const imp = r.check.impact!;
			out.hit = {
				point: last.point,
				n: last.n,
				seq: last.seq,
				start_frame: last.start_frame,
				start_s: round(last.start_frame / fs, 6),
				samples: n,
				decimation: 1,
				step_s: 1 / fs,
				channels: hch,
				windows: { force: imp.force_window, exponential: imp.exponential_window ? { tau_s: imp.exponential_window.tau_s, end: imp.exponential_window.end } : null },
				force_spectrum: { frequency_hz: f, unit: `${dofs.references[0]?.unit ?? ''}^2/Hz`, values: fsv, tip_drop_db: rep.tip_drop_db }
			};
		} else if (impact) out.hit = null;
		if (!impact) out.frame = frameOf(r, dofs);
		if (!impact && avg > 0) {
			out.references = {
				frequency_hz: f,
				averages: avg,
				autospectra: dofs.references.map((q, qi) => ({ index: q.index, unit: `${q.unit}^2/Hz`, values: Float32Array.from(f, (x) => 0.02 * (1 + 0.3 * qi) * (1 + (scatter * 2) * (noise() - 0.5)) * (1 / (1 + (x / hi) ** 8))) }))
			};
		}
		return out;
	}

	/**
	 * The latest shaker frame taken at this point, whole: every channel over the block from the
	 * frame's start, min/max decimated as the service does above 1024 samples. The frame starts where
	 * the inputs see the burst begin; the drives, the same station frames, played it
	 * `MOCK_INPUT_DELAY` frames earlier. The inputs ring down after it, and the first response's level
	 * at the frame's end is its decay.
	 */
	function frameOf(r: Run, dofs: { references: ModalRowDof[]; responses: ModalRowDof[] }): ModalLiveFrame | null {
		const last = latestHere(r);
		const sh = r.check.shaker;
		if (!last || !sh) return null;
		const fs = r.check.fs_hz;
		const block = r.check.block;
		const burst = sh.burst_pct !== null ? Math.round((sh.burst_pct / 100) * block) : null;
		const delay = MOCK_INPUT_DELAY;
		const seed = rng(last.seq * 17 + 5);
		const tau = 0.25 * block / fs;
		const wave = (i: number, amp: number, freq: number, lead: number) => {
			const j = i + lead;
			const s = amp * (Math.sin((2 * Math.PI * freq * j) / fs) * 0.6 + (seed() - 0.5) * 0.8);
			if (burst === null || (j >= 0 && j < burst)) return s;
			if (lead || j < 0) return amp * 0.002 * (seed() - 0.5);
			return s * Math.exp(-(j - burst) / fs / tau);
		};
		const k = block <= SHOWN ? 1 : Math.ceil(block / (SHOWN / 2));
		const decimate = (x: Float32Array): Float32Array => {
			if (k === 1) return x;
			const out: number[] = [];
			for (let c = 0; c < x.length; c += k) {
				let lo = c;
				let hi = c;
				for (let i = c; i < Math.min(x.length, c + k); i++) {
					if (x[i] < x[lo]) lo = i;
					if (x[i] > x[hi]) hi = i;
				}
				const [a, b] = lo <= hi ? [lo, hi] : [hi, lo];
				out.push(x[a], x[b]);
			}
			return Float32Array.from(out);
		};
		const make = (amp: number, freq: number, lead: number) => Float32Array.from({ length: block }, (_, i) => wave(i, amp, freq, lead));
		const measured = [...dofs.references.map((d) => ({ d, role: 'reference' as const })), ...dofs.responses.map((d) => ({ d, role: 'response' as const }))].sort((a, b) => a.d.index - b.d.index);
		const channels: ModalTrace[] = [];
		let decay: ModalLiveFrame['decay'] = null;
		measured.forEach(({ d, role }, c) => {
			const x = make(role === 'reference' ? 20 : 2 + c, role === 'reference' ? 61 : 118, 0);
			if (role === 'response' && !decay && burst !== null) {
				const peak = x.reduce((m, v) => Math.max(m, Math.abs(v)), 0);
				const tail = x.slice(Math.max(0, block - Math.ceil(block / 64))).reduce((m, v) => Math.max(m, Math.abs(v)), 0);
				const ratio = peak > 0 ? round(tail / peak, 4) : 0;
				decay = { row: d.index, ratio, warning: ratio * 100 > (r.config.decay_warn_pct ?? 2) };
			}
			channels.push({ values: decimate(x), index: d.index, role, unit: d.unit });
		});
		for (const d of sh.drives) channels.push({ values: decimate(make(d.level_vrms * 1.4, 61, delay)), index: d.row, role: 'drive', unit: 'V' });
		return {
			point: last.point,
			n: last.n,
			seq: last.seq,
			status: last.status === 'rejected' ? 'rejected' : 'accepted',
			start_frame: last.start_frame,
			start_s: round(last.start_frame / fs, 6),
			samples: block,
			decimation: k,
			step_s: k === 1 ? 1 / fs : k / (2 * fs),
			burst_end_frame: burst === null ? null : last.start_frame + burst,
			input_delay_frames: delay,
			decay,
			channels
		};
	}

	// ---- the verbs ----------------------------------------------------------------------------
	function need(): Run {
		if (!run) refuse('not_configured', 'No Modal Test is set up: apply a setup first.');
		return run;
	}

	/** A live run; a verb that starts something new is refused `ending` once an end or an abort is asked. */
	function needLive(verb: string, starts = false): Run {
		const r = need();
		if (starts && r.ending) refuse('ending', `The test is ending: ${verb} starts nothing new after an end or an abort.`);
		if (r.state === 'configured') refuse('not_live', `${verb} waits for the test to start.`);
		if (r.state === 'finished') refuse('run_finished', `The test has ended; ${verb.toLowerCase()} does not apply.`);
		if (r.state === 'starting' || r.state === 'stopping') refuse('wrong_state', `${verb} does not apply while the test is ${r.state}.`, { state: r.state });
		return r;
	}

	function needImpact(r: Run, verb: string) {
		if (r.config.mode !== 'impact') refuse('not_in_mode', `${verb} is an impact verb; this test drives shakers.`, { mode: r.config.mode });
	}

	function moveTo(r: Run, point: number) {
		if (!Number.isInteger(point) || point < 0 || point >= r.plan.length) refuse('no_such_point', `The plan has no point ${point}.`, { point });
		if (r.state === 'measuring') refuse('wrong_state', 'Stop driving before moving to another point.', { state: r.state });
		r.point = point;
		r.clock = 0;
		r.state = countsOf(r).point.accepted >= r.check.averages ? 'point_done' : 'idle';
		r.drivePhase = 'idle';
		note(r, 'point', `Moved to point ${point + 1}.`);
	}

	return {
		isLive,
		liveId: () => (run && isLive() ? run.id : null),
		detach: () => {},
		seatExpired: () => {
			if (run && isLive()) {
				run.state = 'stopping';
				run.clock = 0;
				note(run, 'end', 'Nobody holds control: the test ends.');
			}
		},
		methods: {
			'modal_test.state': () => stateOf(run),
			'modal_test.config': () => ({ run_id: run?.id ?? null, config: run?.config ?? null, check: run?.check ?? null }),
			'modal_test.log': (params) => {
				const since = typeof params.since === 'number' ? params.since : 0;
				const entries = (run?.log ?? []).filter((e) => e.seq > since);
				return { entries, next: run ? run.log.length + 1 : 1 };
			},
			'modal_test.configure': (params, conn) => {
				if (!isObject(params.config)) bad('config', 'Configuring needs a test configuration.');
				const config = params.config as unknown as ModalTestConfig;
				if (typeof config.fs_hz !== 'number') bad('config.fs_hz', 'The sample rate is a number.');
				const res = checkOf(config);
				if (params.dry_run === true) return { errors: res.errors, warnings: res.warnings, check: res.check };
				host.own(conn);
				if (isLive()) refuse('run_active', `Test ${run!.id} is live; end it before setting up another.`, { run_id: run!.id });
				const other = host.otherRun();
				if (other) refuse('run_active', `Run ${other} is live; stop it before setting up a Modal Test.`, { run_id: other });
				if (res.errors.length || !res.check) refuse('config_invalid', `The setup has ${res.errors.length === 1 ? 'an error' : `${res.errors.length} errors`}.`, { errors: res.errors, warnings: res.warnings, check: res.check });
				const st = host.station();
				const id = `modal-test-${String(++runCount).padStart(4, '0')}`;
				run = {
					id,
					folder: host.recordFolder?.(id) ?? `runs/${id}`,
					config,
					check: res.check,
					state: 'configured',
					source: st.source,
					station: st.ref,
					point: 0,
					plan: res.plan,
					hits: [],
					seq: 0,
					nextKind: null,
					clock: 0,
					t: 0,
					target: null,
					end: null,
					drivePhase: 'idle',
					log: [],
					rand: rng(runCount * 977 + 11),
					ending: false
				};
				note(run, 'configure', `Set up: ${config.mode} mode, ${res.plan.length} ${res.plan.length === 1 ? 'point' : 'points'}, ${res.check.averages} averages.`);
				emit();
				return { state: stateOf(run), check: res.check, warnings: res.warnings };
			},
			'modal_test.start': (_p, conn) => {
				host.own(conn);
				const r = need();
				if (r.state !== 'configured') refuse('wrong_state', `Start applies to a configured test; this one is ${r.state}.`, { state: r.state });
				const other = host.otherRun();
				if (other) refuse('run_active', `Run ${other} is live; stop it first.`, { run_id: other });
				r.state = 'starting';
				note(r, 'start', `Started on the ${r.source === 'SIM' ? 'simulator' : 'station'}.`);
				ensureTimer();
				emit();
				return stateOf(r);
			},
			'modal_test.select': (params, conn) => {
				host.own(conn);
				const r = needLive('Select');
				moveTo(r, Number(params.point));
				emit();
				return answer(r);
			},
			'modal_test.next': (_p, conn) => {
				host.own(conn);
				const r = needLive('Next');
				moveTo(r, (r.point + 1) % r.plan.length);
				emit();
				return answer(r);
			},
			'modal_test.previous': (_p, conn) => {
				host.own(conn);
				const r = needLive('Previous');
				moveTo(r, (r.point - 1 + r.plan.length) % r.plan.length);
				emit();
				return answer(r);
			},
			'modal_test.arm': (_p, conn) => {
				host.own(conn);
				const r = needLive('Arm', true);
				needImpact(r, 'Arm');
				if (r.state === 'armed' || r.state === 'arming') refuse('already', 'The test is already armed.');
				if (r.state === 'point_done') refuse('point_done', 'This point has its hits: move to the next point, or undo one.');
				if (r.state === 'captured') refuse('wrong_state', 'Accept or reject the captured hit first.', { state: r.state });
				r.state = 'arming';
				r.clock = 0;
				note(r, 'arm', `Armed at point ${r.point + 1}.`);
				emit();
				return answer(r);
			},
			'modal_test.disarm': (_p, conn) => {
				host.own(conn);
				const r = needLive('Disarm');
				needImpact(r, 'Disarm');
				if (r.state !== 'armed' && r.state !== 'arming') refuse('wrong_state', 'The test is not armed.', { state: r.state });
				r.state = 'idle';
				note(r, 'arm', 'Disarmed.');
				emit();
				return answer(r);
			},
			'modal_test.accept': (_p, conn) => {
				host.own(conn);
				const r = needLive('Accept', true);
				needImpact(r, 'Accept');
				const h = latestHere(r);
				if (!h) refuse('no_hit', 'No hit at this point to accept.');
				if (h.status === 'accepted') refuse('already', 'This hit is already accepted.');
				if (h.status === 'undone') refuse('wrong_state', 'This hit was undone.');
				h.status = 'accepted';
				h.reason = null;
				note(r, 'operator', `Hit ${h.n} accepted by the operator.`);
				settle(r);
				emit();
				return answer(r);
			},
			'modal_test.reject': (_p, conn) => {
				host.own(conn);
				const r = needLive('Reject');
				needImpact(r, 'Reject');
				const h = latestHere(r);
				if (!h) refuse('no_hit', 'No hit at this point to reject.');
				if (h.status === 'rejected') refuse('already', 'This hit is already rejected.');
				if (h.status === 'undone') refuse('wrong_state', 'This hit was undone.');
				const was = h.status;
				h.status = 'rejected';
				h.reason = 'operator';
				note(r, 'operator', `Hit ${h.n} rejected by the operator.`);
				if (r.state === 'captured' || r.state === 'point_done' || was === 'accepted') r.state = 'arming';
				emit();
				return answer(r);
			},
			'modal_test.undo': (_p, conn) => {
				host.own(conn);
				const r = needLive('Undo');
				const h = [...r.hits].reverse().find((x) => x.point === r.point && x.status === 'accepted');
				if (!h) refuse('nothing_to_undo', 'Nothing accepted at this point to undo.');
				h.status = 'undone';
				note(r, 'operator', `${r.config.mode === 'impact' ? 'Hit' : 'Frame'} ${h.n} undone.`);
				if (r.state === 'point_done') r.state = 'idle';
				emit();
				return answer(r);
			},
			'modal_test.clear_point': (_p, conn) => {
				host.own(conn);
				const r = needLive('Clear Point');
				if (r.state === 'measuring' || r.state === 'captured') refuse('busy', r.state === 'measuring' ? 'Stop driving before clearing the point.' : 'Accept or reject the captured hit first.', { state: r.state });
				const taken = r.hits.filter((x) => x.point === r.point && x.status === 'accepted');
				if (!taken.length) refuse('nothing_to_undo', 'Nothing accepted at this point to clear.');
				for (const h of taken) h.status = 'undone';
				note(r, 'operator', `Point ${r.point + 1} cleared: ${taken.length} taken out.`);
				if (r.state === 'point_done') r.state = 'idle';
				emit();
				return answer(r);
			},
			'modal_test.target_from_hit': (_p, conn) => {
				host.own(conn);
				const r = needLive('Use This Hit as the Target');
				needImpact(r, 'Use This Hit as the Target');
				const h = latestHere(r);
				if (!h?.report) refuse('no_hit', 'Take a hit first.');
				r.target = { peak: h.report.peak, tolerance_pct: r.check.impact?.checks.target?.tolerance_pct ?? r.config.impact?.target?.tolerance_pct ?? 20 };
				note(r, 'target', `Target set from hit ${h.n}: ${h.report.peak}.`);
				emit();
				return answer(r);
			},
			'modal_test.drive_start': (_p, conn) => {
				host.own(conn);
				const r = needLive('Start Driving', true);
				if (r.config.mode !== 'shaker') refuse('not_in_mode', 'Start Driving is a shaker verb; this test is an impact test.', { mode: r.config.mode });
				if (r.state === 'measuring') refuse('already', 'The shakers are already driving.');
				if (r.state === 'point_done') refuse('point_done', 'This point has its frames: move to the next point, or undo.');
				r.state = 'measuring';
				r.drivePhase = 'ramping';
				r.clock = 0;
				note(r, 'drive', `Driving at point ${r.point + 1}.`);
				emit();
				return answer(r);
			},
			'modal_test.drive_stop': (_p, conn) => {
				host.own(conn);
				const r = needLive('Stop Driving');
				if (r.state !== 'measuring') refuse('wrong_state', 'The shakers are not driving.', { state: r.state });
				r.state = 'idle';
				r.drivePhase = 'zeroed';
				note(r, 'drive', 'Drive stopped.');
				emit();
				return answer(r);
			},
			'modal_test.sim_hit': (params, conn) => {
				host.own(conn);
				const r = needLive('Simulated Hit', true);
				needImpact(r, 'Simulated Hit');
				if (r.station.class !== 'sim') refuse('not_simulated', 'A simulated hit needs the simulator station.');
				const kind = params.kind;
				if (kind !== 'clean' && kind !== 'double' && kind !== 'overload') bad('kind', 'The kind is clean, double or overload.');
				r.nextKind = kind;
				return answer(r);
			},
			'modal_test.end': () => {
				const r = need();
				if (!isLive()) refuse('not_live', 'The test is not running.');
				r.ending = true;
				if (r.state !== 'stopping') {
					r.state = 'stopping';
					r.drivePhase = r.config.mode === 'shaker' ? 'stopping' : 'idle';
					r.clock = 0;
					note(r, 'end', 'End Test.');
				}
				emit();
				return stateOf(r);
			},
			'modal_test.abort': () => {
				const r = need();
				if (!isLive()) refuse('not_live', 'The test is not running.');
				r.ending = true;
				finish(r, 'aborted', 'Aborted by the operator.');
				return stateOf(r);
			},
			'modal_test.reset': (_p, conn) => {
				host.own(conn);
				const r = need();
				if (r.state !== 'finished') refuse('wrong_state', 'A new test starts from an ended one.', { state: r.state });
				const id = `modal-test-${String(++runCount).padStart(4, '0')}`;
				run = { ...r, id, folder: host.recordFolder?.(id) ?? `runs/${id}`, state: 'configured', point: 0, hits: [], seq: 0, nextKind: null, clock: 0, t: 0, target: null, end: null, drivePhase: 'idle', log: [], ending: false };
				note(run, 'configure', 'A new test from the same setup.');
				emit();
				return stateOf(run);
			},
			'modal_test.clear': (_p, conn) => {
				host.own(conn);
				if (isLive()) refuse('run_active', 'End the test before clearing it.', { run_id: run!.id });
				run = null;
				emit();
				return stateOf(null);
			}
		}
	};
}
