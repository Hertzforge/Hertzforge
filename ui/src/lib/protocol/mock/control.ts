/**
 * Mock control service: a fake random-vibration simulation that streams `random.display` at
 * 5 Hz. Every event says `source: "SIM"`. One profile is built to abort, so the shell's abort
 * path can be seen without hardware.
 */

import type { ReadingRow, RunState, SimProfile, SpectrumView } from '../types';
import { MockConnection, MockError, type MockServer } from './socket';
import { binEdges } from './bins';
import {
	breakpointArrays,
	frequencyLines,
	gauss,
	hashSeed,
	QUAL_SPEC,
	rms,
	rng,
	scaleSpec,
	specAt,
	specRms,
	WORKMANSHIP_SPEC,
	type Breakpoints
} from './synth';

interface Profile extends SimProfile {
	spec: Breakpoints;
	/** Elapsed time (s) at which a resonance starts to grow past the abort limit. */
	growsAt?: number;
}

const PROFILES: Profile[] = [
	{ id: 'qual', title: 'Qualification Random', summary: '20 to 2000 Hz, 0.04 g²/Hz plateau, 6.06 g rms at 0 dB.', spec: QUAL_SPEC },
	{ id: 'workmanship', title: 'Workmanship Screen', summary: '20 to 2000 Hz, 0.04 g²/Hz plateau to 500 Hz.', spec: WORKMANSHIP_SPEC },
	{
		id: 'abort-demo',
		title: 'Abort Demonstration',
		summary: 'Qualification profile with a resonance near 420 Hz that grows until the abort limit trips.',
		spec: QUAL_SPEC,
		growsAt: 4
	}
];

const TOLERANCE_DB = 3;
const ABORT_DB = 6;
const FRAME_MS = 200;

interface Run {
	id: string;
	profile: Profile;
	targetDb: number;
	speed: number;
	elapsed: number;
	frames: number;
	timer: ReturnType<typeof setInterval>;
}

export function createControlServer(): MockServer {
	const connections = new Set<MockConnection>();
	let state: RunState = 'idle';
	let run: Run | null = null;
	let lastRunId: string | null = null;
	let runCount = 0;
	const f = frequencyLines();

	function broadcast(data: Record<string, unknown>) {
		for (const c of connections) c.emit('random.display', data);
	}

	/** Controllers start low and step up: -6 dB, then +3 dB every 1.5 s to the target. */
	function currentLevel(r: Run): number {
		const steps = Math.floor(r.elapsed / 1.5);
		return Math.min(r.targetDb, r.targetDb - 6 + 3 * steps);
	}

	function frame(r: Run, final: RunState, abortNote?: string) {
		const level = currentLevel(r);
		const spec = scaleSpec(r.profile.spec, level);
		const next = rng(hashSeed(r.id, r.frames));
		const scatterDb = 1.1 / Math.sqrt(1 + r.frames / 3) + 0.35;
		const growth = r.profile.growsAt !== undefined ? Math.max(0, r.elapsed - r.profile.growsAt) * 1.5 : 0;
		const control = new Float64Array(f.length);
		let out = 0;
		let peakDev = 0;
		let peakF = f[0];
		// The abort test uses the estimate without its scatter, as a controller's averaged one would.
		let trendPeak = 0;
		let trendF = f[0];
		for (let i = 0; i < f.length; i++) {
			const ripple = 0.35 * Math.sin(Math.log(f[i]) * 6.1 + r.frames * 0.05);
			const bump = growth * Math.exp(-(((Math.log(f[i]) - Math.log(420)) / 0.05) ** 2));
			const trend = ripple + bump;
			const dev = trend + scatterDb * gauss(next);
			if (Math.abs(trend) > Math.abs(trendPeak)) {
				trendPeak = trend;
				trendF = f[i];
			}
			control[i] = specAt(spec, f[i]) * 10 ** (dev / 10);
			if (Math.abs(dev) > TOLERANCE_DB) out++;
			if (Math.abs(dev) > Math.abs(peakDev)) {
				peakDev = dev;
				peakF = f[i];
			}
		}
		const controlRms = rms(f, control);
		const refRms = specRms(spec);
		const errDb = 20 * Math.log10(controlRms / refRms);
		const band = (d: number) => breakpointArrays(scaleSpec(spec, d));

		const spectrum: SpectrumView = {
			view: 'spectrum',
			title: 'Control Against Reference',
			x: { label: 'Frequency', unit: 'Hz', scale: 'log' },
			y: { label: 'PSD', unit: 'g²/Hz', scale: 'log' },
			series: [
				{ name: 'Control', role: 'measured', x: f.slice(), y: control, shape: 'steps', edges: binEdges(f) },
				{ name: 'Reference', role: 'reference', ...breakpointArrays(spec) },
				{ name: `+${TOLERANCE_DB} dB Tolerance`, role: 'tolerance', ...band(TOLERANCE_DB) },
				{ name: `-${TOLERANCE_DB} dB Tolerance`, role: 'tolerance', ...band(-TOLERANCE_DB) },
				{ name: `+${ABORT_DB} dB Abort`, role: 'abort', ...band(ABORT_DB) },
				{ name: `-${ABORT_DB} dB Abort`, role: 'abort', ...band(-ABORT_DB) }
			],
			link: 'live-frequency'
		};
		const rows: ReadingRow[] = [
			{ label: 'Control rms', value: Number(controlRms.toFixed(2)), unit: 'g', status: Math.abs(errDb) <= 1 ? 'pass' : Math.abs(errDb) <= 3 ? 'warn' : 'fail' },
			{ label: 'Reference rms', value: Number(refRms.toFixed(2)), unit: 'g', status: 'info' },
			{ label: 'Control error', value: `${errDb >= 0 ? '+' : ''}${errDb.toFixed(2)}`, unit: 'dB', status: Math.abs(errDb) <= 1 ? 'pass' : 'warn' },
			{ label: 'Level', value: `${level >= 0 ? '+' : ''}${level.toFixed(1)} of ${r.targetDb >= 0 ? '+' : ''}${r.targetDb.toFixed(1)}`, unit: 'dB', status: level < r.targetDb ? 'info' : 'pass' },
			{ label: 'Lines out of tolerance', value: out, unit: null, status: out === 0 ? 'pass' : out < 12 ? 'warn' : 'fail' },
			{
				label: 'Largest deviation',
				value: `${peakDev >= 0 ? '+' : ''}${peakDev.toFixed(1)} dB at ${peakF} Hz`,
				unit: null,
				status: Math.abs(peakDev) <= TOLERANCE_DB ? 'pass' : Math.abs(peakDev) <= ABORT_DB ? 'warn' : 'fail'
			},
			{ label: 'Drive rms', value: Number((0.42 * 10 ** (level / 20) * (1 + 0.02 * gauss(next))).toFixed(3)), unit: 'V', status: 'info' }
		];
		if (abortNote) rows.unshift({ label: 'Abort', value: abortNote, unit: null, status: 'fail' });
		return {
			data: {
				run_id: r.id,
				source: 'SIM',
				state: final,
				elapsed_s: Number(r.elapsed.toFixed(1)),
				level_db: level,
				views: [spectrum, { view: 'reading', title: 'Control Readings', rows }]
			},
			trendPeak,
			trendF
		};
	}

	function tick() {
		const r = run;
		if (!r) return;
		r.elapsed += r.speed === 0 ? 1.0 : FRAME_MS / 1000;
		r.frames += 1;
		const { data, trendPeak, trendF } = frame(r, 'running');
		if (Math.abs(trendPeak) > ABORT_DB) {
			const note = `Control exceeded the ${ABORT_DB} dB abort limit at ${trendF} Hz (${trendPeak >= 0 ? '+' : ''}${trendPeak.toFixed(2)} dB).`;
			finish(r, note);
			return;
		}
		broadcast(data);
	}

	function finish(r: Run, abortNote?: string) {
		clearInterval(r.timer);
		run = null;
		state = 'stopped';
		broadcast(frame(r, 'stopped', abortNote).data);
	}

	return {
		hello: { type: 'hello', protocol: 'sds/1', service: 'control', version: '0.1.0-mock' },
		latencyMs: () => 25,
		attach: (c) => void connections.add(c),
		detach: (c) => void connections.delete(c),
		methods: {
			'sim.profiles': () => ({ profiles: PROFILES.map(({ id, title, summary }) => ({ id, title, summary })) }),
			'sim.state': () => ({ state, run_id: run?.id ?? lastRunId }),
			'sim.start': (params) => {
				if (run) throw new MockError('refused', 'A simulation is already running; stop it before starting another.');
				const profile = PROFILES.find((p) => p.id === params.profile);
				if (!profile) throw new MockError('not_found', `No profile named "${String(params.profile)}"; sim.profiles lists them.`);
				const level = Number(params.level_db);
				if (!Number.isFinite(level) || level < -24 || level > 6) {
					throw new MockError('bad_request', 'Level must be between -24 and +6 dB.');
				}
				const speed = params.speed === 0 ? 0 : 1;
				const id = `sim-${String(++runCount).padStart(4, '0')}`;
				lastRunId = id;
				state = 'running';
				run = { id, profile, targetDb: level, speed, elapsed: 0, frames: 0, timer: setInterval(tick, FRAME_MS) };
				return { run_id: id };
			},
			'sim.stop': () => {
				if (run) finish(run);
				return { state };
			}
		}
	};
}
