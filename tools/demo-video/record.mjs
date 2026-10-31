// Scripted walkthrough of the shell, recorded to an H.264 MP4.
//
// Drives the running stack (scripts/dev.sh) through the installed Chrome, captures every
// painted frame over the DevTools screencast, and lets ffmpeg lay the frames out on their
// real timestamps. Captions and a pointer are drawn into the page so they land in the frames.
//
//   node record.mjs [output.mp4] [--url http://127.0.0.1:5191/]
//
// The test ids it drives are the shell's own (`data-testid`), so layout changes do not break it.

import { chromium } from 'playwright-core';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

const args = process.argv.slice(2);
const urlFlag = args.indexOf('--url');
const BASE = urlFlag >= 0 ? args[urlFlag + 1] : 'http://127.0.0.1:5191/';
const OUT = path.resolve(args.find((a, i) => !a.startsWith('--') && args[i - 1] !== '--url') ?? 'out/demo.mp4');
const FRAMES = path.resolve('out/frames');
const W = 1280, H = 720, SCALE = 1.5; // 1920 x 1080 frames

fs.rmSync(FRAMES, { recursive: true, force: true });
fs.mkdirSync(FRAMES, { recursive: true });
fs.mkdirSync(path.dirname(OUT), { recursive: true });

const browser = await chromium.launch({ channel: 'chrome', headless: true, args: ['--enable-gpu', '--ignore-gpu-blocklist'] });
const context = await browser.newContext({ viewport: { width: W, height: H }, deviceScaleFactor: SCALE });
const page = await context.newPage();
const tid = (id) => page.locator(`[data-testid="${id}"]`).first();
const pause = (ms) => page.waitForTimeout(ms);

// ---- frame capture -------------------------------------------------------------------
const frames = [];
const cdp = await context.newCDPSession(page);
cdp.on('Page.screencastFrame', async ({ data, metadata, sessionId }) => {
	const file = path.join(FRAMES, `${String(frames.length).padStart(6, '0')}.jpg`);
	fs.writeFileSync(file, Buffer.from(data, 'base64'));
	frames.push({ file, t: metadata.timestamp });
	await cdp.send('Page.screencastFrameAck', { sessionId }).catch(() => {});
});

// ---- overlay: captions, title cards and a visible pointer ----------------------------
const OVERLAY = `
(() => {
  if (window.__demo) return;
  const css = document.createElement('style');
  css.textContent = \`
    #demo-cap { position: fixed; left: 50%; bottom: 28px; transform: translateX(-50%);
      max-width: 860px; padding: 12px 20px; border-radius: 10px; z-index: 99998;
      background: rgba(12, 20, 36, 0.86); color: #F2F5FA; pointer-events: none;
      font: 500 17px/1.4 'Atkinson Hyperlegible Next', system-ui, sans-serif; text-align: center;
      box-shadow: 0 8px 28px rgba(0,0,0,.35); transition: opacity .35s ease; opacity: 0; }
    #demo-cap b { color: #F2B25C; font-weight: 700; }
    #demo-card { position: fixed; inset: 0; z-index: 99999; display: grid; place-items: center;
      background: #0E1626; color: #E8EDF5; transition: opacity .6s ease; opacity: 0; pointer-events: none;
      font-family: 'Atkinson Hyperlegible Next', system-ui, sans-serif; }
    #demo-card .in { text-align: center; display: grid; gap: 14px; }
    #demo-card h1 { margin: 0; font-size: 46px; font-weight: 700; letter-spacing: -0.01em; }
    #demo-card p { margin: 0; font-size: 20px; color: #AFC0D6; }
    #demo-card .rule { width: 120px; height: 3px; background: #E8A040; margin: 6px auto; }
    #demo-card small { font-size: 15px; color: #8496AE; }
    #demo-ptr { position: fixed; left: 0; top: 0; width: 22px; height: 22px; z-index: 100000;
      pointer-events: none; transform: translate(-100px, -100px); transition: transform .05s linear; }
  \`;
  document.head.appendChild(css);
  const cap = document.createElement('div'); cap.id = 'demo-cap'; document.body.appendChild(cap);
  const card = document.createElement('div'); card.id = 'demo-card'; document.body.appendChild(card);
  const ptr = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  ptr.id = 'demo-ptr'; ptr.setAttribute('viewBox', '0 0 22 22');
  ptr.innerHTML = '<path d="M3 2 L3 18 L7.5 13.8 L10.6 20.4 L13.4 19.1 L10.4 12.6 L16.5 12.6 Z" fill="#fff" stroke="#111" stroke-width="1.3" stroke-linejoin="round"/>';
  document.body.appendChild(ptr);
  addEventListener('mousemove', (e) => { ptr.style.transform = \`translate(\${e.clientX - 3}px, \${e.clientY - 2}px)\`; }, true);
  window.__demo = {
    caption(html) { cap.innerHTML = html || ''; cap.style.opacity = html ? '1' : '0'; },
    card(html) { if (html) card.innerHTML = '<div class="in">' + html + '</div>'; card.style.opacity = html ? '1' : '0'; },
  };
})();`;
const caption = (html) => page.evaluate((h) => window.__demo.caption(h), html);
const card = (html) => page.evaluate((h) => window.__demo.card(h), html);

// Glide the mouse to an element's centre (or an offset in it) so the pointer is seen moving.
async function glide(locator, fx = 0.5, fy = 0.5, steps = 18) {
	const box = await locator.boundingBox();
	if (!box) throw new Error('No box for target');
	await page.mouse.move(box.x + box.width * fx, box.y + box.height * fy, { steps });
}
async function click(id, settle = 400) {
	const el = tid(id);
	await el.scrollIntoViewIfNeeded();
	await glide(el);
	await el.click();
	await pause(settle);
}

// ---- the walkthrough -----------------------------------------------------------------
await page.goto(`${BASE}?theme=dark#analyze`, { waitUntil: 'networkidle' });
await page.waitForFunction(() => /Connected/.test(document.querySelector('[data-testid="conn-analysis"]')?.textContent ?? '')
	&& /Connected/.test(document.querySelector('[data-testid="conn-control"]')?.textContent ?? ''), null, { timeout: 20000 });
await page.evaluate(OVERLAY);
await card(`<h1>Hertzforge</h1><div class="rule"></div>
  <p>One structural dynamics suite: test control, analysis, modal and simulation</p>
  <small>First working prototype &middot; October 2026</small>`);
await cdp.send('Page.startScreencast', { format: 'jpeg', quality: 90, maxWidth: W * SCALE, maxHeight: H * SCALE, everyNthFrame: 1 });
await pause(3800);
await card('');
await pause(700);

// Shell overview
await caption('<b>Seven workspaces, one shell.</b> Each workflow shows only its own tools.');
for (const ws of ['acquire', 'analyze', 'modal', 'model', 'specify', 'simulate', 'report']) {
	await glide(tid(`rail-${ws}`), 0.5, 0.5, 8);
	await pause(260);
}
await pause(400);

// Command palette
await caption('<b>Command palette.</b> Every tool, dataset and action, found by name.');
await click('palette-trigger', 500);
await page.keyboard.type('psd', { delay: 140 });
await pause(1600);
await page.keyboard.press('Escape');
await pause(400);

// Analyze
await click('rail-analyze', 500);
await caption('<b>Analyze.</b> Visual Dynamics computes every number from a controller run file. The shell only draws.');
await click('dataset-plate-random', 900);
await page.waitForSelector('[data-testid="tool-random.psd_vs_spec"]', { timeout: 30000 });
await click('tool-random.psd_vs_spec', 500);
await click('run-op', 300);
await page.waitForSelector('[data-testid="view-spectrum"]', { timeout: 30000 });
await pause(1400);
await click('tool-random.error_db', 400);
await click('run-op', 300);
await page.waitForFunction(() => document.querySelectorAll('[data-testid="view-spectrum"]').length >= 2, null, { timeout: 30000 });
await pause(900);
await caption('<b>Linked cursors.</b> Panes on the same frequency axis follow each other.');
const plot = page.locator('[data-testid="view-spectrum"] .u-over').first();
const pb = await plot.boundingBox();
if (pb) {
	await page.mouse.move(pb.x + pb.width * 0.15, pb.y + pb.height * 0.5, { steps: 10 });
	await page.mouse.move(pb.x + pb.width * 0.85, pb.y + pb.height * 0.45, { steps: 90 });
	await page.mouse.move(pb.x + pb.width * 0.45, pb.y + pb.height * 0.55, { steps: 45 });
}
await pause(600);
// Zoom into a narrow band so the bins are visible, then reset the zoom.
await caption('<b>Drawn the way it is computed.</b> Each spectral line is flat across its own bin, one df wide.');
const psdPlot = page.locator('[data-testid="view-spectrum"] .u-over').nth(1);
await psdPlot.evaluate((el) => el.scrollIntoView({ block: 'center' }));
await pause(500);
const zb = await psdPlot.boundingBox();
if (zb) {
	await page.mouse.move(zb.x + zb.width * 0.60, zb.y + zb.height * 0.5, { steps: 14 });
	await page.mouse.down();
	await page.mouse.move(zb.x + zb.width * 0.665, zb.y + zb.height * 0.5, { steps: 24 });
	await page.mouse.up();
	await pause(900);
	await page.mouse.move(zb.x + zb.width * 0.25, zb.y + zb.height * 0.45, { steps: 20 });
	await page.mouse.move(zb.x + zb.width * 0.75, zb.y + zb.height * 0.45, { steps: 60 });
	await pause(1200);
	await page.mouse.dblclick(zb.x + zb.width * 0.5, zb.y + zb.height * 0.5);
	await pause(700);
}
await caption('<b>Readings judge the run</b> against the specification\'s own warning and abort limits.');
await tid('view-reading').scrollIntoViewIfNeeded();
await glide(tid('view-reading'), 0.3, 0.3, 14);
await pause(2600);

// Modal
await click('rail-modal', 600);
await caption('<b>Modal.</b> A 74,000-triangle FE model solved by Visual Dynamics, animated live in the shell.');
await click('dataset-frame-wing-fem', 300);
await page.waitForSelector('[data-testid="mode-row-7"]', { timeout: 60000 });
await pause(2600);
const canvas = tid('shape3d-canvas');
const cb = await canvas.boundingBox();
if (cb) {
	await page.mouse.move(cb.x + cb.width * 0.55, cb.y + cb.height * 0.5, { steps: 12 });
	await page.mouse.down();
	await page.mouse.move(cb.x + cb.width * 0.30, cb.y + cb.height * 0.42, { steps: 60 });
	await page.mouse.up();
}
await pause(1200);
await caption('<b>Step through modes,</b> switch to wireframe, show the undeformed shape.');
await click('mode-row-8', 1700);
await click('mode-row-10', 1700);
await click('shape3d-wireframe', 1500);
await click('shape3d-ghost', 1500);
await click('shape3d-surface', 1200);

// Simulate
await click('rail-simulate', 600);
await caption('<b>Simulate.</b> The controller\'s own Random control law against a simulated test setup.');
await click('sim-start', 300);
await page.waitForFunction(() => document.querySelector('[data-testid="sim-state"]')?.dataset.state === 'running', null, { timeout: 15000 });
await pause(5000);
await caption('Identification, then the level ladder, then full test level. <b>Labeled SIM throughout.</b>');
await glide(tid('sim-badge'), 0.5, 0.5, 16);
await pause(7000);

// Theme
await caption('<b>Light and dark</b> from one set of design tokens.');
await click('theme-toggle', 2600);
await click('theme-toggle', 1200);
await click('sim-stop', 1200);
await caption('');

// Closing card
await card(`<h1>Built on Two Proven Cores</h1><div class="rule"></div>
  <p>Analysis: the Visual Dynamics core, unchanged &middot; Control: the controller's Rust engine</p>
  <p>Next: Acquire &amp; Control, Model (FEM), Specify, Report</p>`);
await pause(4200);
await cdp.send('Page.stopScreencast');
await browser.close();

// ---- assemble on real timestamps -----------------------------------------------------
if (frames.length < 2) throw new Error('No frames captured');
const list = [];
for (let i = 0; i < frames.length; i++) {
	const dur = i + 1 < frames.length ? Math.max(frames[i + 1].t - frames[i].t, 0.001) : 0.5;
	list.push(`file '${frames[i].file}'`, `duration ${dur.toFixed(4)}`);
}
list.push(`file '${frames[frames.length - 1].file}'`);
const listFile = path.join(FRAMES, 'list.txt');
fs.writeFileSync(listFile, list.join('\n') + '\n');
execFileSync('ffmpeg', ['-y', '-loglevel', 'error', '-f', 'concat', '-safe', '0', '-i', listFile,
	'-vf', 'fps=30,scale=1920:1080:flags=lanczos,format=yuv420p', '-c:v', 'libx264', '-preset', 'slow',
	'-crf', '18', '-movflags', '+faststart', OUT], { stdio: 'inherit' });
const seconds = frames[frames.length - 1].t - frames[0].t;
console.log(`wrote ${OUT}: ${frames.length} frames over ${seconds.toFixed(1)} s`);
