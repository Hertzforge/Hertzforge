// The Recorder on the simulator, end to end: the real control service and the shell, driven through
// the installed Chrome the way an operator drives them, every check made on what the screen shows.
//
//   npm --prefix tools/e2e-sim ci
//   node tools/e2e-sim/recorder_sim.mjs [--shots <dir>] [--headed] [--mock]
//
// It builds and starts `sds-control` (release, loopback, a free port) and the shell's dev server
// (a free port), runs the Recorder on the simulated station, and stops both. With `--mock` it starts
// only the shell, against its own mock control service, to try the flow without a build. The run
// takes a few minutes. Not part of `npm test`.
//
// 1. The chooser offers the Recorder; its screen opens on Setup, and the screen takes control. The
//    SIM badge is checked at every step: a simulated run never shows LIVE.
// 2. Station & Channels: four recorded rows and the drive row, which is named as not opened; the
//    simulator's stated source is named.
// 3. Rate & Trigger and Analysis: a level trigger on row 1 at 0.25 g with a count of 3, and an FRF
//    pair; the setup checks clean, with the service's derived numbers and "Outputs: none".
// 4. Proceed to Run applies; Start: the card says "Monitoring: nothing is being saved".
// 5. The Time and Spectra pages move on the station's clock.
// 6. Arm: the card says what the trigger waits for; three recordings appear, one at each marker,
//    and the run goes back to monitoring. The Recordings page lists them with their facts.
// 7. The FRF page draws the pair; the Readings page the run in numbers; a page opened in a window of
//    its own follows the run.
// 8. Stop: every recording saved, and the card names where.
// 9. Manual: Arm, Record, Cut by hand, each answered by the service in station frames.
// 10. A second tab observes and aborts.
// 11. The controlling tab is closed during a recording: the run goes on, and stops with every
//     recording saved after the countdown.
//
// Screenshots (1,440 by 900): `<prefix>_<name>.png`, the dark theme `<prefix>_dark_<name>.png`.

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
const SHOTS = path.resolve(flag('--shots') ?? path.join(os.tmpdir(), 'hertzforge-e2e-recorder'));
const HEADED = args.includes('--headed');
const MOCK = args.includes('--mock');
const PREFIX = flag('--prefix') ?? 'recorder';
/** Run seconds per real second: the mock runs at 4, the service in real time. */
const SPEED = MOCK ? 4 : 1;
fs.mkdirSync(SHOTS, { recursive: true });

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
const text = async (page, id) => ((await tid(page, id).textContent()) ?? '').replace(/\s+/g, ' ').trim();
const attr = (page, id, name) => tid(page, id).getAttribute(name);
const until = (page, what, fn, ms) => waitFor(what, () => fn(page), ms);
const shots = [];

async function shot(page, name) {
	const file = path.join(SHOTS, `${PREFIX}_${name}.png`);
	await page.evaluate(() => document.activeElement instanceof HTMLElement && document.activeElement.blur());
	await page.waitForTimeout(400);
	await page.screenshot({ path: file });
	const kb = Math.round(fs.statSync(file).size / 1024);
	shots.push({ file, kb });
	console.log(`      screenshot ${file} (${kb} kB)`);
}

async function fill(locator, value) {
	await locator.fill(String(value));
	await locator.blur();
}

/** A simulated run never shows LIVE: checked at every step of the flow. */
async function neverLive(page, where) {
	const live = await page.locator('[data-testid="live-badge"]').count();
	check(`no LIVE badge (${where})`, live === 0);
}

/** A setup section, from the Setup step. */
async function section(page, id) {
	if (!(await tid(page, `tab-${id}`).isVisible())) await tid(page, 'flow-setup').click();
	await tid(page, `tab-${id}`).click();
	await page.locator('#recorder-panel').evaluate((el) => el.scrollTo({ top: 0 }));
}

/** The page a window shows, by its tab under the plots. */
const pageName = async (page) => (await page.locator('[data-testid="page-tabs"] [role="tab"][aria-selected="true"]').first().innerText()).trim();
/** A run page by its name, from the page tabs under the plots. */
async function showPage(page, name) {
	await page.locator('[data-testid="page-tabs"]').getByRole('tab', { name, exact: true }).click();
	await until(page, `the ${name} page`, async (p) => (await pageName(p)) === name);
}
/** The page on screen in a window of its own, from the page's menu. */
async function popout(page) {
	await tid(page, 'page-menu').click();
	const [win] = await Promise.all([ctx.waitForEvent('page'), tid(page, 'page-popout').click()]);
	return win;
}

const view = (page, name) => attr(page, 'recorder-run-view', name);
const num = async (page, name) => Number((await view(page, name)) || NaN);

// ---- the run ----------------------------------------------------------------------------------
let servicePort = null;
if (!MOCK) {
	console.log('building the control service (release)');
	execFileSync('cargo', ['build', '--release', '-q', '-p', 'sds-control', '--locked'], { cwd: ROOT, stdio: 'inherit' });
	const TARGET = process.env.CARGO_TARGET_DIR ? path.resolve(process.env.CARGO_TARGET_DIR) : path.join(ROOT, 'target');
	servicePort = await freePort();
	const exe = path.join(TARGET, 'release', process.platform === 'win32' ? 'sds-control.exe' : 'sds-control');
	const service = start('sds-control', exe, ['--port', String(servicePort)]);
	await waitFor('the control service', () => service.log.join('').includes('listening on ws://127.0.0.1:'), 30_000);
}
const shellPort = await freePort();
start('shell', process.platform === 'win32' ? 'npm.cmd' : 'npm', ['--prefix', 'ui', 'run', 'dev', '--', '--port', String(shellPort), '--strictPort'], {
	shell: process.platform === 'win32'
});
const QUERY = MOCK ? `?mock=1&mock_speed=${SPEED}` : `?control=ws://127.0.0.1:${servicePort}/`;
const URL_ = (theme) => `http://127.0.0.1:${shellPort}/${QUERY}&theme=${theme}#acquire`;
await waitFor('the shell dev server', async () => (await fetch(`http://127.0.0.1:${shellPort}/`)).ok, 60_000);
console.log(`shell ${URL_('light')}${MOCK ? ' (mock control service)' : `, service ws://127.0.0.1:${servicePort}/`}`);

const browser = await chromium.launch({ channel: 'chrome', headless: !HEADED });
const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
let failed = null;

async function openRecorder(page, claim) {
	await page.goto(URL_('light'));
	await until(page, 'the chooser or the screen', async (p) => (await attr(p, 'conn-control', 'data-status')) === 'open' && ((await tid(p, 'test-type-chooser').isVisible()) || (await tid(p, 'recorder-screen').isVisible())));
	if (await tid(page, 'test-type-chooser').isVisible()) await tid(page, 'test-type-recorder').click();
	await until(page, 'the Recorder screen', async (p) => tid(p, 'recorder-screen').isVisible());
	if (claim) {
		await tid(page, 'claim').click();
		await until(page, 'control', async (p) => (await attr(p, 'session-role', 'data-role')) === 'controller');
	}
}

/** Apply the setup from the Setup step and start the run; the card then says what it does. */
async function proceedAndStart(page) {
	await tid(page, 'proceed-to-run').click();
	await until(page, 'the setup applied', async (p) => (await attr(p, 'flow-run', 'aria-current')) === 'step' && (await attr(p, 'run-state', 'data-state')) === 'configured', 30_000);
	await tid(page, 'act-start').click();
	await until(page, 'acquisition', async (p) => (await view(p, 'data-state')) === 'running', 30_000);
}

try {
	const a = await ctx.newPage();
	await a.goto(URL_('light'));
	await until(a, 'the chooser', async (p) => (await attr(p, 'conn-control', 'data-status')) === 'open' && (await tid(p, 'test-type-chooser').isVisible()));
	const offered = await a.locator('[data-testid="test-type-chooser"] [data-type]').evaluateAll((els) => els.filter((e) => e.getAttribute('aria-disabled') === 'false').map((e) => e.dataset.type));
	check('the chooser offers the Recorder', offered.includes('recorder'), offered.join());
	await openRecorder(a, true);
	check('the Recorder opens on Setup', (await attr(a, 'flow-setup', 'aria-current')) === 'step');
	await neverLive(a, 'setup');

	// ---- Station & Channels: four recorded rows, the drive row left alone ------------------------
	await section(a, 'station');
	for (let k = 0; k < 3; k++) await tid(a, 'add-input').click();
	// row 1 (the markers) at a 5 g range, so the level trigger's usual hysteresis is 0.05 g
	await fill(a.locator('#ch-0-range'), 5);
	await until(a, 'the table check', async (p) => tid(p, 'table-apply').isEnabled(), 10_000);
	await tid(a, 'table-apply').click();
	await until(a, 'the table applied', async (p) => !(await tid(p, 'table-edited').isVisible().catch(() => false)), 20_000);
	const rowsWords = await text(a, 'recorder-run-channels');
	check('the run names its recorded rows and the drive row it leaves alone', /channels 1, 3, 4, 5/.test(rowsWords) && /Drive row 2 is not opened and not recorded/.test(rowsWords), rowsWords);
	const source = await text(a, 'recorder-sim-source');
	check("the simulator's stated source is named, row by row", /Row 1: markers/.test(source) && /Row 3: 250 Hz/.test(source) && /Row 5: 1000 Hz/.test(source), source.slice(0, 160));
	await shot(a, 'station');

	// ---- Rate & Trigger: a level trigger on the markers --------------------------------------------
	await section(a, 'trigger');
	await tid(a, 'recorder-mode-level').click();
	await a.locator('[data-testid="recorder-row"]').selectOption('1');
	await fill(tid(a, 'recorder-level'), 0.25);
	await fill(tid(a, 'recorder-pre'), 0.5);
	await fill(tid(a, 'recorder-length'), 1);
	const countOff = tid(a, 'recorder-count-off');
	if (await countOff.isChecked()) await countOff.uncheck();
	await fill(tid(a, 'recorder-count'), 3);
	check('the trigger is set: row 1 rising through 0.25 g, 0.5 s before and 1.0 s after, three a time', /^Level, row 1 rising through 0\.25 g, pre-window 0\.5 s, 1 s, 3 a time$/.test(await text(a, 'recorder-summary-trigger')), await text(a, 'recorder-summary-trigger'));

	// ---- Analysis: an FRF pair ---------------------------------------------------------------------
	await section(a, 'analysis');
	await tid(a, 'recorder-frf-on').check();
	await a.locator('[data-testid="recorder-frf-reference"]').selectOption('3');
	await a.locator('[data-testid="recorder-frf-response"]').selectOption('4');
	await until(a, 'the setup checked clean', async (p) => (await attr(p, 'recorder-ready', 'data-ready')) === 'true', 30_000);
	const grid = await text(a, 'recorder-derived-grid');
	check("the setup checks clean, with the service's derived numbers beside it", /^12800 Hz, 256 frames a block, 512 a batch/.test(grid) && /6\.25 Hz lines, 800 lines to 5000 Hz/.test(await text(a, 'recorder-derived-analysis')), `${grid}; ${await text(a, 'recorder-derived-analysis')}`);
	check('the service states the outputs as none', /^None/.test(await text(a, 'recorder-derived-outputs')), await text(a, 'recorder-derived-outputs'));
	check('the pre-window and length in frames are the service\'s', /^Pre-window 6400 frames; 12800 frames after the trigger/.test(await text(a, 'recorder-derived-trigger')), await text(a, 'recorder-derived-trigger'));
	await shot(a, 'setup');

	// ---- Run: monitoring ---------------------------------------------------------------------------
	await proceedAndStart(a);
	await until(a, 'monitoring', async (p) => (await text(p, 'recorder-status')) === 'Monitoring: nothing is being saved', 20_000);
	check('Start: the card says monitoring saves nothing', true);
	check('the Run card has no shared Arm and the header no level in dB', (await tid(a, 'act-arm').count()) === 0 && (await tid(a, 'run-level').count()) === 0);
	check('the Run card offers Arm, and in level mode no Record and no Cut', (await tid(a, 'act-rec-arm').isVisible()) && (await tid(a, 'act-rec-record').count()) === 0 && (await tid(a, 'act-rec-cut').count()) === 0);
	await neverLive(a, 'monitoring');

	// ---- The Time and Spectra pages move -------------------------------------------------------------
	await showPage(a, 'Time');
	await until(a, 'the time history', async (p) => (await p.locator('[data-testid="recorder-time-plot"]').count()) > 0, 20_000);
	const t0 = await num(a, 'data-t');
	await until(a, 'the station clock to run on', async (p) => (await num(p, 'data-t')) > t0 + 1, 20_000);
	check('the Time page draws the rows as the station clock runs', true, `station time ${t0} then ${await num(a, 'data-t')} s`);
	const xShown = async (p) => [Number(await attr(p, 'recorder-time-plot', 'data-x0')), Number(await attr(p, 'recorder-time-plot', 'data-x1'))];
	await until(a, 'the time axis to follow', async (p) => {
		const [x0, x1] = await xShown(p);
		const t = await num(p, 'data-t');
		return x1 > t - 0.5 && x1 <= t + 0.01 && x1 - x0 > 1.9 && x1 - x0 < 2.1;
	}, 10_000);
	check('the time axis follows station time: the last 2 s up to now', true, `${(await xShown(a)).map((v) => v.toFixed(2)).join(' to ')} s at ${await num(a, 'data-t')} s`);
	await shot(a, 'time');
	await showPage(a, 'Spectra');
	await until(a, 'the spectra', async (p) => /Channel 1/.test(await text(p, 'recorder-page-view')) && !/drawn once/.test(await text(p, 'recorder-page-view')), 20_000);
	check('the Spectra page draws every recorded row once the first average is complete', true);

	// ---- Arm the level trigger: three recordings at three markers -----------------------------------
	await showPage(a, 'Recordings');
	await tid(a, 'act-rec-arm').click();
	await until(a, 'armed', async (p) => /^Armed: (holding off [\d.]+ s, then )?waiting for row 1 to rise through 0\.250 g \(pre-window [\d.]+ s of 0\.50 s\)$/.test(await text(p, 'recorder-status')), 10_000);
	check('Arm: the card says what the trigger waits for and how full the pre-window is', true, await text(a, 'recorder-status'));
	check('the service answers the arm with its pre-window', /^Armed: the pre-window of 0\.50 s fills from the next batch$/.test(await text(a, 'recorder-answer')), await text(a, 'recorder-answer'));
	await until(a, 'three recordings', async (p) => (await view(p, 'data-recordings')) === '3' && (await view(p, 'data-phase')) === 'monitoring', 60_000);
	check('three recordings are saved, one at each marker, and the run goes back to monitoring', (await view(a, 'data-fires')) === '3', `fires ${await view(a, 'data-fires')}`);
	await until(a, 'the recordings listed', async (p) => (await attr(p, 'recorder-recordings', 'data-count')) === '3', 10_000);
	const frames = [];
	for (const n of ['rec_0001', 'rec_0002', 'rec_0003']) frames.push(Number(await attr(a, `recorder-recording-${n}`, 'data-frames')));
	check('each re-armed recording holds its 0.5 s pre-window and exactly 1.0 s after its trigger', frames[1] === 19200 && frames[2] === 19200 && frames[0] <= 19200, frames.join(', '));
	check('the card names the last recording saved', /^Saved rec_0003: 1\.50 s, no gaps$/.test(await text(a, 'recorder-saved')), await text(a, 'recorder-saved'));
	await shot(a, 'recordings');

	// ---- The FRF and Readings pages; a page in a window of its own ------------------------------------
	await showPage(a, 'FRF');
	await until(a, 'the FRF', async (p) => /Coherence/.test(await text(p, 'recorder-page-view')), 20_000);
	check('the FRF page draws the pair: magnitude, phase and coherence', /FRF/.test(await text(a, 'recorder-page-view')) && /Phase/.test(await text(a, 'recorder-page-view')));
	await shot(a, 'frf');
	await showPage(a, 'Readings');
	await until(a, 'the readings', async (p) => /Group delay/.test(await text(p, 'recorder-page-view')), 20_000);
	check('the Readings page shows the run in numbers and its marks', /Free above floor/.test(await text(a, 'recorder-page-view')) && (await attr(a, 'recorder-mark-list', 'data-count')) === '0');
	await showPage(a, 'Time');
	const win = await popout(a);
	await until(win, 'the display window', async (p) => tid(p, 'recorder-run-view').isVisible(), 30_000);
	await until(win, 'the window to read the run', async (p) => Number.isFinite(await num(p, 'data-t')), 30_000);
	const w0 = await num(win, 'data-t');
	await until(win, 'the window to follow the run', async (p) => (await num(p, 'data-t')) > w0 + 1, 30_000);
	check('a page opened in a window of its own follows the run', true, `station time ${w0} then ${await num(win, 'data-t')} s`);
	check('the window keeps Stop and Abort', (await tid(win, 'act-stop').isVisible()) && (await tid(win, 'act-abort').isVisible()));
	await win.close();

	// ---- Stop: every recording saved, and where --------------------------------------------------------
	await tid(a, 'act-stop').click();
	await until(a, 'the end', async (p) => (await attr(p, 'run-state', 'data-outcome')) === 'operator_stop', 20_000);
	const where = await text(a, 'recorder-end-where');
	check('Stop: the card names the recordings and their folder', /^Stopped: 3 recordings in \S*recorder$/.test(where), where);
	await neverLive(a, 'after the stop');

	// ---- Manual: Arm, Record and Cut by hand ----------------------------------------------------------
	await tid(a, 'flow-setup').click();
	await section(a, 'trigger');
	await tid(a, 'recorder-mode-manual').click();
	const lenOff = tid(a, 'recorder-length-off');
	if (!(await lenOff.isChecked())) await lenOff.check();
	await until(a, 'the setup checked clean', async (p) => (await attr(p, 'recorder-ready', 'data-ready')) === 'true', 30_000);
	await proceedAndStart(a);
	check('a manual run offers Arm and Record, and Cut waits for a recording', (await tid(a, 'act-rec-arm').isVisible()) && (await tid(a, 'act-rec-record').isVisible()) && (await attr(a, 'act-rec-cut', 'aria-disabled')) === 'true');
	await tid(a, 'act-rec-arm').click();
	await until(a, 'the pre-window full', async (p) => (await text(p, 'recorder-status')) === 'Armed: Record begins with the pre-window (pre-window 0.50 s of 0.50 s)', 20_000);
	check('armed by hand, the card says Record begins with the pre-window it holds', true);
	await tid(a, 'act-rec-record').click();
	await until(a, 'the record answer', async (p) => /^rec_0001 began at station frame \d+, with 0\.50 s before it$/.test(await text(p, 'recorder-answer')), 10_000);
	check('Record is answered with its station frame and the pre-window it took', true, await text(a, 'recorder-answer'));
	await until(a, 'a recording until cut', async (p) => /^Recording rec_0001: [\d.]+ s, until cut$/.test(await text(p, 'recorder-status')) && Number((await text(p, 'recorder-status')).match(/([\d.]+) s/)[1]) > 1, 20_000);
	check('the card counts the recording until cut', true, await text(a, 'recorder-status'));
	await shot(a, 'recording');
	await tid(a, 'act-rec-cut').click();
	await until(a, 'the cut', async (p) => /^rec_0001 ends at station frame \d+$/.test(await text(p, 'recorder-answer')) && (await view(p, 'data-recordings')) === '1', 10_000);
	check('Cut is answered with the last station frame, and the recording is saved', true, await text(a, 'recorder-answer'));
	await until(a, 'monitoring again', async (p) => (await view(p, 'data-phase')) === 'monitoring', 10_000);
	check('a manual recording returns the run to monitoring, not armed', (await tid(a, 'act-rec-arm').isVisible()) && (await tid(a, 'act-rec-disarm').count()) === 0);
	await tid(a, 'act-stop').click();
	await until(a, 'the end', async (p) => (await attr(p, 'run-state', 'data-outcome')) === 'operator_stop', 20_000);
	check('the second run keeps its own folder', /^Stopped: 1 recording in \S*recorder-2$/.test(await text(a, 'recorder-end-where')), await text(a, 'recorder-end-where'));

	// ---- An observer aborts; the controlling tab is closed ----------------------------------------------
	// Both need one service shared by two tabs: the shell's mock lives in each tab, so `--mock` skips them.
	if (MOCK) {
		console.log('skip  the observer abort and the closed controlling tab: they need the control service');
	} else {
		await tid(a, 'act-reset').click();
		await until(a, 'a new run', async (p) => (await attr(p, 'run-state', 'data-state')) === 'configured', 20_000);
		await tid(a, 'act-start').click();
		await until(a, 'acquisition', async (p) => (await view(p, 'data-state')) === 'running', 30_000);
		const b = await ctx.newPage();
		await openRecorder(b, false);
		await tid(b, 'flow-run').click().catch(() => {});
		await until(b, 'the observer sees the run', async (p) => (await attr(p, 'run-state', 'data-state')) === 'running', 30_000);
		check('a second tab observes', (await attr(b, 'session-role', 'data-role')) === 'observer');
		check('the observer is told to take control to record', (await attr(b, 'act-rec-record', 'aria-disabled')) === 'true');
		await tid(b, 'act-abort').click();
		await until(a, 'the abort', async (p) => (await attr(p, 'run-state', 'data-outcome')) === 'operator_abort', 20_000);
		check('an observer tab aborts the run, every recording saved', /^Aborted: /.test(await text(a, 'recorder-end-where')), await text(a, 'recorder-end-where'));

		// ---- The controlling tab is closed during a recording --------------------------------------------
		await tid(a, 'act-reset').click();
		await until(a, 'a new run', async (p) => (await attr(p, 'run-state', 'data-state')) === 'configured', 20_000);
		await tid(a, 'act-start').click();
		await until(a, 'acquisition', async (p) => (await view(p, 'data-state')) === 'running', 30_000);
		await tid(a, 'act-rec-record').click();
		await until(a, 'a recording', async (p) => (await view(p, 'data-phase')) === 'recording', 10_000);
		await a.close();
		await until(b, 'the countdown', async (p) => tid(p, 'orphan').isVisible(), 20_000);
		check('with nobody in control the observer sees the countdown', true, await text(b, 'orphan'));
		const s0 = await num(b, 'data-t');
		await until(b, 'the run to go on', async (p) => (await num(p, 'data-t')) > s0 + 2 && (await view(p, 'data-phase')) === 'recording', 20_000);
		check('the run and its recording go on with nobody in control', true);
		await shot(b, 'orphan');
		await until(b, 'the safe stop', async (p) => (await attr(p, 'run-state', 'data-outcome')) === 'safe_stop', 90_000);
		check('the run stops with every recording saved', /^Stopped: 1 recording in /.test(await text(b, 'recorder-end-where')), await text(b, 'recorder-end-where'));
		await neverLive(b, 'after the safe stop');
	}

	// ---- The dark theme ---------------------------------------------------------------------------
	const d = await ctx.newPage();
	await d.goto(URL_('dark'));
	await until(d, 'the screen', async (p) => (await tid(p, 'recorder-screen').isVisible()) || (await tid(p, 'test-type-chooser').isVisible()));
	if (await tid(d, 'test-type-chooser').isVisible()) await tid(d, 'test-type-recorder').click();
	await until(d, 'the Recorder screen', async (p) => tid(p, 'recorder-screen').isVisible());
	await section(d, 'trigger');
	await shot(d, 'dark_setup');
	await tid(d, 'flow-run').click();
	await until(d, 'the run view', async (p) => tid(p, 'recorder-run-view').isVisible(), 20_000);
	await d.waitForTimeout(1000);
	await shot(d, 'dark_run');
} catch (err) {
	failed = err;
	console.error(err);
} finally {
	await browser.close();
	stopAll();
}

const passed = results.filter((r) => r.ok).length;
console.log(`\n${passed} of ${results.length} checks passed; ${shots.length} screenshots in ${SHOTS}`);
process.exit(failed ? 1 : 0);
