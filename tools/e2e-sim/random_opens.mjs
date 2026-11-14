// A finished Random run opened in Analysis, end to end: the real control service, the real
// analysis service and the shell, driven through the installed Chrome the way an operator drives
// them, with every check made on what the screen shows and on the run folder the run left.
//
//   npm --prefix tools/e2e-sim ci
//   node tools/e2e-sim/random_opens.mjs [--python <python with hertzforge installed>] [--shots <dir>] [--headed]
//
// It builds and starts `sds-control` (release, loopback, a free port, its data folder a new
// temporary folder), the analysis service (`python -m hertzforge serve`, a free port) and the
// shell's dev server, runs one short Random run on the simulator, and stops all three. It takes
// about a minute once the control service is built. It is not part of `npm --prefix ui test`.
//
// 1. One screen takes control, sets a channel table (a control input and a drive), a flat profile,
//    a short automatic schedule without the review hold, and the run's record: operator, test
//    article, the specification's document and clause, and notes. Recording time histories is on,
//    and the size an hour of them takes is shown beside it.
// 2. It arms: the Run card names the run folder, under the service's data folder. It starts, and
//    the run completes.
// 3. The run folder holds the operator's words in run.json, and its inputs and drives streams.
// 4. Open in Analysis: the analysis service converts the folder and opens it as the project; the
//    shell is in the Random workspace, and the project tree holds the run's channel table, its
//    time histories, its specification and its run record, selected.

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
const SHOTS = path.resolve(flag('--shots') ?? path.join(os.tmpdir(), 'hertzforge-e2e-opens'));
const PYTHON = flag('--python') ?? process.env.HERTZFORGE_PYTHON ?? (process.platform === 'win32' ? 'python' : 'python3');
const HEADED = args.includes('--headed');
fs.mkdirSync(SHOTS, { recursive: true });
const DATA_ROOT = fs.mkdtempSync(path.join(os.tmpdir(), 'hertzforge-opens-data-'));

const RECORD = { operator: 'Bench operator', article: 'Loopback fixture', document: 'Bench plan', clause: 'Run 1', notes: 'Flat profile on the simulator.' };

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
const stateOf = (page) => tid(page, 'run-state').getAttribute('data-state');
const roleOf = (page) => tid(page, 'session-role').getAttribute('data-role');
const text = async (page, id) => ((await tid(page, id).textContent()) ?? '').replace(/\s+/g, ' ').trim();
const until = (page, what, fn, ms) => waitFor(what, () => fn(page), ms);

async function shot(page, name) {
	const file = path.join(SHOTS, `${name}.png`);
	await page.screenshot({ path: file });
	console.log(`      screenshot ${file}`);
}

async function fill(page, selector, value) {
	const el = page.locator(selector).first();
	await el.fill(String(value));
	await el.blur();
}

async function verb(page, id, ms = 30_000) {
	const b = tid(page, id);
	await waitFor(`${id} enabled`, async () => (await b.isVisible()) && (await b.getAttribute('aria-disabled')) === 'false', ms);
	await b.click();
}

async function tab(page, id) {
	if ((await tid(page, 'flow-setup').getAttribute('aria-current')) !== 'step') await tid(page, 'flow-setup').click();
	await tid(page, `tab-${id}`).click();
}

// ---- the run ----------------------------------------------------------------------------------
console.log('building the control service (release)');
execFileSync('cargo', ['build', '--release', '-q', '-p', 'sds-control', '--locked'], { cwd: ROOT, stdio: 'inherit' });
const TARGET = process.env.CARGO_TARGET_DIR ? path.resolve(process.env.CARGO_TARGET_DIR) : path.join(ROOT, 'target');
const exe = path.join(TARGET, 'release', process.platform === 'win32' ? 'sds-control.exe' : 'sds-control');
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
const URL_ = `http://127.0.0.1:${shellPort}/?control=ws://127.0.0.1:${controlPort}/&analysis=ws://127.0.0.1:${analysisPort}/&theme=light#acquire`;
await waitFor('the shell dev server', async () => (await fetch(`http://127.0.0.1:${shellPort}/`)).ok, 60_000);
console.log(`control ws://127.0.0.1:${controlPort}/ (data ${DATA_ROOT}), analysis ws://127.0.0.1:${analysisPort}/, shell ${URL_}`);

const browser = await chromium.launch({ channel: 'chrome', headless: !HEADED });
let failed = null;
try {
	const a = await (await browser.newContext({ viewport: { width: 1440, height: 1000 } })).newPage();
	await a.goto(URL_);
	await until(a, 'the chooser', async (p) => (await tid(p, 'conn-control').getAttribute('data-status')) === 'open' && (await tid(p, 'test-type-chooser').isVisible()));
	await until(a, 'the analysis service', async (p) => (await tid(p, 'conn-analysis').getAttribute('data-status')) === 'open');
	await tid(a, 'test-type-random').click();
	await until(a, 'the shell to connect', async (p) => (await roleOf(p)) === 'observer');
	await tid(a, 'claim').click();
	await until(a, 'control', async (p) => (await roleOf(p)) === 'controller');

	// 1. Station and table: the simulator, a control input and a drive.
	await tab(a, 'station');
	await tid(a, 'station-sim').click();
	if ((await a.locator('[data-testid="station-use"]').count()) > 0) await tid(a, 'station-use').click();
	await until(a, 'the simulator in use', async (p) => /simulat/i.test(await text(p, 'station-current')));
	await fill(a, '#ch-0-sens', 1000);
	await fill(a, '#ch-0-range', 5);
	await fill(a, '#ch-1-range', 1);
	await until(a, 'the table check', async (p) => (await tid(p, 'table-apply').isEnabled()));
	await tid(a, 'table-apply').click();
	await until(a, 'the table applied', async (p) => /applied \(revision \d+\)/.test(await text(p, 'note')));

	// A flat profile, a short schedule, and the run's record.
	await tab(a, 'test');
	await fill(a, '[data-testid="setup-title"]', 'Opened in Analysis');
	while ((await a.locator('[data-testid^="bp-row-"]').count()) > 2) {
		const n = await a.locator('[data-testid^="bp-row-"]').count();
		await a.getByRole('button', { name: `Remove row ${n}`, exact: true }).click();
	}
	await fill(a, '#bp-0-f', 20);
	await fill(a, '#bp-0-v', '2e-5');
	await tid(a, 'bp-1-psd').click();
	await fill(a, '#bp-1-f', 2000);
	await fill(a, '#bp-1-v', '2e-5');
	await fill(a, '#acq-fs', 12800);
	await a.locator('#acq-npts').selectOption('2048');
	await fill(a, '#acq-navg', 4);
	await tid(a, 'schedule-auto').click();
	await fill(a, '#lv-0-dw', 2);
	await fill(a, '#lv-1-dw', 2);
	await fill(a, '#lv-full', 4);
	await fill(a, '[data-testid="rec-operator"]', RECORD.operator);
	await fill(a, '[data-testid="rec-article"]', RECORD.article);
	await fill(a, '[data-testid="rec-document"]', RECORD.document);
	await fill(a, '[data-testid="rec-clause"]', RECORD.clause);
	await fill(a, '[data-testid="rec-notes"]', RECORD.notes);
	check('Run Record: time histories are on by default', await tid(a, 'rec-streams').isChecked());
	const size = await text(a, 'rec-size');
	// two channels (the control input and the drive) at 12800 Hz, four bytes a sample: 369 MB an hour
	check('Run Record: the size an hour of time histories takes', /^About 369 MB per hour: 2 channels/.test(size), size);
	await tid(a, 'record-panel').scrollIntoViewIfNeeded();
	await shot(a, '01-run-record');

	await tab(a, 'control');
	const review = tid(a, 'ctl-review');
	if (await review.isChecked()) await review.uncheck({ force: true });
	await fill(a, '#ctl-drive_ceiling_vrms', 0.3);
	await fill(a, '#ctl-pre-max', 0.2);
	await tab(a, 'test');
	await until(a, 'the dry run', async (p) => /Checked by the control service/.test(await text(p, 'setup-check')), 15_000);

	// 2. Apply, arm: the Run card names the run folder. Start, and the run completes.
	await verb(a, 'act-apply');
	await until(a, 'applied', async (p) => (await tid(p, 'setup-status').getAttribute('data-status')) === 'applied');
	await tid(a, 'proceed-to-run').click();
	await until(a, 'the Run step', async (p) => tid(p, 'run-view').isVisible());
	await verb(a, 'act-arm');
	await until(a, 'armed', async (p) => (await stateOf(p)) === 'armed');
	await until(a, 'the recording line', async (p) => tid(p, 'record-line').isVisible());
	const folder = await text(a, 'record-path');
	const name = await text(a, 'record-folder');
	check('Arm: the Run card names the run folder under the data folder', path.dirname(folder) === fs.realpathSync(DATA_ROOT) || path.dirname(folder) === DATA_ROOT, folder);
	check('Arm: the folder is named for the time it armed, the test and the run', /^\d{8}-\d{6}_random_/.test(name) && folder.endsWith(name), name);
	check('Arm: the line says it is recording', /^Recording to/.test(await text(a, 'record-line')), await text(a, 'record-line'));
	await verb(a, 'act-start');
	await until(a, 'driving', async (p) => (await stateOf(p)) === 'running', 30_000);
	await shot(a, '02-recording');
	await until(a, 'the run to finish', async (p) => (await stateOf(p)) === 'finished', 120_000);
	const outcome = await tid(a, 'run-end').getAttribute('data-outcome');
	check('the run completed', outcome === 'completed', outcome);
	check('Finished: the line says where the run was recorded', /^Recorded in/.test(await text(a, 'record-line')));

	// 3. The run folder.
	const manifest = JSON.parse(fs.readFileSync(path.join(folder, 'run.json'), 'utf8'));
	check('run.json keeps the operator, the article, the notes and the specification', manifest.operator === RECORD.operator && manifest.article === RECORD.article && manifest.notes === RECORD.notes && manifest.specification?.document === RECORD.document && manifest.specification?.clause === RECORD.clause, JSON.stringify({ operator: manifest.operator, article: manifest.article, specification: manifest.specification }));
	check('run.json says the time histories are recorded, and the run is complete', manifest.recording?.streams === 'on' && manifest.complete === true, JSON.stringify(manifest.recording));
	check('the folder holds the inputs and drives streams', fs.existsSync(path.join(folder, 'streams', 'inputs', 'telemetry.f32')) && fs.existsSync(path.join(folder, 'streams', 'drives', 'telemetry.f32')));

	// 4. Open in Analysis.
	await until(a, 'the end actions', async (p) => tid(p, 'run-end-actions').isVisible());
	await shot(a, '03-end-actions');
	await tid(a, 'act-open-analysis').click();
	await until(a, 'the Random workspace', async (p) => (await tid(p, 'rail-random').getAttribute('aria-current')) === 'page', 120_000);
	await until(a, 'the project tree', async (p) => tid(p, 'project-tree').isVisible());
	const objects = await a.locator('[data-testid="project-tree"] [data-testid^="object-"]').evaluateAll((els) => els.map((e) => ({ name: e.dataset.testid.slice('object-'.length), selected: e.getAttribute('aria-selected') === 'true' })));
	const names = objects.map((o) => o.name);
	check('the project tree holds the run\'s channel table, time histories, specification and run record', ['Channel Table', 'inputs', 'drives', 'Specification', 'Run Record'].every((n) => names.includes(n)), names.join(', '));
	check('the run\'s objects are selected', objects.filter((o) => o.selected).length === objects.length, `${objects.filter((o) => o.selected).length} of ${objects.length}`);
	check('the project came from the run folder\'s converted file', fs.readdirSync(folder).some((f) => f.endsWith('.hforge')), fs.readdirSync(folder).join(', '));
	await shot(a, '04-in-analysis');

	// Back on the run, the action now shows the run in Analysis again.
	await tid(a, 'rail-acquire').click();
	await until(a, 'the run again', async (p) => tid(p, 'open-analysis-status').isVisible());
	check('back on the run: it says the run was opened', (await tid(a, 'open-analysis-status').getAttribute('data-status')) === 'done' && (await text(a, 'act-open-analysis')) === 'Show in Analysis', await text(a, 'open-analysis-status'));
} catch (err) {
	failed = err;
} finally {
	await browser.close();
	stopAll();
}

const passed = results.filter((r) => r.ok).length;
console.log(`\n${passed} of ${results.length} checks passed${failed ? `; stopped: ${failed.message}` : ''}`);
console.log(`screenshots in ${SHOTS}`);
process.exit(failed ? 1 : 0);
