// The MIMO Random run views on the simulator, end to end: the real control service running a
// 72-control, 24-drive simulated run, and the shell's run views driven in the installed Chrome.
//
//   npm --prefix tools/e2e-sim ci
//   node tools/e2e-sim/mimo_views.mjs [--shots <dir>] [--headed]
//
// It builds and starts `sds-control` (release, loopback, a free port) and the shell's dev server,
// sets up and starts the run on a connection of its own (the controller), and mounts the run view
// in Chrome until the MIMO screen hosts it:
//
// 1. One monitor: the four pages (error heatmap, matrix grid, drive and plant, geometry), each
//    shown with data from the run and asking the service for exactly what it draws; a page added
//    and removed during the run, and the subscription following it. A screenshot of each page.
// 2. Two monitors: two windows sharing one layout, the grid in the main window and the heatmap in
//    the second, both at 10 updates a second, and the frames per second each window keeps while
//    both draw (the bar is 30).

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
const SHOTS = path.resolve(flag('--shots') ?? path.join(os.tmpdir(), 'hertzforge-e2e-mimo'));
const HEADED = args.includes('--headed');
fs.mkdirSync(SHOTS, { recursive: true });

const CONTROLS = 72;
const DRIVES = 24;
const TITLE = 'Full-Scale MIMO on the Simulator';
// The upper half of the frame holds tighter tolerances than the lower, so every band of the scale shows.
const BANDS = Array.from({ length: CONTROLS }, (_, i) => (i < CONTROLS / 2 ? { alarm_db: [3, 3], abort_db: [6, 6] } : { alarm_db: [1, 1], abort_db: [1.5, 1.5] }));

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

// ---- the controller: a connection of its own that holds control for the whole check ----------
function controller(port) {
	const ws = new WebSocket(`ws://127.0.0.1:${port}/`);
	ws.binaryType = 'arraybuffer';
	let id = 0;
	const pending = new Map();
	const status = { last: null };
	const opened = new Promise((resolve, reject) => {
		ws.onerror = () => reject(new Error('cannot reach the service'));
		ws.onmessage = (m) => {
			if (typeof m.data !== 'string') return;
			const v = JSON.parse(m.data);
			if (v.type === 'hello') resolve();
			else if (v.type === 'response') {
				const p = pending.get(v.id);
				pending.delete(v.id);
				if (v.error) p?.reject(new Error(`${p.method}: ${v.error.message}`));
				else p?.resolve(v.result);
			} else if (v.type === 'event' && v.topic === 'mimo_random.status') status.last = v.data;
		};
	});
	const call = (method, params = {}) =>
		new Promise((resolve, reject) => {
			const n = ++id;
			pending.set(n, { resolve, reject, method });
			ws.send(JSON.stringify({ type: 'request', id: n, method, params }));
		});
	return { opened, call, status, close: () => ws.close() };
}

const AXES = ['X', 'Y', 'Z'];
const pad = (n) => String(n).padStart(2, '0');

function row(index, role, description) {
	const v = role === 'drive';
	return {
		index,
		enabled: true,
		description,
		role,
		physical: null,
		interface: 'single_ended',
		iepe_bias_v: null,
		units: v ? 'V' : 'g',
		sensitivity_mv_per_eu: v ? 1000 : 100,
		range_max: v ? 10 : 100,
		polarity: 'positive',
		control_group: null,
		abort: { accel_g: null, velocity_ips: null, displacement_mil: null }
	};
}

/** 24 triaxial points on a 1.2 m by 0.8 m by 0.5 m frame: 72 controls; 24 drives under and beside it. */
function layout() {
	const points = [];
	for (const z of [0, 0.5]) for (const y of [-0.4, 0, 0.4]) for (const x of [-0.6, -0.2, 0.2, 0.6]) points.push([x, y, z]);
	const sensors = [];
	points.forEach((p, k) => AXES.forEach((a, j) => sensors.push({ label: `P${pad(k + 1)} ${a}`, position: p, direction: [j === 0 ? 1 : 0, j === 1 ? 1 : 0, j === 2 ? 1 : 0] })));
	const drives = [];
	for (let k = 0; k < DRIVES; k++) {
		const axis = k < 12 ? 2 : k < 18 ? 0 : 1;
		const i = k < 12 ? k : k < 18 ? k - 12 : k - 18;
		const pos = axis === 2 ? [-0.6 + (i % 4) * 0.4, -0.4 + Math.floor(i / 4) * 0.4, -0.35] : axis === 0 ? [-0.95, -0.4 + (i % 3) * 0.4, i < 3 ? 0 : 0.5] : [-0.6 + (i % 3) * 0.6, -0.75, i < 3 ? 0 : 0.5];
		drives.push({ label: `Shaker ${pad(k + 1)} ${AXES[axis]}`, position: pos, direction: [axis === 0 ? 1 : 0, axis === 1 ? 1 : 0, axis === 2 ? 1 : 0] });
	}
	return { sensors, drives, reference_point: [0, 0, 0.25], length_unit: 'm' };
}

// ---- checks -----------------------------------------------------------------------------------
const results = [];
function check(name, ok, detail = '') {
	results.push({ name, ok, detail });
	console.log(`${ok ? 'pass' : 'FAIL'}  ${name}${detail ? `  (${detail})` : ''}`);
	if (!ok) throw new Error(`check failed: ${name} ${detail}`);
}

const tid = (page, id) => page.locator(`[data-testid="${id}"]`).first();

async function shot(page, name) {
	const file = path.join(SHOTS, `${name}.png`);
	await page.screenshot({ path: file });
	const kb = fs.statSync(file).size / 1024;
	console.log(`      screenshot ${file} (${kb.toFixed(0)} kB)`);
	check(`${name}.png is under 1 MB`, kb < 1024, `${kb.toFixed(0)} kB`);
}

/** The plot ids and quantities this window's service accepted. */
const accepted = (page) => page.evaluate(() => (globalThis.__mimoViews.feed.accepted?.plots ?? []).map((p) => `${p.id}=${p.quantity}`));
const hasData = (page, id) => page.evaluate((i) => !!globalThis.__mimoViews.plotData(i)?.views?.length, id);

/** Frames per second this page draws over `ms`, from its own animation frames. */
const fps = (page, ms) =>
	page.evaluate(
		(ms) =>
			new Promise((resolve) => {
				let n = 0;
				let worst = 0;
				let last = performance.now();
				const t0 = last;
				const tick = (t) => {
					n++;
					worst = Math.max(worst, t - last);
					last = t;
					if (t - t0 < ms) requestAnimationFrame(tick);
					else resolve({ fps: (n * 1000) / (t - t0), worstFrameMs: worst });
				};
				requestAnimationFrame(tick);
			}),
		ms
	);

async function mountView(page, url, port, o) {
	await page.goto(url);
	await page.waitForLoadState('networkidle');
	await page.evaluate(
		async ({ o, port, geometry }) => {
			const m = await import('/src/lib/acquire/mimo/views/harness.ts');
			m.mountRunView({ controlUrl: `ws://127.0.0.1:${port}/`, geometry, ...o });
		},
		{ o: { title: TITLE, bands: BANDS, ...o }, port, geometry: layout() }
	);
	await waitFor('the run view', () => page.evaluate(() => !!globalThis.__mimoViews?.status?.shape));
}

// ---- the run ----------------------------------------------------------------------------------
console.log('building the control service (release)');
execFileSync('cargo', ['build', '--release', '-q', '-p', 'sds-control', '--locked'], { cwd: ROOT, stdio: 'inherit' });
const servicePort = await freePort();
const shellPort = await freePort();
const TARGET = process.env.CARGO_TARGET_DIR ? path.resolve(process.env.CARGO_TARGET_DIR) : path.join(ROOT, 'target');
const exe = path.join(TARGET, 'release', process.platform === 'win32' ? 'sds-control.exe' : 'sds-control');
const service = start('sds-control', exe, ['--port', String(servicePort)]);
await waitFor('the control service', () => service.log.join('').includes('listening on ws://127.0.0.1:'), 30_000);
start('shell', process.platform === 'win32' ? 'npm.cmd' : 'npm', ['--prefix', 'ui', 'run', 'dev', '--', '--port', String(shellPort), '--strictPort'], { shell: process.platform === 'win32' });
const URL_ = `http://127.0.0.1:${shellPort}/?control=ws://127.0.0.1:${servicePort}/&theme=light`;
await waitFor('the shell dev server', async () => (await fetch(`http://127.0.0.1:${shellPort}/`)).ok, 60_000);
console.log(`service ws://127.0.0.1:${servicePort}/, shell ${URL_}`);

const ctl = controller(servicePort);
await ctl.opened;
await ctl.call('session.claim', { name: 'Run controller' });
await ctl.call('station.select', { kind: 'sim' });
const geo = layout();
const rows = [...geo.sensors.map((s, i) => row(i + 1, 'control', s.label)), ...geo.drives.map((d, k) => row(101 + k, 'drive', d.label))];
const rev = (await ctl.call('channels.get'))['revision'];
await ctl.call('channels.set', { revision: rev, channels: rows });
const set = await ctl.call('mimo_random.setup', {
	setup: {
		title: TITLE,
		channels: {
			controls: rows.filter((r) => r.role === 'control').map((r, i) => ({ channel: r.index, ...BANDS[i] })),
			drives: rows.filter((r) => r.role === 'drive').map((r) => ({ channel: r.index, ceiling_vrms: 5 }))
		},
		reference: { kind: 'file', path: '/data/references/full_scale.hforge', sha256: 'ab'.repeat(32) },
		estimation: { fs_hz: 8192, n_pts: 4096 },
		identification: { review: false },
		schedule: { mode: 'auto', levels: [{ db: 0 }], full_level_s: 3600 }
	}
});
check('the service took the 72 by 24 setup', set.state.shape.controls === CONTROLS && set.state.shape.drives === DRIVES, `${set.state.shape.controls} by ${set.state.shape.drives}, ${set.state.grid?.lines} lines`);
await ctl.call('mimo_random.start');
await waitFor('full level', () => ctl.status.last?.phase === 'full_level', 120_000);
console.log('the run is at full level');

const browser = await chromium.launch({ channel: 'chrome', headless: !HEADED });
const viewport = { width: 1440, height: 900 };
const report = {};
let failed = null;

try {
	// ============ 1. One monitor ================================================================
	const ctxA = await browser.newContext({ viewport });
	const a = await ctxA.newPage();
	await mountView(a, URL_, servicePort, { monitors: 1 });

	// The error heatmap first: every control, nothing else asked for.
	await waitFor('the heatmap', async () => (await tid(a, 'heatmap').getAttribute('data-rows')) === String(CONTROLS), 30_000);
	const heatSub = await accepted(a);
	check('the heatmap page asks for its rows only', heatSub.length === 1 && heatSub[0] === 'mp-1:heat=error_db', heatSub.join());
	await tid(a, 'heatmap-canvas').hover({ position: { x: 600, y: 200 } });
	const readout = (await tid(a, 'heatmap-readout').textContent()).replace(/\s+/g, ' ').trim();
	check('the heatmap cursor reads control, frequency and dB', /Hz/.test(readout) && /dB/.test(readout), readout);
	await shot(a, 'wp8_heatmap');

	// The matrix grid: its window block and its 8 spectra.
	await tid(a, 'mimo-page-select').selectOption('mp-2');
	await waitFor('the grid data', () => hasData(a, 'mp-2:sdm'), 30_000);
	await waitFor('the grid spectra', () => hasData(a, 'mp-2:psd:7'), 30_000);
	const gridSub = await accepted(a);
	check('the grid page asks for one 8 by 8 block and 8 spectra', gridSub.length === 9 && gridSub[0] === 'mp-2:sdm=sdm' && gridSub.slice(1).every((s) => s.endsWith('=psd')), gridSub.join());
	const box = await tid(a, 'grid-canvas').boundingBox();
	await a.mouse.move(box.x + 96 + 2.5 * ((box.width - 96) / 8), box.y + 26 + 0.5 * ((box.height - 56) / 8));
	await a.waitForTimeout(300);
	const gridRead = (await tid(a, 'grid-readout').textContent()).replace(/\s+/g, ' ').trim();
	check('the grid cursor names the cell and gives its numbers', /Hz/.test(gridRead) && /reference/.test(gridRead), gridRead);
	await shot(a, 'wp8_grid');

	// Scroll and pin: the subscription follows the window.
	await tid(a, 'grid-pin').selectOption('P20 Z');
	await a.locator('[aria-label="Next controls"]').click();
	await waitFor('the scrolled window', async () => { const s = await accepted(a); return s.includes('mp-2:psd:59=psd') && s.includes('mp-2:psd:13=psd'); }, 10_000);
	const scrolled = await accepted(a);
	check('pinned and scrolled: the block follows the window', scrolled.includes('mp-2:psd:59=psd') && scrolled.length === 9, scrolled.slice(1).join());
	// A cell opened full size adds exactly its own plot.
	await tid(a, 'grid-canvas').focus();
	await a.keyboard.press('ArrowRight');
	await a.keyboard.press('Enter');
	await waitFor('the open cell', async () => (await accepted(a)).some((s) => s.startsWith('mp-2:open=')), 10_000);
	await waitFor('the open cell data', () => hasData(a, 'mp-2:open'), 10_000);
	const opened = await accepted(a);
	check('an opened cell is subscribed as its own quantity', opened.includes('mp-2:open=coh'), opened.filter((s) => s.includes('open')).join());
	await shot(a, 'wp8_grid_open');
	await tid(a, 'grid-close').click();
	await waitFor('the open cell let go', async () => !(await accepted(a)).some((s) => s.startsWith('mp-2:open=')), 10_000);

	// Drive and plant.
	await tid(a, 'mimo-page-select').selectOption('mp-3');
	await waitFor('the plant data', () => hasData(a, 'mp-3:frf'), 30_000);
	await waitFor('the drive data', () => hasData(a, 'mp-3:drive'), 30_000);
	const plantSub = await accepted(a);
	check('the drive and plant page asks for its drive, pair, response, singular values, condition and caps', plantSub.map((s) => s.split('=')[1]).join() === 'drive_psd,drive_coh,frf,sv,cond,cap', plantSub.join());
	await a.waitForTimeout(500);
	await shot(a, 'wp8_plant');

	// Geometry: physical control, nothing subscribed; colors from the status.
	await tid(a, 'mimo-page-select').selectOption('mp-4');
	await waitFor('the 3-D view', () => a.locator('[data-testid="shape3d-canvas"] canvas').count().then((n) => n > 0), 30_000);
	await a.waitForTimeout(1200);
	const geoSub = await accepted(a);
	check('the geometry page under physical control asks for nothing', geoSub.length === 0, geoSub.join());
	await shot(a, 'wp8_geometry');

	// A page added and removed during the run.
	await tid(a, 'mimo-page-add').selectOption('heatmap');
	await waitFor('the added page subscribed', async () => (await accepted(a)).some((s) => /^mp-5:heat=/.test(s)), 10_000);
	check('a page added during the run is subscribed', true);
	await tid(a, 'mimo-page-remove').click();
	await tid(a, 'mimo-page-remove-yes').click();
	await waitFor('the removed page let go', async () => !(await accepted(a)).some((s) => s.startsWith('mp-5:')), 10_000);
	check('a page removed during the run is let go', true, (await accepted(a)).join() || 'nothing');
	check('the run went on untouched', ctl.status.last?.phase === 'full_level', ctl.status.last?.phase);
	await ctxA.close();

	// ============ 2. Two monitors ===============================================================
	const ctxB = await browser.newContext({ viewport });
	const main = await ctxB.newPage();
	const second = await ctxB.newPage();
	await mountView(main, URL_, servicePort, { monitors: 2, windowIndex: 0 });
	await mountView(second, URL_, servicePort, { monitors: 2, windowIndex: 1, compact: true });
	for (const p of [main, second]) await p.evaluate(() => globalThis.__mimoViews.setRate(10));
	await waitFor('the main window on the grid', () => hasData(main, 'mp-2:sdm'), 30_000);
	await waitFor('the second window on the heatmap', async () => (await tid(second, 'heatmap').getAttribute('data-rows')) === String(CONTROLS), 30_000);
	check('two monitors: the grid in the main window, the heatmap in the second', (await accepted(main))[0] === 'mp-2:sdm=sdm' && (await accepted(second))[0] === 'mp-1:heat=error_db');
	// A10: both windows draw at 10 updates a second while the frame rate is counted.
	const [fm, fs2] = await Promise.all([fps(main, 6000), fps(second, 6000)]);
	const drawGrid = Number(await tid(main, 'matrix-grid').getAttribute('data-draw-ms'));
	const drawHeat = Number(await tid(second, 'heatmap').getAttribute('data-draw-ms'));
	report.fps = { main_grid: fm, second_heatmap: fs2, draw_ms: { grid: drawGrid, heatmap: drawHeat } };
	console.log(`      frames per second: grid window ${fm.fps.toFixed(1)} (worst frame ${fm.worstFrameMs.toFixed(1)} ms), heatmap window ${fs2.fps.toFixed(1)} (worst frame ${fs2.worstFrameMs.toFixed(1)} ms); last draw: grid ${drawGrid} ms, heatmap ${drawHeat} ms`);
	check('A10: both windows keep 30 frames per second or more', fm.fps >= 30 && fs2.fps >= 30, `${fm.fps.toFixed(1)} and ${fs2.fps.toFixed(1)}`);
	await main.bringToFront();
	await shot(main, 'wp8_two_monitors_main');
	await second.bringToFront();
	await shot(second, 'wp8_two_monitors_second');
	await ctxB.close();
} catch (err) {
	failed = err;
} finally {
	await browser.close();
	try {
		await ctl.call('mimo_random.stop');
	} catch {
		/* the service may be gone */
	}
	ctl.close();
	stopAll();
}

fs.writeFileSync(path.join(SHOTS, 'mimo_views_results.json'), JSON.stringify({ results, report }, null, 2));
const passed = results.filter((r) => r.ok).length;
console.log(`\n${passed} of ${results.length} checks passed${failed ? `; stopped: ${failed.message}` : ''}`);
process.exit(failed ? 1 : 0);
