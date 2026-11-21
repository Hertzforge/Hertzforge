// MIMO Mixed Mode against the real control service on the simulator, and the finished run opened in
// the analysis side, driven through the installed Chrome the way an operator drives them, every
// check made on what the screen shows and on the run folder the run left.
//
//   npm --prefix tools/e2e-sim ci
//   node tools/e2e-sim/mimo_mixed_sim.mjs [--exe <sds-control>] [--python <python with hertzforge>]
//                                         [--data <dir>] [--shots <dir>] [--headed]
//
// Without `--exe` it builds `sds-control` (release) and runs that. It starts the control service
// (loopback, a free port, its data folder `--data`, else a new temporary folder), the analysis
// service and the shell's dev server, and stops all three. Not part of `npm --prefix ui test`.
//
// 0. The channel table is set to the simulator's four-by-four structure over the wire before the
//    screen opens: controls 1 to 4, drives 5 to 8.
// 1. MIMO Mixed Mode is chosen in the chooser and takes control. Every setup section in turn, as
//    an operator states the test: each drive's rms and peak ceilings; a flat floor of
//    0.001 g²/Hz from 20 to 1600 Hz at every control, with a coherence between them; a tone that
//    stands at 150 Hz, a tone that sweeps 400 to 440 Hz at 0.5 Hz/s, a narrowband of 0.003 g²/Hz
//    added over 40 Hz at 300 Hz (its level shown whole in its field); a manual schedule of -6, -3
//    and 0 dB; the laws and the rules; the run's record. The service's dry run finds nothing missing.
// 2. Start identifies; the review hold gives the gate in words and the prediction with every drive
//    against its ceilings; Proceed. The floor equalizes alone, the parts come in, the Controls page
//    draws every control, the parts, drives and limits cards fill. The sweeping tone is switched
//    off and on again; the level is stepped up a rung, then taken to full level; Hold stops the
//    parts' clock and Resume starts it; the level card gives the time at full level and what is
//    left of it; the run completes after its time at full level and nothing plays after the end.
// 3. The run folder is complete and holds what docs/run-folder.md says; Open in Analysis opens it
//    as a project with the objects a converted run has.
// 4. A second run from the same setup with a coherence gate no identification passes: the review
//    holds at the failed gate, its worst coherence read under the gate, the log's kinds in words,
//    Proceed waits, an override with the operator's words releases it
//    (once more if the prediction holds), then Abort ends the run at once. The folder keeps the
//    override's words.
//
// Screenshots (1,440 by 1,000): `mimo_mixed_sim_<name>.png`.

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
const SHOTS = path.resolve(flag('--shots') ?? path.join(os.tmpdir(), 'hertzforge-e2e-mimo-mixed-sim'));
const PYTHON = flag('--python') ?? process.env.HERTZFORGE_PYTHON ?? (process.platform === 'win32' ? 'python' : 'python3');
const HEADED = args.includes('--headed');
fs.mkdirSync(SHOTS, { recursive: true });
const DATA_ROOT = flag('--data') ? path.resolve(flag('--data')) : fs.mkdtempSync(path.join(os.tmpdir(), 'hertzforge-mmix-data-'));
fs.mkdirSync(DATA_ROOT, { recursive: true });
const RECORD = { operator: 'Bench operator', article: 'Four-shaker plate' };
const OBJECTS = ['Channel Table', 'inputs', 'drives', 'Specification', 'identification_frf', 'identification_coherence', 'Run Record'];
const RESULTS = ['reference', 'identification_frf', 'identification_coherence', 'identification_response', 'identification_drive', 'identification', 'control_spectra', 'drive_spectra', 'tone_levels', 'band_levels', 'drive_levels', 'control_trace'];
const OVERRIDE_WORDS = 'Simulator plant; the gate is set past what any identification reaches, on purpose.';

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
const runState = (page) => attr(page, 'mmix-run-view', 'data-state');
const runPhase = (page) => attr(page, 'mmix-run-view', 'data-phase');
const runLevel = async (page) => Number((await attr(page, 'mmix-run-view', 'data-level')) || NaN);

async function shot(page, name) {
	const file = path.join(SHOTS, `mimo_mixed_sim_${name}.png`);
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
const fillId = (page, id, value) => fill(page, `[data-testid="${id}"]`, value);

async function section(page, id) {
	if ((await attr(page, 'flow-setup', 'aria-current')) !== 'step' || !(await tid(page, `tab-${id}`).isVisible())) await tid(page, 'flow-setup').click();
	await tid(page, `tab-${id}`).click();
}

/** The channel table over the wire: four controls, then four drives, the simulator's 4 by 4. */
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
		index, enabled: true, description: role === 'drive' ? `Shaker ${index - 4}` : `Plate ${index}`, role, physical: null,
		interface: 'single_ended', iepe_bias_v: null, units: role === 'drive' ? 'V' : 'g', sensitivity_mv_per_eu: 1000,
		range_max: role === 'drive' ? 10 : 1000, polarity: 'positive', control_group: null,
		abort: { accel_g: null, velocity_ips: null, displacement_mil: null }
	});
	await call('channels.set', { revision, channels: [1, 2, 3, 4].map((i) => row(i, 'control')).concat([5, 6, 7, 8].map((i) => row(i, 'drive'))) });
	await call('session.release');
	ws.close();
}

/** The results a run folder holds, by name (a result is a folder or a file of that name). */
function resultNames(folder) {
	const dir = path.join(folder, 'results');
	if (!fs.existsSync(dir)) return [];
	return fs.readdirSync(dir).map((n) => n.replace(/\.[^.]+$/, ''));
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
	const ctx = await browser.newContext({ viewport: { width: 1440, height: 1000 } });
	const a = (page = await ctx.newPage());
	const pageErrors = [];
	a.on('pageerror', (e) => pageErrors.push(e.message));
	// ---- 1. the setup ------------------------------------------------------------------------------
	await a.goto(URL_);
	await until(a, 'the chooser', async (p) => (await attr(p, 'conn-control', 'data-status')) === 'open' && (await tid(p, 'test-type-chooser').isVisible()));
	await tid(a, 'test-type-mimo_mixed').click();
	await until(a, 'the MIMO Mixed screen', async (p) => tid(p, 'mimo-mixed-screen').isVisible());
	await tid(a, 'claim').click();
	await until(a, 'control', async (p) => (await attr(p, 'session-role', 'data-role')) === 'controller');

	// Station & Channels: the table's four controls and four drives, each drive's two ceilings.
	await section(a, 'channels');
	await until(a, 'the drives', async (p) => (await p.locator('[data-testid^="mmix-drive-"][data-testid$="-0"]').count()) > 0 && tid(p, 'mmix-drives').isVisible());
	const controlsOn = await a.locator('[data-testid^="mmix-control-"]:checked').count();
	const driveRows = await a.locator('[data-testid="mmix-drives"] tbody tr').count();
	check('the table\'s four control rows are controls and its four drive rows drives', controlsOn === 4 && driveRows === 4, `${controlsOn} controls, ${driveRows} drives`);
	for (let i = 0; i < 4; i++) {
		await fillId(a, `mmix-drive-rms-${i}`, 2);
		await fillId(a, `mmix-drive-peak-${i}`, 6);
	}
	const vpk = await text(a, 'mmix-drive-vpk-default-0');
	check('an empty peak abort says the value it runs with', /6\.7\d* V/.test(vpk) && /plus 1 dB/.test(vpk), vpk);
	await shot(a, 'channels');

	// Floor Reference: one flat table, copied to every control, and a coherence between them.
	await section(a, 'reference');
	await until(a, 'the breakpoint table', async (p) => tid(p, 'breakpoint-table').isVisible());
	await tid(a, 'bp-1-psd').click();
	await tid(a, 'bp-3-psd').click();
	for (const i of [0, 1, 2, 3]) await fillId(a, `bp-${i}-value`, 0.001);
	await fillId(a, 'bp-3-freq', 1600);
	await tid(a, 'mmix-ref-copy-all').click();
	await fillId(a, 'mmix-ref-coherence', 0.25);
	await until(a, 'the reference the service built', async (p) => /from 20 to 1600 Hz/.test(await text(p, 'mmix-ref-preview')), 60_000);
	check('the service builds the floor from the tables, its band 20 to 1600 Hz', true, (await text(a, 'mmix-ref-preview')).slice(0, 90));
	await shot(a, 'reference');

	// Parts: the standing tone, a slow sweep and an added narrowband.
	await section(a, 'parts');
	await until(a, 'the tones table', async (p) => tid(p, 'mmix-tones-table').isVisible());
	await fillId(a, 'mmix-tones-0-freq', 150);
	await tid(a, 'mmix-tones-0-phases').uncheck();
	await tid(a, 'mmix-tones-add').click();
	await until(a, 'a second tone', async (p) => tid(p, 'mmix-tones-row-1').isVisible());
	await tid(a, 'mmix-tones-1-kind').selectOption('sweep');
	await tid(a, 'mmix-tones-1-law').selectOption('linear');
	await fillId(a, 'mmix-tones-1-lo', 400);
	await fillId(a, 'mmix-tones-1-hi', 440);
	await fillId(a, 'mmix-tones-1-rate', 0.5);
	await tid(a, 'mmix-tones-1-phases').uncheck();
	for (let k = 0; k < 4; k++) await fillId(a, `mmix-tones-1-amp-${k}`, 0.5);
	await tid(a, 'mmix-bands-add').click();
	await until(a, 'a narrowband', async (p) => tid(p, 'mmix-bands-row-0').isVisible());
	await fillId(a, 'mmix-bands-0-freq', 300);
	await fillId(a, 'mmix-bands-0-width', 40);
	await tid(a, 'mmix-bands-0-added').click();
	await fillId(a, 'mmix-bands-0-coh', 0.5);
	for (let k = 0; k < 4; k++) await fillId(a, `mmix-bands-0-lv-${k}`, 0.003);
	const lvFit = await a.locator('input[data-testid^="mmix-bands-0-lv-"]').evaluateAll((els) => els.map((e) => [e.value, e.scrollWidth <= e.clientWidth]));
	check('a narrowband\'s added level shows whole in its field', lvFit.length === 4 && lvFit.every(([, fits]) => fits), JSON.stringify(lvFit));
	const delay = await text(a, 'mmix-tracker-delay');
	check('the parts panel says how late the tracker reads a tone', /1\.25 s late/.test(delay), delay.slice(0, 80));
	await shot(a, 'parts');

	// Schedule & Identification: manual, -6, -3 and full level, 30 s at full level.
	await section(a, 'schedule');
	await tid(a, 'schedule-manual').click();
	const lv = a.locator('[data-testid="level-table"] tbody tr');
	await until(a, 'the level table', async () => (await lv.count()) > 0);
	await fill(a, '#lv-0-db', -6);
	await fill(a, '#lv-0-dw', 5);
	await fill(a, '#lv-1-db', -3);
	await fill(a, '#lv-1-dw', 5);
	await fillId(a, 'full-level-s', 30);
	await shot(a, 'schedule');

	// Laws: the two laws from the service's catalog and the three rules.
	await section(a, 'laws');
	await until(a, 'the law lists', async (p) => (await p.locator('[data-testid="law-list"]').count()) >= 2, 30_000);
	check('the ceiling rule offers Common Scale and In Turn', (await tid(a, 'mmix-ceiling-rule-common_scale').isChecked()) && (await tid(a, 'mmix-ceiling-rule-in_turn').count()) === 1);
	await shot(a, 'laws');

	// Limits & Protections, with the run's record.
	await section(a, 'limits');
	await fillId(a, 'rec-operator', RECORD.operator);
	await fillId(a, 'rec-article', RECORD.article);
	await shot(a, 'limits');

	// Prediction & Budget: before identification it says the prediction comes after.
	await section(a, 'forecast');
	await until(a, 'the setup checked clean by the control service', async (p) => (await attr(p, 'mmix-ready', 'data-ready')) === 'true', 60_000);
	check('the summary lists nothing missing', true, await text(a, 'mmix-ready'));
	await shot(a, 'forecast');

	// ---- 2. identification, the review, the run ---------------------------------------------------
	await tid(a, 'proceed-to-run').click();
	await until(a, 'the setup applied', async (p) => (await attr(p, 'flow-run', 'aria-current')) === 'step', 30_000);
	await tid(a, 'act-start').click();
	await until(a, 'the review hold', async (p) => (await runState(p)) === 'review' || (await runState(p)) === 'finished', 300_000);
	check('the run holds for review after identification', (await runPhase(a)) === 'awaiting_review', `${await runState(a)} ${await runPhase(a)} ${await text(a, 'run-end').catch(() => '')}`);
	const gate = await text(a, 'mmix-gate-words');
	check('the review gives the gate in words', /coherence gate passed/.test(gate) && /identified at -?\d/.test(gate), gate);
	const predDrives = await a.locator('[data-testid="mmix-review-prediction-drives"] tbody tr td:first-child').allInnerTexts();
	check('the review gives the prediction for every drive, by its name, within its ceilings', predDrives.length === 4 && predDrives.every((n, j) => n.trim() === `Shaker ${j + 1}`) && (await attr(a, 'mmix-review-prediction', 'data-within')) === 'true', predDrives.join(', '));
	const predControls = await a.locator('[data-testid="mmix-review-prediction-controls"] tbody tr td:first-child').allInnerTexts();
	check('and for every control, by its name', predControls.length === 4 && predControls.every((n, k) => n.trim() === `Plate ${k + 1}`), predControls.join(', '));
	await shot(a, 'review');
	await tid(a, 'review-proceed').click();
	await until(a, 'the floor engaged', async (p) => ['floor_only', 'engaged', 'full_level'].includes(await runPhase(p)), 60_000);
	check('proceeding engages the floor', true, await runPhase(a));
	await until(a, 'the parts in', async (p) => (await runPhase(p)) === 'engaged' || (await runState(p)) === 'finished', 120_000);
	check('the floor equalizes and the parts come in', (await runPhase(a)) === 'engaged', `${await runPhase(a)} ${await text(a, 'mmix-floor-state')}`);
	const front = async (p) => (await p.locator('[data-testid="page-tabs"] [role="tab"][aria-selected="true"]').first().innerText()).trim();
	check('the Controls page is in front', (await front(a)) === 'Controls', await front(a));
	await until(a, 'the control traces', async (p) => /alarm high/i.test(await p.locator('[data-testid="mmix-page-view"]').innerText()), 30_000);
	check('each control is drawn with its reference and its alarm and abort lines', /abort low/i.test(await a.locator('[data-testid="mmix-page-view"]').innerText()));
	await shot(a, 'running');

	// The parts card is on the Tones & Bands page: both tones and the narrowband play.
	await pageTo(a, 'Tones & Bands');
	const playing = (p) => p.locator('tr[data-testid^="mmix-part-"][data-on="true"]').count();
	await until(a, 'every part playing', async (p) => (await playing(p)) === 4, 30_000);
	check('the floor, both tones and the narrowband play (the run bar counts the parts, not the floor)', (await text(a, 'run-parts-on')) === '3', await text(a, 'run-parts-on'));

	// A part switched off and on again, on its ramp.
	await a.locator('label:has([data-testid="mmix-part-t2-switch"])').click();
	await until(a, 'the sweep off', async (p) => (await attr(p, 'mmix-part-t2', 'data-enabled')) === 'false' && (await attr(p, 'mmix-part-t2', 'data-on')) === 'false', 30_000);
	check('the sweeping tone switches off', true, await text(a, 'mmix-part-t2-state'));
	await shot(a, 'part_off');
	await a.locator('label:has([data-testid="mmix-part-t2-switch"])').click();
	await until(a, 'the sweep on', async (p) => (await attr(p, 'mmix-part-t2', 'data-enabled')) === 'true' && (await attr(p, 'mmix-part-t2', 'data-on')) === 'true', 30_000);
	check('and on again', true, await text(a, 'mmix-part-t2-state'));

	// The level, by hand: a rung up, then full level.
	const l0 = await runLevel(a);
	await tid(a, 'level-step').selectOption('3');
	await tid(a, 'level-up').click();
	await until(a, 'the level up a rung', async (p) => (await runLevel(p)) === l0 + 3, 30_000);
	check('the level steps up 3 dB', true, `${l0} to ${await runLevel(a)} dB; ${await text(a, 'mmix-level')}`);
	await tid(a, 'level-goto').selectOption('0');
	await until(a, 'full level', async (p) => (await runPhase(p)) === 'full_level', 60_000);
	check('the level goes to full level', (await runLevel(a)) === 0, await text(a, 'mmix-level'));
	await until(a, 'the time at full level', async (p) => /left/.test(await text(p, 'mmix-at-level').catch(() => '')), 15_000);
	check('the level card gives the time at full level and what is left of it', true, await text(a, 'mmix-at-level'));
	await pageTo(a, 'Controls');
	await a.waitForTimeout(3000);
	await shot(a, 'full_level');
	await pageTo(a, 'Drives');
	await until(a, 'the drives card', async (p) => tid(p, 'mmix-drives-table').isVisible(), 10_000);
	const scale = await text(a, 'mmix-scale');
	check('the drives card gives every drive and the common scale', (await a.locator('[data-testid^="mmix-drive-live-"]').count()) === 4 && scale.length > 0, scale);
	const limitsCard = await text(a, 'mmix-limits-card');
	check('the limits card says the setup has no limit channels', /no limit channels/.test(limitsCard), limitsCard);
	await shot(a, 'drives');
	await pageTo(a, 'Tones & Bands');
	await a.waitForTimeout(2500);
	await shot(a, 'tones');
	const partRows = await a.locator('[data-testid="mmix-parts-table"] tbody tr').evaluateAll((rows) => rows.map((r) => [...r.cells].map((c) => c.textContent.replace(/\s+/g, ' ').trim()).join(' | ')));
	console.log(`      parts at full level:\n        ${partRows.join('\n        ')}`);
	await pageTo(a, 'Controls');

	// Hold and Resume.
	const t0 = await text(a, 'mmix-t');
	await tid(a, 'act-hold').click();
	await until(a, 'the hold', async (p) => (await attr(p, 'mmix-run-view', 'data-held')) === 'true', 15_000);
	check('Hold stops the parts\' clock', true, t0);
	await shot(a, 'held');
	await a.waitForTimeout(2000);
	await tid(a, 'act-resume').click();
	await until(a, 'the resume', async (p) => (await attr(p, 'mmix-run-view', 'data-held')) === 'false', 15_000);
	check('Resume starts it again', true);

	await until(a, 'the run finished', async (p) => (await runState(p)) === 'finished', 300_000);
	const outcome = await attr(a, 'run-end', 'data-outcome');
	const end = await text(a, 'mmix-end');
	check('the run completes after its time at full level', outcome === 'completed', `${outcome}: ${end}`);
	check('nothing plays once the run has ended', (await text(a, 'run-parts-on')) === '0', await text(a, 'run-parts-on'));
	await shot(a, 'finished');

	// ---- 3. the run folder and Open in Analysis -----------------------------------------------------
	await until(a, 'the end line', async (p) => /^Recorded in/.test(await text(p, 'record-line')), 30_000);
	const folder = await text(a, 'record-path');
	const manifest = JSON.parse(fs.readFileSync(path.join(folder, 'run.json'), 'utf8'));
	check('run.json keeps the operator and the article', manifest.operator === RECORD.operator && manifest.article === RECORD.article, JSON.stringify({ operator: manifest.operator, article: manifest.article }));
	check('run.json says the run is complete and simulated', manifest.complete === true && manifest.source === 'SIM' && manifest.test === 'mimo_mixed', JSON.stringify({ complete: manifest.complete, source: manifest.source, test: manifest.test }));
	const missingFiles = ['table.json', 'setup.json', 'law/law.json', 'log.jsonl', 'end.json', 'streams/inputs', 'streams/drives'].filter((f) => !fs.existsSync(path.join(folder, f)));
	const have = resultNames(folder);
	const missingResults = RESULTS.filter((r) => !have.includes(r));
	check('the run folder holds every file and result a MIMO Mixed run saves', !missingFiles.length && !missingResults.length, `missing: ${[...missingFiles, ...missingResults].join(', ') || 'none'}`);
	const ended = JSON.parse(fs.readFileSync(path.join(folder, 'end.json'), 'utf8'));
	check('end.json says the run completed', ended.end?.outcome === 'completed', JSON.stringify(ended.end));
	// a mark on the page, to tell a page reload from the shell's own choice of screen
	await a.evaluate(() => {
		window.__mmixMark = true;
	});
	await tid(a, 'act-open-analysis').click();
	await until(a, 'the project tree in Analysis', async (p) => tid(p, 'project-tree').isVisible(), 180_000);
	const rail = await a.locator('[data-testid^="rail-"][aria-current="page"]').evaluateAll((els) => els.map((x) => x.dataset.testid));
	check('Open in Analysis leaves Acquire', rail.length > 0 && !rail.includes('rail-acquire'), rail.join(','));
	await a.waitForTimeout(800);
	const names = await a.locator('[data-testid="project-tree"] [data-testid^="object-"]').evaluateAll((els) => els.map((e) => e.dataset.testid.slice('object-'.length)));
	await shot(a, 'in_analysis');
	const missing = OBJECTS.filter((n) => !names.includes(n));
	check('the finished run opens in the analysis side with what the run saves', missing.length === 0, `listed: ${names.join(', ')}${missing.length ? `; missing: ${missing.join(', ')}` : ''}`);

	// ---- 4. a review hold released with an override, then Abort -------------------------------------
	await tid(a, 'rail-acquire').click();
	await until(a, 'Acquire again', async (p) => (await tid(p, 'mimo-mixed-screen').isVisible()) || (await tid(p, 'test-type-chooser').isVisible()), 20_000);
	const reloaded = !(await a.evaluate(() => window.__mmixMark === true));
	const backOn = await tid(a, 'mimo-mixed-screen').isVisible();
	console.log(`      back in Acquire: ${backOn ? 'the MIMO Mixed screen' : 'the chooser'}; the page ${reloaded ? 'was reloaded' : 'was not reloaded'}`);
	check('Acquire comes back to the MIMO Mixed screen (or the page was reloaded)', backOn || reloaded);
	if (!backOn) {
		await tid(a, 'test-type-mimo_mixed').click();
		await until(a, 'the MIMO Mixed screen again', async (p) => tid(p, 'mimo-mixed-screen').isVisible(), 20_000);
	}
	await tid(a, 'flow-run').click();
	await until(a, 'New Run', async (p) => tid(p, 'act-reset').isVisible(), 20_000);
	await tid(a, 'act-reset').click();
	await until(a, 'a new run configured', async (p) => (await attr(p, 'run-state', 'data-state')) === 'configured' || (await runState(p)) === 'configured', 30_000);
	await section(a, 'schedule');
	await fillId(a, 'mmix-id-coherence', 0.999999);
	await section(a, 'forecast');
	await until(a, 'the changed setup checked clean', async (p) => (await attr(p, 'mmix-ready', 'data-ready')) === 'true', 60_000);
	await tid(a, 'proceed-to-run').click();
	await until(a, 'the setup applied again', async (p) => (await attr(p, 'flow-run', 'aria-current')) === 'step', 30_000);
	await tid(a, 'act-start').click();
	await until(a, 'the gate hold', async (p) => (await runState(p)) === 'review' || (await runState(p)) === 'finished', 300_000);
	check('a gate no identification passes holds the review at gate_failed', (await runPhase(a)) === 'gate_failed', await runPhase(a));
	check('Proceed waits for the override', (await attr(a, 'review-proceed', 'aria-disabled')) === 'true');
	const holdWords = await text(a, 'mmix-hold');
	check('the hold says what it waits for', /failed coherence gate/.test(holdWords), holdWords);
	const gate2 = await text(a, 'mmix-gate-words');
	const worst2 = Number(/worst band coherence is ([\d.]+)/.exec(gate2)?.[1]);
	check('the failed gate\'s worst coherence reads under the gate', worst2 < 0.999999, gate2);
	const kinds = await a.locator('[data-testid="mixed-log"] .k').allInnerTexts();
	check('the log names every kind in a word', kinds.length > 0 && kinds.every((k) => /^[A-Z]/.test(k.trim())), [...new Set(kinds)].join(', '));
	check('Override waits for the operator\'s words', (await attr(a, 'act-override-words', 'aria-disabled')) === 'true');
	await tid(a, 'mmix-ack').fill(OVERRIDE_WORDS);
	await until(a, 'Override enabled', async (p) => (await attr(p, 'act-override-words', 'aria-disabled')) === 'false', 5_000);
	await shot(a, 'override_hold');
	await tid(a, 'act-override-words').click();
	await until(a, 'past the gate', async (p) => (await runPhase(p)) !== 'gate_failed', 30_000);
	if ((await runPhase(a)) === 'prediction_exceeded') {
		await tid(a, 'mmix-ack').fill(OVERRIDE_WORDS);
		await tid(a, 'act-override-words').click();
	} else if ((await runPhase(a)) === 'awaiting_review') {
		await tid(a, 'review-proceed').click();
	}
	await until(a, 'the floor engaged past the override', async (p) => ['floor_only', 'engaged', 'full_level'].includes(await runPhase(p)), 60_000);
	check('the override releases the hold and the floor engages', true, await runPhase(a));
	await a.waitForTimeout(2000);
	await tid(a, 'act-abort').click();
	await until(a, 'the abort', async (p) => (await runState(p)) === 'finished', 30_000);
	const aborted = await attr(a, 'run-end', 'data-outcome');
	check('Abort ends the run at once', aborted === 'operator_abort', `${aborted}: ${await text(a, 'mmix-end')}`);
	await shot(a, 'aborted');
	await until(a, 'the second end line', async (p) => /^Recorded in/.test(await text(p, 'record-line')), 30_000);
	const folder2 = await text(a, 'record-path');
	check('a second run has a folder of its own', folder2 !== folder, folder2);
	const end2 = JSON.parse(fs.readFileSync(path.join(folder2, 'end.json'), 'utf8'));
	check('end.json keeps the override with the operator\'s words', (end2.overrides ?? []).some((o) => o.reason === 'gate_failed' && o.text === OVERRIDE_WORDS) && end2.end?.outcome === 'operator_abort', JSON.stringify(end2.overrides));
	check('the page raised no error', pageErrors.length === 0, pageErrors.join(' | '));
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
