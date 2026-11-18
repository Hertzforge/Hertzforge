// MIMO Sine against the real control service on the simulator, and the finished run opened in
// the analysis side, driven through the installed Chrome the way an operator drives them, every
// check made on what the screen shows and on the run folder the run left.
//
//   npm --prefix tools/e2e-sim ci
//   node tools/e2e-sim/mimo_sine_sim.mjs [--exe <sds-control>] [--python <python with hertzforge>]
//                                        [--data <dir>] [--shots <dir>] [--headed]
//
// Without `--exe` it builds `sds-control` (release) and runs that. It starts the control service
// (loopback, a free port, its data folder `--data`, else a new temporary folder), the analysis
// service and the shell's dev server, and stops all three. Not part of `npm --prefix ui test`.
//
// 0. The channel table is set to the simulator's base and article over the wire before the screen
//    opens: controls 1 to 4 on the base, the article's two responses 5 and 6 as monitors, drives 7
//    to 10.
// 1. MIMO Sine is chosen in the chooser and takes control.
//    Station & Channels: each drive's ceiling and abort, and the base-and-article structure.
//    References: each control's abort band at 6 dB. Test: a sweep of 150 to 350 Hz at 2 oct/min,
//    one way, through the article's 230 Hz resonance. Schedule: one level; the identification.
//    Limits: channels 5 and 6 limit channels with a 0.6 g bound, which the article passes
//    near its resonance at the schedule's one level (-12 dB). Control: the laws that cannot run a
//    sweep are listed apart, with why. The run's record: operator and article.
// 2. Start identifies; the review hold shows the identification; Proceed with words for the log
//    brings the Controls page forward. The frequency readout moves, every control's trace with its
//    bands on the Controls page, the article's limit channels take the notch in near 230 Hz, and
//    the run completes with its one sweep counted and the time the notch was in.
// 3. The run folder holds the operator's words and is complete; Open in Analysis opens the
//    project with the objects docs/run-folder.md says a MIMO Sine run saves.
//
// Screenshots (1,440 by 1,000): `mimo_sine_sim_<name>.png`.

import { chromium } from 'playwright-core';
import { execFileSync, spawn } from 'node:child_process';
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');
const args = process.argv.slice(2);
const flag = (name) => {
	const i = args.indexOf(name);
	return i >= 0 ? args[i + 1] : undefined;
};
const SHOTS = path.resolve(flag('--shots') ?? path.join(os.tmpdir(), 'hertzforge-e2e-mimo-sine-sim'));
const PYTHON = flag('--python') ?? process.env.HERTZFORGE_PYTHON ?? (process.platform === 'win32' ? 'python' : 'python3');
const HEADED = args.includes('--headed');
fs.mkdirSync(SHOTS, { recursive: true });
const DATA_ROOT = flag('--data') ? path.resolve(flag('--data')) : fs.mkdtempSync(path.join(os.tmpdir(), 'hertzforge-msine-data-'));
fs.mkdirSync(DATA_ROOT, { recursive: true });
const RECORD = { operator: 'Bench operator', article: 'Four-shaker plate' };
const OBJECTS = ['Channel Table', 'inputs', 'drives', 'Specification', 'Sine Levels', 'identification_frf', 'identification_coherence', 'Run Record'];

const children = [];
const stopAll = () => {
	for (const c of children) if (c.exitCode === null) c.kill('SIGTERM');
};
process.on('exit', stopAll);
process.on('SIGINT', () => process.exit(130));

function freePort() {
	return new Promise((resolve, reject) => {
		const s = net.createServer();
		s.unref();
		s.on('error', reject);
		s.listen(0, '127.0.0.1', () => {
			const { port } = s.address();
			s.close(() => resolve(port));
		});
	});
}

function start(name, cmd, argv, opts = {}) {
	const child = spawn(cmd, argv, { cwd: ROOT, stdio: ['ignore', 'pipe', 'pipe'], ...opts });
	const log = [];
	const keep = (b) => {
		log.push(b.toString());
		if (log.length > 200) log.shift();
	};
	child.stdout.on('data', keep);
	child.stderr.on('data', keep);
	child.on('exit', (code) => {
		if (code !== 0 && code !== null) console.error(`${name} exited ${code}\n${log.join('')}`);
	});
	children.push(child);
	return { child, log };
}

async function waitFor(what, test, ms = 60_000) {
	const t0 = Date.now();
	for (;;) {
		try {
			if (await test()) return;
		} catch {
			/* not yet */
		}
		if (Date.now() - t0 > ms) throw new Error(`timed out waiting for ${what}`);
		await new Promise((r) => setTimeout(r, 200));
	}
}

const results = [];
function check(name, ok, detail = '') {
	results.push({ name, ok, detail });
	console.log(`${ok ? 'pass' : 'FAIL'}  ${name}${detail ? `  (${detail})` : ''}`);
	if (!ok) throw new Error(`check failed: ${name} ${detail}`);
}

const tid = (page, id) => page.locator(`[data-testid="${id}"]`).first();
const attr = (page, id, name) => tid(page, id).getAttribute(name);
const text = async (page, id) => ((await tid(page, id).textContent()) ?? '').replace(/\s+/g, ' ').trim();
const until = (page, what, fn, ms) => waitFor(what, () => fn(page), ms);
const runState = (page) => attr(page, 'msine-run-view', 'data-state');
const runPhase = (page) => attr(page, 'msine-run-view', 'data-phase');
const hzNow = async (page) => Number((await attr(page, 'msine-run-view', 'data-hz')) || NaN);

async function shot(page, name) {
	const file = path.join(SHOTS, `mimo_sine_sim_${name}.png`);
	await page.evaluate(() => document.activeElement instanceof HTMLElement && document.activeElement.blur());
	// A note stays until it is dismissed; it would cover the plots in the picture.
	const note = page.locator('[data-testid="note"] button[aria-label="Dismiss"]');
	if (await note.count()) await note.first().click().catch(() => {});
	await page.waitForTimeout(300);
	await page.screenshot({ path: file });
	console.log(`      screenshot ${file}`);
}

/** The run view's page, by its tab's name. */
async function pageTo(page, name) {
	await page.locator('[data-testid="page-tabs"]').getByRole('tab', { name, exact: true }).click();
	await until(page, `the ${name} page`, async (p) => (await p.locator('[data-testid="page-tabs"] [role="tab"][aria-selected="true"]').first().innerText()).trim() === name);
}

async function fill(page, selector, value) {
	const l = page.locator(selector).first();
	await l.fill(String(value));
	await l.blur();
}

async function section(page, id) {
	if ((await attr(page, 'flow-setup', 'aria-current')) !== 'step' || !(await tid(page, `tab-${id}`).isVisible())) await tid(page, 'flow-setup').click();
	await tid(page, `tab-${id}`).click();
}

/** The channel table over the wire: four controls, the article's two responses, then four drives. */
async function setTable(url) {
	const ws = new WebSocket(url);
	const replies = new Map();
	let next = 1;
	await new Promise((resolve, reject) => {
		ws.onopen = resolve;
		ws.onerror = reject;
	});
	ws.onmessage = (m) => {
		if (typeof m.data !== 'string') return;
		const v = JSON.parse(m.data);
		if (v.type === 'response') replies.get(v.id)?.(v);
	};
	const call = (method, params = {}) =>
		new Promise((resolve, reject) => {
			const id = next++;
			replies.set(id, (v) => (v.error ? reject(new Error(`${method}: ${JSON.stringify(v.error)}`)) : resolve(v.result)));
			ws.send(JSON.stringify({ type: 'request', id, method, params }));
		});
	await call('session.claim', { name: 'Table' });
	await call('station.select', { kind: 'sim' });
	const { revision } = await call('channels.get');
	const row = (index, role) => ({
		index, enabled: true, description: role === 'drive' ? `Shaker ${index - 6}` : role === 'monitor' ? `Article ${index - 4}` : `Base ${index}`, role, physical: null,
		interface: 'single_ended', iepe_bias_v: null, units: role === 'drive' ? 'V' : 'g', sensitivity_mv_per_eu: 1000,
		range_max: role === 'drive' ? 10 : 1000, polarity: 'positive', control_group: null,
		abort: { accel_g: null, velocity_ips: null, displacement_mil: null }
	});
	await call('channels.set', { revision, channels: [1, 2, 3, 4].map((i) => row(i, 'control')).concat([5, 6].map((i) => row(i, 'monitor')), [7, 8, 9, 10].map((i) => row(i, 'drive'))) });
	await call('session.release');
	ws.close();
}

// ---- the services -----------------------------------------------------------------------------
let exe = flag('--exe');
if (!exe) {
	console.log('building the control service (release)');
	execFileSync('cargo', ['build', '--release', '-q', '-p', 'sds-control', '--locked'], { cwd: ROOT, stdio: 'inherit' });
	const TARGET = process.env.CARGO_TARGET_DIR ? path.resolve(process.env.CARGO_TARGET_DIR) : path.join(ROOT, 'target');
	exe = path.join(TARGET, 'release', process.platform === 'win32' ? 'sds-control.exe' : 'sds-control');
}
const [controlPort, analysisPort, shellPort] = [await freePort(), await freePort(), await freePort()];
const control = start('sds-control', exe, ['--port', String(controlPort)], { env: { ...process.env, HERTZFORGE_DATA_ROOT: DATA_ROOT } });
await waitFor('the control service', () => control.log.join('').includes('listening on ws://127.0.0.1:'), 30_000);
start('analysis', PYTHON, ['-m', 'hertzforge', 'serve', '--port', String(analysisPort)]);
await waitFor('the analysis service', () => new Promise((resolve) => {
	const s = net.connect(analysisPort, '127.0.0.1', () => {
		s.end();
		resolve(true);
	});
	s.on('error', () => resolve(false));
}), 60_000);
start('shell', process.platform === 'win32' ? 'npm.cmd' : 'npm', ['--prefix', 'ui', 'run', 'dev', '--', '--port', String(shellPort), '--strictPort'], { shell: process.platform === 'win32' });
await waitFor('the shell dev server', async () => (await fetch(`http://127.0.0.1:${shellPort}/`)).ok, 60_000);
await setTable(`ws://127.0.0.1:${controlPort}/`);
const URL_ = `http://127.0.0.1:${shellPort}/?control=ws://127.0.0.1:${controlPort}/&analysis=ws://127.0.0.1:${analysisPort}/&theme=light#acquire`;

const browser = await chromium.launch({ channel: 'chrome', headless: !HEADED });
let failed = null;
let page = null;
try {
	const a = (page = await (await browser.newContext({ viewport: { width: 1440, height: 1000 } })).newPage());
	// ---- 1. the setup ------------------------------------------------------------------------------
	await a.goto(URL_);
	await until(a, 'the chooser', async (p) => (await attr(p, 'conn-control', 'data-status')) === 'open' && (await tid(p, 'test-type-chooser').isVisible()));
	await tid(a, 'test-type-mimo_sine').click();
	await until(a, 'the MIMO Sine screen', async (p) => tid(p, 'mimo-sine-screen').isVisible());
	await tid(a, 'claim').click();
	await until(a, 'control', async (p) => (await attr(p, 'session-role', 'data-role')) === 'controller');
	await section(a, 'channels');
	await until(a, 'the drives', async (p) => tid(p, 'msine-drives').isVisible());
	for (let i = 0; i < 4; i++) {
		await fill(a, `[data-testid="msine-drive-ceiling-${i}"]`, 2);
		await fill(a, `[data-testid="msine-drive-abort-${i}"]`, 4);
	}
	await tid(a, 'msine-structure').selectOption('sine-base-article-6x4');
	await shot(a, 'channels');
	await section(a, 'references');
	await fill(a, '#msine-abort-lo-0', 6);
	await fill(a, '#msine-abort-hi-0', 6);
	await tid(a, 'msine-copy-1').click();
	await section(a, 'test');
	await tid(a, 'msine-kind-sweep').check();
	await fill(a, '#msine-lo', 150);
	await fill(a, '#msine-hi', 350);
	await fill(a, '#msine-rate', 2);
	await a.locator('#msine-end').selectOption('sweeps');
	await fill(a, '#msine-sweeps', 1);
	await section(a, 'schedule');
	const levels = a.locator('[data-testid="msine-levels"] button[aria-label^="Remove level"]');
	while ((await levels.count()) > 0 && (await a.locator('[data-testid="msine-levels"] tbody tr').count()) > 1) await levels.first().click();
	await fill(a, '#msine-id-resolution', 0.25);
	// A field with a default is off until its box is cleared.
	const framesBox = a.locator('#msine-id-frames').locator('xpath=ancestor::*[.//input[@type="checkbox"]][1]//input[@type="checkbox"]').first();
	if (await framesBox.isChecked()) await framesBox.uncheck();
	await fill(a, '#msine-id-frames', 8);
	await section(a, 'limits');
	for (const ch of ['5', '6']) {
		await tid(a, 'msine-limit-pick').selectOption(ch);
		await tid(a, 'msine-limit-add').click();
		for (const i of [0, 1]) await fill(a, `[data-testid="msine-bound-${ch}-${i}-value"]`, 0.6);
	}
	check('the article\'s two responses are limit channels', (await tid(a, 'msine-limit-5').isVisible()) && (await tid(a, 'msine-limit-6').isVisible()));
	await shot(a, 'limits');
	await section(a, 'law');
	await until(a, 'the law list', async (p) => tid(p, 'law-list').isVisible());
	const why = await text(a, 'msine-refused-secant');
	check('a law that cannot run a sweep is listed apart, with why', /secant/.test(why) && /sweep/i.test(why), why);
	await shot(a, 'law');
	await section(a, 'channels');
	await fill(a, '#msine-operator', RECORD.operator);
	await fill(a, '#msine-article', RECORD.article);
	await until(a, 'the setup checked clean by the control service', async (p) => (await attr(p, 'msine-ready', 'data-ready')) === 'true', 60_000);
	await shot(a, 'setup');

	// ---- 2. identification, the review, the run ---------------------------------------------------
	await tid(a, 'proceed-to-run').click();
	await until(a, 'the setup applied', async (p) => (await attr(p, 'flow-run', 'aria-current')) === 'step', 30_000);
	await tid(a, 'act-start').click();
	await until(a, 'the review hold', async (p) => (await runState(p)) === 'review', 300_000);
	const ident = await text(a, 'msine-ident-words');
	check('the review shows the identification in words', ident.length > 0, ident);
	await tid(a, 'msine-ack').fill('Reviewed the identification.');
	await shot(a, 'review');
	await tid(a, 'review-proceed').click();
	await until(a, 'the tone engaged', async (p) => ['engaged', 'full_level'].includes(await runPhase(p)) && Number.isFinite(await hzNow(p)), 60_000);
	const front = async (p) => (await p.locator('[data-testid="page-tabs"] [role="tab"][aria-selected="true"]').first().innerText()).trim();
	await until(a, 'the Controls page in front', async (p) => (await front(p)) === 'Controls', 10_000).catch(() => {});
	check('proceeding from the review brings the Controls page forward', (await front(a)) === 'Controls', await front(a));
	const f0 = await hzNow(a);
	await until(a, 'the frequency moving', async (p) => (await hzNow(p)) !== f0, 30_000);
	check('the frequency readout moves', true);
	await shot(a, 'running');
	await until(a, 'the sweep past 200 Hz', async (p) => (await hzNow(p)) > 200 || (await runState(p)) === 'finished', 120_000);
	const bands = await a.locator('[data-testid="msine-page-view"]').innerText();
	check('each control is drawn with its alarm and abort bands', /Alarm high/.test(bands) && /Abort low/.test(bands));
	await shot(a, 'running_controls');
	// The article's limit channels take the notch in near its 230 Hz resonance.
	let notch = '';
	await until(a, 'the notch in', async (p) => /Reference held down|Drive cut/.test((notch = await text(p, 'msine-notch'))) || (await runState(p)) === 'finished', 120_000);
	check('the limit channels take the notch in, and the card says how', /Reference held down|Drive cut/.test(notch), notch);
	await pageTo(a, 'Limits');
	await shot(a, 'notch');
	await pageTo(a, 'Controls');
	await shot(a, 'notch_controls');
	await until(a, 'the run finished', async (p) => (await runState(p)) === 'finished', 300_000);
	const end = await text(a, 'msine-end');
	check('the sweep completes', /Completed|complete/.test(end), end);
	const notched = await text(a, 'msine-report-limits');
	check('the finished run gives the time the notch was in per limit channel', /Limit 5/.test(notched) && /notch in [\d.]+ s/.test(notched), notched);
	const swept = await text(a, 'run-sweep');
	check('the finished one-sweep run reads 1 of 1', /\b1 of 1$/.test(swept.trim()), swept);
	await shot(a, 'finished');

	// ---- 3. the run folder and Open in Analysis -----------------------------------------------------
	await until(a, 'the end line', async (p) => /^Recorded in/.test(await text(p, 'record-line')), 30_000);
	const folder = await text(a, 'record-path');
	const manifest = JSON.parse(fs.readFileSync(path.join(folder, 'run.json'), 'utf8'));
	check('run.json keeps the operator and the article', manifest.operator === RECORD.operator && manifest.article === RECORD.article, JSON.stringify({ operator: manifest.operator, article: manifest.article }));
	check('run.json says the run is complete and simulated', manifest.complete === true && manifest.source === 'SIM' && manifest.test === 'mimo_sine');
	await tid(a, 'act-open-analysis').click();
	await until(a, 'the project tree in Analysis', async (p) => tid(p, 'project-tree').isVisible(), 180_000);
	const rail = await a.locator('[data-testid^="rail-"][aria-current="page"]').evaluateAll((els) => els.map((x) => x.dataset.testid));
	check('Open in Analysis leaves Acquire', rail.length > 0 && !rail.includes('rail-acquire'), rail.join(','));
	await a.waitForTimeout(800);
	const names = await a.locator('[data-testid="project-tree"] [data-testid^="object-"]').evaluateAll((els) => els.map((e) => e.dataset.testid.slice('object-'.length)));
	await shot(a, 'in_analysis');
	const missing = OBJECTS.filter((n) => !names.includes(n));
	check('the finished run opens in the analysis side with what the run saves', missing.length === 0, `listed: ${names.join(', ')}${missing.length ? `; missing: ${missing.join(', ')}` : ''}`);
} catch (err) {
	failed = err;
	console.error(err);
	if (page) await shot(page, 'stopped').catch(() => {});
} finally {
	await browser.close();
	stopAll();
}
const passed = results.filter((r) => r.ok).length;
console.log(`\n${passed} of ${results.length} checks passed${failed ? `; stopped: ${failed.message}` : ''}.`);
process.exit(failed ? 1 : 0);
