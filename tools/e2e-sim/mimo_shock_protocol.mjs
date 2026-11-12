// MIMO Shock on the simulator, end to end over the control protocol (no browser): a client does
// what a screen does and checks what the service answers.
//
//   node tools/e2e-sim/mimo_shock_protocol.mjs [--port 8792] [--target waveform|srs]
//
// Start the service first (`sds-control --port <port>`). The run uses the simulator's
// shock-linear-4x4 plant (four controls, four drives): it configures (a dry run first, for the
// target views), starts, reads the review with each control's reachable floor, proceeds, follows
// the pulses (every mimo_shock.pulse frame decoded, its views named per control), and checks the
// run completed with its last three pulses at full level passing. Exit 0 on success, 1 on a failed
// check, 2 on an error.

const args = process.argv.slice(2);
const opt = (n, d) => {
	const i = args.indexOf(n);
	return i >= 0 && i + 1 < args.length ? args[i + 1] : d;
};
const PORT = opt('--port', '8792');
const KIND = opt('--target', 'waveform');

function decode(data) {
	if (typeof data === 'string') return JSON.parse(data);
	const b = Buffer.from(data);
	if (b.subarray(0, 4).toString('latin1') !== 'SDS1') throw new Error('a binary frame without its magic');
	const h = b.readUInt32LE(4);
	return JSON.parse(b.subarray(8, 8 + h).toString('utf8'));
}

const ws = new WebSocket(`ws://127.0.0.1:${PORT}/`);
ws.binaryType = 'arraybuffer';
const pending = new Map();
const events = [];
let nextId = 1;
ws.onmessage = (e) => {
	const m = decode(e.data);
	if (m.type === 'response') pending.get(m.id)?.(m);
	else if (m.type === 'event') events.push(m);
};
const call = (method, params = {}) =>
	new Promise((res) => {
		const id = nextId++;
		pending.set(id, res);
		ws.send(JSON.stringify({ type: 'request', id, method, params }));
	});
const ok = async (method, params) => {
	const r = await call(method, params);
	if (r.error) throw new Error(`${method}: ${JSON.stringify(r.error)}`);
	return r.result;
};
const until = async (test, ms) => {
	const t = Date.now();
	for (;;) {
		const m = events.find(test);
		if (m) return m;
		if (Date.now() - t > ms) throw new Error('timed out');
		await new Promise((r) => setTimeout(r, 50));
	}
};
let failed = 0;
const check = (name, pass, value) => {
	console.log(pass ? 'PASS' : 'FAIL', name, pass ? '' : JSON.stringify(value));
	if (!pass) failed++;
};

const row = (index, role) => ({
	index, enabled: true, description: role, role, physical: null, interface: 'single_ended',
	iepe_bias_v: null, units: role === 'drive' ? 'V' : 'g', sensitivity_mv_per_eu: 1000,
	range_max: role === 'drive' ? 10 : 1000, polarity: 'positive', control_group: null,
	abort: { accel_g: null, velocity_ips: null, displacement_mil: null },
});

function config() {
	const base = {
		title: `Protocol run (${KIND})`,
		channels: {
			controls: [1, 2, 3, 4].map((channel) => ({ channel })),
			drives: [5, 6, 7, 8].map((channel) => ({ channel, ceiling_vpk: 6, abort_vpk: 9 })),
		},
		acquisition: { fs_hz: 12800, record_len: KIND === 'srs' ? 16384 : 8192, band_hz: [5, 2500] },
		schedule: { mode: 'auto', levels_db: [-12, -9, -6, -3, 0] },
		spacing: { cooling_s: 0.2, settling_s: 0, compute_s: 0 },
		identification: { drive_vrms: 1, frames: 40, review: true },
		control: { law_budget: 1 },
		simulator: { plant: 'shock-linear-4x4' },
	};
	base.target =
		KIND === 'srs'
			? {
					kind: 'srs', bands: { f_lo_hz: 50, f_hi_hz: 2000, bands_per_octave: 3, q: 10 },
					controls: [20, 16, 20, 16].map((g) => ({ target: { kind: 'breakpoints', points: [[50, g / 6], [300, g], [2500, g]] } })),
				}
			: { kind: 'waveform', pulses: [20, 18, 20, 18].map((g) => ({ shape: 'half_sine', amplitude_g: g, duration_ms: 11 })) };
	return base;
}

try {
	await new Promise((res, rej) => ((ws.onopen = res), (ws.onerror = rej)));
	await ok('session.claim', { name: 'Protocol run' });
	await ok('station.select', { kind: 'sim' });
	const t = await ok('channels.get', {});
	await ok('channels.set', { revision: t.revision, channels: [1, 2, 3, 4].map((i) => row(i, 'control')).concat([5, 6, 7, 8].map((i) => row(i, 'drive'))) });
	const dry = await ok('mimo_shock.configure', { config: config(), dry_run: true });
	check('the dry run is clean and draws each target', dry.errors.length === 0 && dry.views.length >= 4, dry.errors);
	const r = await ok('mimo_shock.configure', { config: config() });
	const id = r.state.run_id;
	await ok('mimo_shock.start', {});
	const review = await until((m) => m.topic === 'mimo_shock.state' && m.data.run_id === id && ['review', 'finished'].includes(m.data.state), 120_000);
	check('the run holds for review with each control\'s floor', review.data.state === 'review' && review.data.review.controls.length === 4, review.data.end);
	await ok('mimo_shock.proceed', {});
	const end = await until((m) => m.topic === 'mimo_shock.state' && m.data.run_id === id && m.data.state === 'finished', 600_000);
	const rows = (await ok('mimo_shock.pulses', {})).pulses;
	console.log('end', JSON.stringify(end.data.end));
	for (const p of rows) console.log(`pulse ${p.index} at ${p.level_db} dB: ${p.passed ? 'passed' : 'failed'}, over-test ${p.over_test_db} dB`);
	check('the run completed', end.data.end.outcome === 'completed', end.data.end);
	check('the last three pulses at full level passed', rows.slice(-3).every((p) => p.level_db === 0 && p.passed), rows.slice(-3));
	const frames = events.filter((m) => m.topic === 'mimo_shock.pulse');
	check('one pulse frame per row', frames.length === rows.length, [frames.length, rows.length]);
	const titles = frames.at(-1)?.data.views.map((v) => v.title) ?? [];
	check('a pulse frame names its views per control', titles.includes('Pulse 1') && titles.includes('Drives'), titles);
} catch (e) {
	console.error('ERROR', e);
	process.exit(2);
} finally {
	ws.close();
}
process.exit(failed ? 1 : 0);
