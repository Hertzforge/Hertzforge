// Playback on the simulator, end to end: the real control service, the real analysis service and
// the shell, driven through the installed Chrome the way an operator drives them, every check made
// on what the screen shows and on the run folder the run left.
//
//   npm --prefix tools/e2e-sim ci
//   node tools/e2e-sim/playback_sim.mjs [--exe <sds-control>] [--python <python with hertzforge installed>]
//                                       [--data <dir>] [--shots <dir>] [--headed]
//
// Without `--exe` it builds `sds-control` (release) and runs that. It starts the control service
// (loopback, a free port, its data folder `--data`, else a new temporary folder), the analysis
// service (`python -m hertzforge serve`, a free port) and the shell's dev server, and stops all
// three. It writes two small CSV drives beside the data folder. Not part of `npm --prefix ui test`.
//
// 1. The chooser's Specialized section holds the Recorder, Explorer and Playback; Playback opens on
//    Setup, and the screen takes control on the simulator.
// 2. Drive File: a CSV by its full path is read (its signal, its rate from the time column) and
//    converted; the drive plays on the table's drive row.
// 3. Level & Limits and Check: at -6 dB under a 1 V ceiling the control service's check passes,
//    every channel at rest at both ends, with a preview of each drive (screenshot).
// 4. Refused: at 0 dB under a 0.4 V ceiling the check refuses the peak over the ceiling, and
//    Proceed to Run waits (screenshot).
// 5. Run: Proceed to Run, Start; the play's progress and the inputs and drives move (screenshot);
//    the run ends Completed (screenshot); the run folder holds the inputs, the drives, the check and
//    the outcome; Open in Analysis lands in Signals with the inputs and the drives in the project.
// 6. Fix Ends: a CSV whose ends are not at rest is refused on its ends; Fix Ends writes a copy and
//    shows each channel before and after (screenshot); played, the copy checks clean.
//
// Screenshots (1,440 by 1,000): `playback_<name>.png`.

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
const SHOTS = path.resolve(flag('--shots') ?? path.join(os.tmpdir(), 'hertzforge-e2e-playback'));
const PYTHON = flag('--python') ?? process.env.HERTZFORGE_PYTHON ?? (process.platform === 'win32' ? 'python' : 'python3');
const HEADED = args.includes('--headed');
fs.mkdirSync(SHOTS, { recursive: true });
const DATA_ROOT = flag('--data') ? path.resolve(flag('--data')) : fs.mkdtempSync(path.join(os.tmpdir(), 'hertzforge-playback-data-'));
fs.mkdirSync(DATA_ROOT, { recursive: true });

// ---- processes --------------------------------------------------------------------------------
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

// ---- checks -----------------------------------------------------------------------------------
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
const field = (page, id) => page.locator(`input[data-testid="${id}"], [data-testid="${id}"] input`).first();

async function shot(page, name) {
	const file = path.join(SHOTS, `playback_${name}.png`);
	await page.evaluate(() => document.activeElement instanceof HTMLElement && document.activeElement.blur());
	await page.waitForTimeout(400);
	await page.screenshot({ path: file });
	console.log(`      screenshot ${file}`);
}

async function fill(locator, value) {
	await locator.fill(String(value));
	await locator.blur();
}

async function section(page, id) {
	if ((await attr(page, 'flow-setup', 'aria-current')) !== 'step' || !(await tid(page, `tab-${id}`).isVisible())) await tid(page, 'flow-setup').click();
	await tid(page, `tab-${id}`).click();
	await page.locator('#playback-panel').evaluate((el) => el.scrollTo({ top: 0 }));
}

async function neverLive(page, where) {
	check(`no LIVE badge (${where})`, (await page.locator('[data-testid="live-badge"]').count()) === 0);
}

// ---- the drive files --------------------------------------------------------------------------
// A burst whose acceleration, velocity and displacement are at rest at both ends (the golden drive
// folder's form): A cos(2 pi m (t - T/2) / T) sin(pi t / T)^2, T = 1 s, m = 20, A = 0.5 V, at 2048 Hz.
const FS = 2048;
function writeDrive(name, offset) {
	const n = FS + 1;
	const lines = ['Time (s),Drive (V)'];
	for (let i = 0; i < n; i++) {
		const t = i / FS;
		const v = 0.5 * Math.cos((2 * Math.PI * 20 * (t - 0.5)) / 1) * Math.sin(Math.PI * t) ** 2 + offset;
		lines.push(`${t.toFixed(11)},${v.toFixed(8)}`);
	}
	const file = path.join(path.dirname(DATA_ROOT), `playback_${name}_${process.pid}.csv`);
	fs.writeFileSync(file, `${lines.join('\n')}\n`);
	return file;
}
const GOOD = writeDrive('burst', 0);
// the same burst on a 0.05 V offset: it starts and ends away from zero, and its velocity drifts
const BAD = writeDrive('offset', 0.05);

// ---- the services -----------------------------------------------------------------------------
let exe = flag('--exe');
if (!exe) {
	console.log('building the control service (release)');
	execFileSync('cargo', ['build', '--release', '-q', '-p', 'sds-control', '--locked'], { cwd: ROOT, stdio: 'inherit' });
	const TARGET = process.env.CARGO_TARGET_DIR ? path.resolve(process.env.CARGO_TARGET_DIR) : path.join(ROOT, 'target');
	exe = path.join(TARGET, 'release', process.platform === 'win32' ? 'sds-control.exe' : 'sds-control');
}
const [controlPort, analysisPort, shellPort] = [await freePort(), await freePort(), await freePort()];
const env = { ...process.env, HERTZFORGE_DATA_ROOT: DATA_ROOT };
const control = start('sds-control', exe, ['--port', String(controlPort)], { env });
await waitFor('the control service', () => control.log.join('').includes('listening on ws://127.0.0.1:'), 30_000);
start('analysis', PYTHON, ['-m', 'hertzforge', 'serve', '--port', String(analysisPort)], { env });
await waitFor('the analysis service', () => new Promise((resolve) => {
	const s = net.connect(analysisPort, '127.0.0.1', () => {
		s.end();
		resolve(true);
	});
	s.on('error', () => resolve(false));
}), 60_000);
start('shell', process.platform === 'win32' ? 'npm.cmd' : 'npm', ['--prefix', 'ui', 'run', 'dev', '--', '--port', String(shellPort), '--strictPort'], { shell: process.platform === 'win32' });
const URL_ = `http://127.0.0.1:${shellPort}/?control=ws://127.0.0.1:${controlPort}/&analysis=ws://127.0.0.1:${analysisPort}/&theme=light#acquire`;
await waitFor('the shell dev server', async () => (await fetch(`http://127.0.0.1:${shellPort}/`)).ok, 60_000);
console.log(`playback: control ws://127.0.0.1:${controlPort}/ (data ${DATA_ROOT}), analysis ws://127.0.0.1:${analysisPort}/, shell ${URL_}`);

const browser = await chromium.launch({ channel: 'chrome', headless: !HEADED });
let failed = null;
let page = null;

/** Read and convert a CSV by its path; resolves once the setup plays the new folder. */
async function convert(a, file) {
	await section(a, 'drive');
	if ((await attr(a, 'pb-from-path', 'aria-checked')) !== 'true') await tid(a, 'pb-from-path').click();
	await fill(tid(a, 'pb-path'), file);
	await tid(a, 'pb-read').click();
	await until(a, 'the signals', async (p) => (await attr(p, 'pb-signals', 'data-count')) === '1', 30_000);
	const before = await attr(a, 'pb-folder', 'data-folder');
	await tid(a, 'pb-convert').click();
	await until(a, 'the drive folder', async (p) => {
		const f = await attr(p, 'pb-folder', 'data-folder');
		return !!f && f !== before;
	}, 30_000);
	return attr(a, 'pb-folder', 'data-folder');
}

async function checked(a, pass) {
	await section(a, 'check');
	await until(a, `the check to ${pass ? 'pass' : 'refuse'}`, async (p) => (await attr(p, 'playback-check', 'data-pass')) === String(pass), 30_000);
}

try {
	const a = (page = await (await browser.newContext({ viewport: { width: 1440, height: 1000 } })).newPage());
	await a.goto(URL_);
	await until(a, 'the chooser', async (p) => (await attr(p, 'conn-control', 'data-status')) === 'open' && (await attr(p, 'conn-analysis', 'data-status')) === 'open' && (await tid(p, 'test-type-chooser').isVisible()), 60_000);

	// ---- 1. The chooser ---------------------------------------------------------------------------
	const specialized = await a.locator('[data-testid="test-group-specialized"] [data-type]').evaluateAll((els) => els.map((e) => e.dataset.type));
	check('the Specialized section holds the Recorder, Explorer and Playback', specialized.join() === 'recorder,explorer,playback', specialized.join());
	check('the section is named Specialized', ((await a.locator('#tg-specialized').textContent()) ?? '').trim() === 'Specialized');
	const single = await a.locator('[data-testid="test-group-single"] [data-type]').evaluateAll((els) => els.map((e) => e.dataset.type));
	check('Explorer is no longer under Single Output', !single.includes('explorer'), single.join());
	await tid(a, 'test-type-playback').click();
	await until(a, 'the Playback screen', async (p) => tid(p, 'playback-screen').isVisible());
	check('Playback opens on Setup', (await attr(a, 'flow-setup', 'aria-current')) === 'step');
	await tid(a, 'claim').click();
	await until(a, 'control', async (p) => (await attr(p, 'session-role', 'data-role')) === 'controller');
	await section(a, 'station');
	if (await tid(a, 'station-sim').count()) {
		await tid(a, 'station-sim').click();
		if ((await a.locator('[data-testid="station-use"]').count()) > 0) await tid(a, 'station-use').click();
		await until(a, 'the simulator in use', async (p) => /simulat/i.test(await text(p, 'station-current')));
	}
	await neverLive(a, 'setup');

	// ---- 2. The drive file --------------------------------------------------------------------------
	const folder1 = await convert(a, GOOD);
	check('the CSV converts into a drive folder under the data folder', folder1.startsWith(path.join(DATA_ROOT, 'drives')), folder1);
	check('the drive folder holds drive.bin and drive.json', fs.existsSync(path.join(folder1, 'drive.bin')) && fs.existsSync(path.join(folder1, 'drive.json')));
	const sidecar = JSON.parse(fs.readFileSync(path.join(folder1, 'drive.json'), 'utf8'));
	check("the drive is at the file's rate, every sample", sidecar.rate_hz === FS && sidecar.samples === FS + 1, `${sidecar.rate_hz} Hz, ${sidecar.samples} samples`);
	check('the drive plays on the drive row', (await tid(a, 'pb-map-0').inputValue()) === '2');

	// ---- 3. A check that passes --------------------------------------------------------------------
	await section(a, 'level');
	await fill(field(a, 'pb-level'), -6);
	await fill(field(a, 'pb-ceiling'), 1);
	await checked(a, true);
	check('every channel passes, at rest at both ends', (await attr(a, 'pb-check-0', 'data-pass')) === 'true', await text(a, 'pb-check-0'));
	check('each drive has its preview with its velocity and displacement', (await a.locator('[data-testid="pb-previews"] figure').count()) === 1);
	await until(a, 'the setup to check clean', async (p) => (await attr(p, 'playback-ready', 'data-ready')) === 'true', 20_000);
	await shot(a, 'setup_check_passes');

	// ---- 4. Refused: the peak over the ceiling --------------------------------------------------------
	await section(a, 'level');
	await fill(field(a, 'pb-level'), 0);
	await fill(field(a, 'pb-ceiling'), 0.4);
	await checked(a, false);
	const refused = await text(a, 'playback-check');
	check('the control service refuses the peak over the ceiling, naming the channel and the numbers', /channel 0 \(Drive\) peaks at 0\.5\d* V.*over the drive ceiling of 0\.4 V/i.test(refused), refused.slice(0, 240));
	check('Proceed to Run waits while the setup has errors', (await attr(a, 'proceed-to-run', 'aria-disabled')) === 'true');
	await shot(a, 'refused_peak_over_ceiling');
	await section(a, 'level');
	await fill(field(a, 'pb-level'), -6);
	await fill(field(a, 'pb-ceiling'), 1);
	await checked(a, true);

	// ---- 5. The run ----------------------------------------------------------------------------------
	await section(a, 'level');
	await fill(field(a, 'pb-pre'), 0.5);
	await fill(field(a, 'pb-post'), 1);
	await until(a, 'the setup to check clean', async (p) => (await attr(p, 'playback-ready', 'data-ready')) === 'true', 20_000);
	await tid(a, 'proceed-to-run').click();
	await until(a, 'the setup applied', async (p) => (await attr(p, 'flow-run', 'aria-current')) === 'step' && (await attr(p, 'run-state', 'data-state')) === 'configured', 30_000);
	await tid(a, 'act-start').click();
	let shotRunning = false;
	await until(a, 'the play', async (p) => {
		const phase = await attr(p, 'playback-run-view', 'data-phase');
		if (!shotRunning && phase === 'playing') {
			const played = Number(await attr(p, 'playback-run-view', 'data-played'));
			if (played > 0 && (await p.locator('[data-testid="playback-page-view"] canvas').count()) > 0) {
				await shot(p, 'run_in_progress');
				shotRunning = true;
			}
		}
		return (await attr(p, 'run-state', 'data-state')) === 'finished';
	}, 60_000);
	check('a screenshot was taken while the drive played', shotRunning);
	check('the run ends Completed', (await attr(a, 'run-state', 'data-outcome')) === 'completed', await text(a, 'run-end').catch(() => ''));
	check('every sample of the file was played', (await attr(a, 'playback-run-view', 'data-played')) === String(FS + 1), `${await attr(a, 'playback-run-view', 'data-played')}`);
	await until(a, 'the end actions', async (p) => tid(p, 'run-end-actions').isVisible(), 30_000);
	await shot(a, 'run_finished');
	await neverLive(a, 'after the run');
	const runs = fs.readdirSync(DATA_ROOT).filter((f) => /^playback/.test(f));
	check('the run left its folder under the data folder', runs.length === 1, runs.join(', '));
	const run = path.join(DATA_ROOT, runs[0]);
	const kept = fs.existsSync(path.join(run, 'results')) ? fs.readdirSync(path.join(run, 'results')) : [];
	const streams = fs.existsSync(path.join(run, 'streams')) ? fs.readdirSync(path.join(run, 'streams')) : [];
	check('the run folder holds the inputs, the drives, the check and the outcome', streams.sort().join() === 'drives,inputs' && kept.some((f) => f.startsWith('playback_check')) && kept.some((f) => f.startsWith('playback_outcome')), `${streams.join(', ')}; ${kept.join(', ')}`);
	const setup = JSON.parse(fs.readFileSync(path.join(run, 'setup.json'), 'utf8'));
	check("the run's setup holds the drive folder and its SHA-256", setup.config.drive.folder === folder1 && setup.config.drive.sha256 === sidecar.sha256);

	await tid(a, 'act-open-analysis').click();
	await until(a, 'the Signals workspace', async (p) => (await attr(p, 'rail-signals', 'aria-current')) === 'page', 180_000);
	await until(a, 'the project tree', async (p) => tid(p, 'project-tree').isVisible(), 30_000);
	await a.waitForTimeout(800);
	const names = await a.locator('[data-testid="project-tree"] [data-testid^="object-"]').evaluateAll((els) => els.map((e) => e.dataset.testid.slice('object-'.length)));
	check('Open in Analysis brings the table, the inputs, the drives and the run record', ['Channel Table', 'inputs', 'drives', 'Run Record'].every((n) => names.includes(n)), names.join(', '));
	await shot(a, 'in_analysis');
	await tid(a, 'rail-acquire').click();
	await until(a, 'the run again', async (p) => tid(p, 'open-analysis-status').isVisible(), 30_000);

	// ---- 6. Fix Ends ---------------------------------------------------------------------------------
	const folder2 = await convert(a, BAD);
	await checked(a, false);
	check('the offset drive is refused on its ends', (await attr(a, 'pb-check-0', 'data-pass')) === 'false' && (await attr(a, 'playback-check', 'data-fixable')) === 'true', await text(a, 'pb-check-0'));
	await shot(a, 'check_fails_ends');
	await tid(a, 'pb-fix-ends').click();
	await until(a, 'the fix', async (p) => tid(p, 'pb-fix-table').isVisible(), 30_000);
	check('the setup still plays the original until the fix is chosen', (await attr(a, 'playback-check', 'data-folder')) === folder2);
	await shot(a, 'fix_ends_before_after');
	await tid(a, 'pb-fix-use').click();
	await until(a, 'the fixed folder in the setup', async (p) => (await attr(p, 'playback-check', 'data-folder')) !== folder2, 10_000);
	const fixed = await attr(a, 'playback-check', 'data-folder');
	check('the fixed copy is a new folder beside the original, which is unchanged', fixed !== folder2 && path.dirname(fixed) === path.dirname(folder2) && fs.existsSync(path.join(folder2, 'drive.bin')), fixed);
	await checked(a, true);
	check('the fixed copy checks clean', (await attr(a, 'pb-check-0', 'data-pass')) === 'true', await text(a, 'pb-check-0'));
	const fixedSidecar = JSON.parse(fs.readFileSync(path.join(fixed, 'drive.json'), 'utf8'));
	check("the fix is kept in the copy's sidecar", fixedSidecar.fixes.length === 1 && fixedSidecar.fixes[0].kind === 'fix_ends', JSON.stringify(fixedSidecar.fixes.map((f) => f.kind)));
} catch (err) {
	failed = err;
	console.error(err);
	if (page) await shot(page, 'stopped').catch(() => {});
} finally {
	await browser.close();
	stopAll();
	for (const f of [GOOD, BAD]) fs.rmSync(f, { force: true });
}

const passed = results.filter((r) => r.ok).length;
console.log(`\nplayback: ${passed} of ${results.length} checks passed${failed ? `; stopped: ${failed.message}` : ''}`);
console.log(`screenshots in ${SHOTS}`);
process.exit(failed ? 1 : 0);
