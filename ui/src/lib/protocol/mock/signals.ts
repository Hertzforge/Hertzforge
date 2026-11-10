/**
 * Mock Signals (schema/PROTOCOL.md, "Signals"): `signals.subscribe` and `signals.unsubscribe`,
 * refused as the service refuses them, and `signals.data` to each subscribed connection at its
 * rate while a run of any test type acquires, and once more when that run ends.
 *
 * The samples are made up: each input row a pair of tones and a little noise in its own unit,
 * each drive row a pair of tones in volts, on a station clock that starts with the run. Decimated
 * and laid out in views exactly as the service does, so the shell draws them the same way.
 */

import type { ChannelRow, SpectrumView } from '../types';
import {
	SIGNALS_DEFAULT_POINTS,
	SIGNALS_DEFAULT_RATE_HZ,
	SIGNALS_DEFAULT_WINDOW_S,
	SIGNALS_MAX_PLOTS,
	SIGNALS_MAX_POINTS,
	SIGNALS_MAX_RATE_HZ,
	SIGNALS_MAX_WINDOW_S,
	SIGNALS_MIN_POINTS,
	SIGNALS_MIN_RATE_HZ,
	type SignalArrange,
	type SignalChannel,
	type SignalChannelSelector,
	type SignalsData,
	type SignalsDataPlot,
	type SignalsSubscribedPlot,
	type SignalsSubscribeResult
} from '../signals';
import { MockError, type MockConnection, type MockMethod } from './socket';

/** What the rest of the mock service tells Signals. */
export interface SignalsMockHost {
	/** The channel table's rows. */
	rows(): ChannelRow[];
	/** The run acquiring now (its id and the rate it runs at), or null when none does. */
	acquiring(): { id: string; fsHz: number } | null;
}

export interface SignalsMock {
	methods: Record<string, MockMethod>;
	detach(conn: MockConnection): void;
}

/** How often the pump looks for frames due, ms. */
const TICK_MS = 50;
/** How early a frame may go against its period, ms. */
const SLACK_MS = 30;
/** Samples looked at for each point of an envelope: the mock's samples cost nothing to skip. */
const ENVELOPE_LOOKS = 8;

interface Display {
	rate: number;
	plots: SignalsSubscribedPlot[];
	last: number | null;
	/** The run and its frame count at the last frame sent. */
	sent: string | null;
	/** The run this connection saw acquiring and is owed a last frame of. */
	owed: string | null;
}

interface Run {
	id: string;
	fsHz: number;
	/** Wall time of the run's first frame, ms. */
	start: number;
	channels: (SignalChannel & { role: string })[];
	/** Frames when the run ended; null while it acquires. */
	ended: number | null;
}

function bad(param: string, message: string): never {
	throw new MockError('bad_request', message, 'bad_params', { param });
}

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);

/** Check and normalize `signals.subscribe` params, as the service does. */
export function parseSignalsSubscribe(params: Record<string, unknown>): { rate: number; plots: SignalsSubscribedPlot[] } {
	const extra = Object.keys(params).find((k) => k !== 'rate_hz' && k !== 'plots');
	if (extra) bad(extra, `signals.subscribe takes "rate_hz" and "plots", not "${extra}"`);
	let rate = SIGNALS_DEFAULT_RATE_HZ;
	if (params.rate_hz !== undefined && params.rate_hz !== null) {
		const r = params.rate_hz;
		if (typeof r !== 'number' || !Number.isFinite(r) || r < SIGNALS_MIN_RATE_HZ || r > SIGNALS_MAX_RATE_HZ) {
			bad('rate_hz', `"rate_hz" is a number of frames per second from ${SIGNALS_MIN_RATE_HZ} to ${SIGNALS_MAX_RATE_HZ}`);
		}
		rate = r;
	}
	const list = params.plots;
	if (list === undefined || list === null) bad('plots', 'a subscription lists its plots ("plots", possibly empty)');
	if (!Array.isArray(list)) bad('plots', '"plots" is a list');
	if (list.length > SIGNALS_MAX_PLOTS) bad('plots', `a subscription holds at most ${SIGNALS_MAX_PLOTS} plots; ${list.length} were sent`);
	const plots: SignalsSubscribedPlot[] = [];
	list.forEach((p: unknown, i: number) => {
		const at = (f: string) => `plots[${i}].${f}`;
		if (!isObject(p)) bad(`plots[${i}]`, 'each plot is an object');
		const key = Object.keys(p).find((k) => !['id', 'channels', 'window_s', 'points', 'arrange'].includes(k));
		if (key) bad(at(key), `a plot has an id, channels, window_s, points and arrange; not "${key}"`);
		const id = p.id;
		if (typeof id !== 'string' || id.length === 0 || [...id].length > 64) bad(at('id'), 'a plot\'s "id" is text of 1 to 64 characters');
		if (plots.some((q) => q.id === id)) bad(at('id'), `the plot id "${id}" is used twice; each plot's id is its own`);
		let channels: SignalChannelSelector = 'all';
		const ch = p.channels;
		const words = '"channels" is a list of channel indexes, or "all", "inputs", "drives" or "control"';
		if (typeof ch === 'string') {
			if (!['all', 'inputs', 'drives', 'control'].includes(ch)) bad(at('channels'), words);
			channels = ch as SignalChannelSelector;
		} else if (Array.isArray(ch)) {
			const ids: number[] = [];
			for (const x of ch) {
				if (typeof x !== 'number' || !Number.isInteger(x) || x < 0 || x > 0xffffffff) bad(at('channels'), 'a channel index is a whole number from 0');
				if (!ids.includes(x)) ids.push(x);
			}
			channels = ids;
		} else if (ch !== undefined && ch !== null) bad(at('channels'), words);
		let window = SIGNALS_DEFAULT_WINDOW_S;
		if (p.window_s !== undefined && p.window_s !== null) {
			const w = p.window_s;
			if (typeof w !== 'number' || !Number.isFinite(w) || w <= 0 || w > SIGNALS_MAX_WINDOW_S) bad(at('window_s'), `"window_s" is a number of seconds above 0 and at most ${SIGNALS_MAX_WINDOW_S}`);
			window = w;
		}
		let points = SIGNALS_DEFAULT_POINTS;
		if (p.points !== undefined && p.points !== null) {
			const n = p.points;
			if (typeof n !== 'number' || !Number.isInteger(n) || n < SIGNALS_MIN_POINTS || n > SIGNALS_MAX_POINTS) bad(at('points'), `"points" is a whole number from ${SIGNALS_MIN_POINTS} to ${SIGNALS_MAX_POINTS}`);
			points = n;
		}
		let arrange: SignalArrange = 'overlay';
		if (p.arrange !== undefined && p.arrange !== null) {
			if (p.arrange !== 'overlay' && p.arrange !== 'split') bad(at('arrange'), '"arrange" is "overlay" (one view for each unit) or "split" (one view for each channel)');
			arrange = p.arrange;
		}
		plots.push({ id, channels, window_s: window, points, arrange });
	});
	return { rate, plots };
}

/** The run's channels as a run configured with `rows` taps them: the enabled rows in table order. */
function channelsOf(rows: ChannelRow[]): (SignalChannel & { role: string })[] {
	return rows
		.filter((r) => r.enabled)
		.map((r) => ({
			index: r.index,
			name: r.description.trim() || r.role,
			unit: r.role === 'drive' ? 'V' : r.units,
			kind: r.role === 'drive' ? ('drive' as const) : ('input' as const),
			role: r.role
		}));
}

/** A deterministic noise in [-1, 1) for frame `f` of channel `k`. */
function noise(f: number, k: number): number {
	const s = Math.sin(f * 12.9898 + k * 78.233) * 43758.5453;
	return 2 * (s - Math.floor(s)) - 1;
}

/** Channel `c`'s made-up sample at station frame `f`. */
function sample(c: SignalChannel, k: number, f: number, fs: number): number {
	const t = f / fs;
	if (c.kind === 'drive') return 0.8 * Math.sin(2 * Math.PI * 55 * t) + 0.2 * Math.sin(2 * Math.PI * 130 * t + k);
	const f1 = 40 + 15 * k;
	return 0.6 * Math.sin(2 * Math.PI * f1 * t + k) + 0.3 * Math.sin(2 * Math.PI * 3.1 * f1 * t) + 0.05 * noise(f, k);
}

const TIME_AXIS = { label: 'Time', unit: 's', scale: 'linear' as const };

function viewTitle(chs: SignalChannel[]): string {
	return chs.length === 1 ? `Channel ${chs[0].index}: ${chs[0].name}` : `Channels ${chs.map((c) => c.index).join(', ')}`;
}

export function createSignalsMock(host: SignalsMockHost): SignalsMock {
	const displays = new Map<MockConnection, Display>();
	let run: Run | null = null;
	let timer: ReturnType<typeof setInterval> | null = null;

	const framesOf = (r: Run, now = Date.now()) => r.ended ?? Math.max(0, Math.floor(((now - r.start) / 1000) * r.fsHz));
	const maxWindowS = () => SIGNALS_MAX_WINDOW_S;

	/** The run now: a new one whenever a run starts acquiring, the last one ended when none does. */
	function current(): Run | null {
		const now = host.acquiring();
		if (now && (!run || run.id !== now.id || run.ended !== null)) {
			run = { id: now.id, fsHz: now.fsHz, start: Date.now(), channels: channelsOf(host.rows()), ended: null };
		}
		if (!now && run && run.ended === null) run.ended = framesOf(run);
		return run;
	}

	function resolve(r: Run, sel: SignalChannelSelector): Run['channels'] {
		return r.channels.filter((c) =>
			sel === 'all' ? true : sel === 'inputs' ? c.kind === 'input' : sel === 'drives' ? c.kind === 'drive' : sel === 'control' ? c.role === 'control' : sel.includes(c.index)
		);
	}

	function view(r: Run, chs: Run['channels'], lo: number, hi: number, points: number): SpectrumView {
		const n = hi - lo;
		const series = n <= 0 ? [] : chs.map((c) => {
			const k = r.channels.indexOf(c);
			const role = c.kind === 'drive' ? ('drive' as const) : ('measured' as const);
			const name = `Channel ${c.index}`;
			if (n <= points) {
				const x = Float64Array.from({ length: n }, (_, i) => (lo + i) / r.fsHz);
				const y = Float64Array.from({ length: n }, (_, i) => sample(c, k, lo + i, r.fsHz));
				return { name, role, x, y };
			}
			const span = Math.ceil(n / points);
			const m = Math.ceil(n / span);
			const x = new Float64Array(m);
			const y = new Float64Array(m);
			const yMin = new Float64Array(m);
			for (let p = 0; p < m; p++) {
				const a = lo + p * span;
				const len = Math.min(span, hi - a);
				const step = Math.max(1, Math.floor(len / ENVELOPE_LOOKS));
				let top = -Infinity;
				let bottom = Infinity;
				for (let f = a; f < a + len; f += step) {
					const v = sample(c, k, f, r.fsHz);
					top = Math.max(top, v);
					bottom = Math.min(bottom, v);
				}
				x[p] = a / r.fsHz;
				y[p] = top;
				yMin[p] = bottom;
			}
			return { name, role, x, y, y_min: yMin };
		});
		return { view: 'spectrum', title: viewTitle(chs), x: TIME_AXIS, y: { label: 'Amplitude', unit: chs[0].unit, scale: 'linear' }, series };
	}

	function data(r: Run, plots: SignalsSubscribedPlot[], available: boolean): SignalsData {
		const now = framesOf(r);
		const out: SignalsDataPlot[] = plots.map((p) => {
			const chs = resolve(r, p.channels);
			const frames = Math.min(Math.max(1, Math.round(p.window_s * r.fsHz)), Math.round(maxWindowS() * r.fsHz));
			const lo = Math.max(0, now - frames);
			let views: SpectrumView[];
			if (p.arrange === 'split') views = chs.map((c) => view(r, [c], lo, now, p.points));
			else {
				const units = [...new Set(chs.map((c) => c.unit))];
				views = units.map((u) => view(r, chs.filter((c) => c.unit === u), lo, now, p.points));
			}
			return { id: p.id, channels: chs.map((c) => c.index), window_s: frames / r.fsHz, first_frame: lo, views };
		});
		return {
			available,
			fs_hz: r.fsHz,
			t_s: now / r.fsHz,
			frames: now,
			max_window_s: maxWindowS(),
			lost_frames: 0,
			channels: r.channels.map(({ index, name, unit, kind }) => ({ index, name, unit, kind })),
			plots: out
		};
	}

	function emit(conn: MockConnection, r: Run, d: Display, available: boolean) {
		conn.emit('signals.data', data(r, d.plots, available) as unknown as Record<string, unknown>);
	}

	function tick() {
		const r = current();
		if (displays.size === 0) {
			if (timer) clearInterval(timer);
			timer = null;
			return;
		}
		if (!r) return;
		const now = Date.now();
		const live = r.ended === null;
		const frames = framesOf(r, now);
		const key = `${r.id}:${frames}`;
		for (const [conn, d] of displays) {
			let send: boolean;
			if (live) {
				d.owed = r.id;
				send = frames > 0 && d.sent !== key && (d.last === null || now + SLACK_MS >= d.last + 1000 / d.rate);
			} else {
				send = d.owed === r.id;
				d.owed = null;
			}
			if (send) {
				d.last = now;
				d.sent = key;
				emit(conn, r, d, live);
			}
		}
	}

	function ensureTimer() {
		if (!timer) timer = setInterval(tick, TICK_MS);
	}

	return {
		detach(conn) {
			displays.delete(conn);
		},
		methods: {
			'signals.subscribe': (params, conn): SignalsSubscribeResult => {
				const { rate, plots } = parseSignalsSubscribe(params);
				const r = current();
				const live = r && r.ended === null ? r : null;
				const d: Display = { rate, plots, last: null, sent: null, owed: live ? live.id : null };
				displays.set(conn, d);
				ensureTimer();
				// A plot added while a run acquires fills at once.
				if (live) {
					setTimeout(() => {
						if (displays.get(conn) !== d || live.ended !== null || framesOf(live) === 0) return;
						d.last = Date.now();
						d.sent = `${live.id}:${framesOf(live)}`;
						emit(conn, live, d, true);
					}, 30);
				}
				return {
					available: live !== null,
					fs_hz: live ? live.fsHz : null,
					max_window_s: live ? maxWindowS() : null,
					rate_hz: rate,
					channels: live ? live.channels.map(({ index, name, unit, kind }) => ({ index, name, unit, kind })) : [],
					plots
				};
			},
			'signals.unsubscribe': (params, conn) => {
				const extra = Object.keys(params)[0];
				if (extra) bad(extra, `signals.unsubscribe takes no parameters, not "${extra}"`);
				displays.delete(conn);
				return {};
			}
		}
	};
}
