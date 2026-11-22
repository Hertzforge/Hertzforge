// MIMO Mixed Mode on the simulator, end to end over the control protocol (no browser): a client does
// what a screen does and checks what the service answers.
//
//   node tools/e2e-sim/mimo_mixed_protocol.mjs [--port 8792] [--hold gate|prediction]
//
// Start the service first (`sds-control --port <port>`). The run is the golden configuration
// (`schema/golden/request_mimo_mixed_configure.json`: a floor, two tones and a band on the
// simulator's sine-linear-4x4, four controls and four drives) in the automatic schedule: it
// configures (a dry run first), starts, reads the review's prediction, proceeds (with `--hold`,
// first holds the review at a failed gate or a prediction over a ceiling and releases it with an
// override), switches to manual mode, takes the level to full by hand, and checks the run completed
// after its time at full level. Exit 0 on success, 1 on a failed check, 2 on an error.

import { readFileSync } from 'node:fs';

const args = process.argv.slice(2);
const opt = (n, d) => {
	const i = args.indexOf(n);
	return i >= 0 && i + 1 < args.length ? args[i + 1] : d;
};
const PORT = opt('--port', '8792');
const HOLD = opt('--hold', '');

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
	index, enabled: true, description: `${role} ${index}`, role, physical: null, interface: 'single_ended',
	iepe_bias_v: null, units: role === 'drive' ? 'V' : 'g', sensitivity_mv_per_eu: 1000,
	range_max: role === 'drive' ? 10 : 1000, polarity: 'positive', control_group: null,
	abort: { accel_g: null, velocity_ips: null, displacement_mil: null },
});

const golden = new URL('../../schema/golden/request_mimo_mixed_configure.json', import.meta.url);
function config() {
	const c = JSON.parse(readFileSync(golden, 'utf8')).params.config;
	c.title = 'Protocol run';
	c.schedule.mode = 'auto';
	c.full_level_duration_s = 3;
	if (HOLD === 'gate') c.gates.coherence_threshold = 0.999999999;
	return c;
}

const state = (id, test) => (m) => m.topic === 'mimo_mixed.state' && m.data.run_id === id && test(m.data);

try {
	await new Promise((res, rej) => ((ws.onopen = res), (ws.onerror = rej)));
	await ok('session.claim', { name: 'Protocol run' });
	await ok('station.select', { kind: 'sim' });
	const t = await ok('channels.get', {});
	await ok('channels.set', { revision: t.revision, channels: [1, 2, 3, 4].map((i) => row(i, 'control')).concat([5, 6, 7, 8].map((i) => row(i, 'drive'))) });
	let cfg = config();
	const dry = await ok('mimo_mixed.configure', { config: cfg, dry_run: true });
	check('the dry run is clean', dry.errors.length === 0, dry.errors);
	if (HOLD === 'prediction') {
		// The prediction of a first review gives the drive's total; a ceiling under it holds the next.
		const first = (await ok('mimo_mixed.configure', { config: cfg })).state.run_id;
		await ok('mimo_mixed.start', {});
		const r = await until(state(first, (s) => s.phase === 'awaiting_review' || s.state === 'finished'), 120_000);
		cfg.drives[0].ceiling_vrms = 0.7 * r.data.prediction.drives[0].total_rms_v;
		await ok('mimo_mixed.abort', {});
		await until(state(first, (s) => s.state === 'finished'), 60_000);
	}
	const id = (await ok('mimo_mixed.configure', { config: cfg })).state.run_id;
	await ok('mimo_mixed.start', {});
	const review = await until(state(id, (s) => s.state === 'review' || s.state === 'finished'), 120_000);
	const p = review.data.prediction;
	if (p) for (const d of p.drives) console.log(`drive ${d.drive}: random ${d.random_rms_v} V rms, total ${d.total_rms_v} V rms, peak ${d.peak_v} V against ${d.ceiling_vrms} V rms, ${d.ceiling_peak_v} V peak`);
	if (HOLD) {
		const phase = HOLD === 'gate' ? 'gate_failed' : 'prediction_exceeded';
		check(`the review holds at ${phase}`, review.data.phase === phase, review.data.phase);
		const e = await call('mimo_mixed.proceed', {});
		check('proceed is refused by the hold\'s name', e.error?.reason === phase, e.error);
		await ok('mimo_mixed.override', { ack: 'Accepted for the protocol run.', reason: phase });
		const s = await ok('mimo_mixed.state', {});
		check('the override is kept with its words', s.overrides.length === 1 && s.overrides[0].reason === phase, s.overrides);
		if (HOLD === 'gate' && s.phase === 'awaiting_review') await ok('mimo_mixed.proceed', {});
	} else {
		check('the run holds for review with its prediction', review.data.phase === 'awaiting_review' && p?.within === true, [review.data.phase, p]);
		await ok('mimo_mixed.proceed', {});
	}
	const engaged = await until(state(id, (s) => s.phase === 'engaged' || s.state === 'finished'), 120_000);
	if (engaged.data.state === 'finished') {
		// Past an overridden prediction the drive's own ceilings and aborts still hold it.
		console.log('end', JSON.stringify(engaged.data.end));
		check('only an overridden prediction ends before the level by hand', HOLD === 'prediction', engaged.data.end);
	} else {
		const mode = await ok('mimo_mixed.mode', { mode: 'manual' });
		check('the mode switches to manual', mode.mode === 'manual', mode);
		const rung = await call('mimo_mixed.level', { to_db: -1.5 });
		check('a level off the schedule is refused not_a_rung', rung.error?.reason === 'not_a_rung', rung.error);
		const up = await call('mimo_mixed.level', { to_db: 0 });
		const end = await until(state(id, (s) => s.state === 'finished'), 600_000);
		console.log('end', JSON.stringify(end.data.end));
		if (HOLD === 'prediction') check('the run ended', end.data.end !== null, end.data.end);
		else check('the run completed at full level by hand', !up.error && end.data.end.outcome === 'completed', [up.error, end.data.end]);
	}
} catch (e) {
	console.error('ERROR', e);
	process.exit(2);
} finally {
	ws.close();
}
process.exit(failed ? 1 : 0);
