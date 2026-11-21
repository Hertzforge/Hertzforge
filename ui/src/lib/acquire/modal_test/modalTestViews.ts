/**
 * The Measure screen's plots, as views the shared plot pane draws, from the control service's
 * `modal_test.live` frame, its state and the check it gave the setup. Nothing is computed here:
 * each curve is a series the service sent, against its own axis (a time trace's samples at the
 * service's rate; the FRF, phase and coherence at the service's lines). The windows over the
 * latest hit are drawn from the counts and the time constant the hit was taken with, by the
 * formulas PROTOCOL.md gives, and a second impact is marked at the frame the service found it.
 */

import type { ModalLatest, ModalRowDof, ModalTestLive, ModalTrace, ModalTraceRole } from '$lib/protocol/modal_test';
import type { NumericArray, Region, SpectrumView } from '$lib/protocol/types';
import type { PaneNote, SeriesOverride } from '$lib/viz/paneOptions';
import { TRACE_COLORS } from '$lib/viz/roles';
import { num } from './modalTestWords';

const arr = (v: Float32Array | number[]): NumericArray => (v instanceof Float32Array ? v : Float64Array.from(v));

/** A row's name: its DOF as the analysis side writes it, `4Z+`, with its table row. */
export function dofName(r: ModalRowDof | undefined, fallback: string): string {
	if (!r) return fallback;
	const dof = r.node !== null && r.node !== undefined ? `${r.node}${r.direction ?? ''}` : 'no DOF';
	return `${dof} (row ${r.index})`;
}

/**
 * The traces of one role. With the roles the service sends, by role; without them, by the order
 * the service sends them in: every measured channel in table order (by row index), and anything
 * after them a drive.
 */
export function tracesOf(channels: ModalTrace[], role: ModalTraceRole, dofs: { references: ModalRowDof[]; responses: ModalRowDof[] }): ModalTrace[] {
	if (channels.some((c) => c.role)) return channels.filter((c) => c.role === role);
	const measured = [...dofs.references.map((r) => ({ r, role: 'reference' as const })), ...dofs.responses.map((r) => ({ r, role: 'response' as const }))].sort((a, b) => a.r.index - b.r.index);
	if (role === 'drive') return channels.slice(measured.length);
	const own = role === 'reference' ? dofs.references : dofs.responses;
	return own.map((r) => channels[measured.findIndex((m) => m.r === r)]).filter((c): c is ModalTrace => !!c);
}

function timeAxis(n: number, rate: number): Float64Array {
	const x = new Float64Array(n);
	for (let i = 0; i < n; i++) x[i] = i / rate;
	return x;
}

export interface HitViews {
	force: SpectrumView | null;
	response: SpectrumView | null;
	/** The second impact's time on the force trace, s, or null. */
	second: number | null;
	/** The force window and the exponential window over the force trace, as the service took them. */
	forceRegions: Region[];
	/** The exponential window over the response trace. */
	responseRegions: Region[];
	/** The hit these traces are: its number and point. */
	n: number | null;
}

/** At most this many weights on a window's rail: the window is drawn, not computed for analysis. */
const RAIL_POINTS = 256;

/** The sample numbers the rail shows, evenly spread over the block. */
function railSamples(samples: number): number[] {
	const n = Math.max(2, Math.min(samples, RAIL_POINTS));
	return Array.from({ length: n }, (_, k) => (k * (samples - 1)) / (n - 1));
}

/**
 * The force window as PROTOCOL.md defines it from the service's own counts: one up to
 * `pre + width`, a half-cosine fall over `fall` samples, then zero.
 */
export function forceWindowWeights(w: { pre_frames: number; width_frames: number; fall_frames: number }, samples: number): number[] {
	const open = w.pre_frames + w.width_frames;
	return railSamples(samples).map((n) => {
		if (n < open) return 1;
		const k = n - open;
		return k < w.fall_frames ? 0.5 * (1 + Math.cos((Math.PI * k) / w.fall_frames)) : 0;
	});
}

/** The exponential window, exp(-t / tau), from the service's tau and rate. */
export function exponentialWeights(tau_s: number, rate: number, samples: number): number[] {
	return railSamples(samples).map((n) => Math.exp(-n / rate / tau_s));
}

/**
 * The latest hit's own block, as the service sends it: the force and the chosen response from the
 * block's start, the force window and the exponential window it was taken with drawn over them on
 * the window rail, and the second impact marked at the frame the service found it. With no hit yet
 * there is nothing to draw.
 */
export function hitViews(live: ModalTestLive | null, latest: ModalLatest | null, dofs: { references: ModalRowDof[]; responses: ModalRowDof[] }, response: number): HitViews {
	const none: HitViews = { force: null, response: null, second: null, forceRegions: [], responseRegions: [], n: null };
	const h = live?.hit;
	const rate = live?.time.rate_hz ?? 0;
	if (!h || !(h.step_s > 0) || !(rate > 0)) return none;
	const forces = tracesOf(h.channels, 'reference', dofs);
	const resps = tracesOf(h.channels, 'response', dofs);
	const make = (t: ModalTrace[], names: ModalRowDof[], title: string, label: string): SpectrumView | null => {
		if (!t.length || !t[0].values.length) return null;
		const x = new Float64Array(t[0].values.length);
		for (let i = 0; i < x.length; i++) x[i] = i * h.step_s;
		const units = [...new Set(t.map((c, k) => c.unit ?? names[k]?.unit ?? ''))];
		return {
			view: 'spectrum',
			title,
			x: { label: 'Time from the block start', unit: 's', scale: 'linear' },
			y: { label, unit: units.length === 1 ? units[0] : '', scale: 'linear' },
			series: t.map((c, k) => ({ name: dofName(names[k], `Channel ${k + 1}`), role: label === 'Force' ? 'drive' : 'measured', x, y: arr(c.values) }))
		};
	};
	const force = make(forces, dofs.references, 'Force', 'Force');
	const pick = resps[response] ? response : 0;
	const resp = make(resps.slice(pick, pick + 1), [dofs.responses[pick]], 'Response', 'Response');
	const end = (h.samples - 1) / rate;
	const fw = h.windows.force;
	const ew = h.windows.exponential;
	const rows = (fw ? 1 : 0) + (ew ? 1 : 0);
	const forceRegions: Region[] = [];
	const responseRegions: Region[] = [];
	if (fw) forceRegions.push({ x0: 0, x1: end, kind: 'frame', label: `Force window: open ${fw.pre_frames + fw.width_frames} samples, falling over ${fw.fall_frames}`, window: forceWindowWeights(fw, h.samples), level: 0, rows });
	if (ew) {
		const r: Region = { x0: 0, x1: end, kind: 'frame', label: `Exponential window: tau ${num(ew.tau_s, 's')}, ${num(ew.end * 100)}% at the block end`, window: exponentialWeights(ew.tau_s, rate, h.samples) };
		forceRegions.push({ ...r, level: rows - 1, rows });
		responseRegions.push({ ...r, level: 0, rows: 1 });
	}
	const d = latest && latest.seq === h.seq ? latest.report?.double_hit : null;
	const second = d ? d.frame / rate : null;
	const readouts = (v: SpectrumView | null, withForce: boolean) => {
		if (!v) return;
		v.readouts = [
			...(withForce && fw ? [{ label: 'Force window', text: `open ${fw.pre_frames + fw.width_frames} samples, falling over ${fw.fall_frames}`, status: 'info' as const }] : []),
			...(ew ? [{ label: 'Exponential window', text: `tau ${num(ew.tau_s, 's')}, ${num(ew.end * 100)}% at the block end`, status: 'info' as const }] : [])
		];
	};
	readouts(force, true);
	readouts(resp, false);
	return { force, response: resp, second, forceRegions, responseRegions, n: h.n };
}

/**
 * The labels over the force trace, inside the plot's drawing area: Double Hit near the top beside
 * the second impact's marker (on its left when the marker is in the right part of the block), and
 * Overload at the top left. Positions are fractions of the drawing area, the x axis running over
 * the block.
 */
export function forceNotes(second: number | null, blockEnd: number, overload: boolean): PaneNote[] {
	const out: PaneNote[] = [];
	if (second !== null && blockEnd > 0) {
		const at = Math.min(1, Math.max(0, second / blockEnd));
		out.push({ id: 'double-hit', text: 'Double\u00a0Hit', fx: at > 0.6 ? Math.max(0, at - 0.3) : at + 0.02, fy: 0.03 });
	}
	if (overload) out.push({ id: 'overload', text: 'Overload', fx: 0.02, fy: second !== null ? 0.2 : 0.03 });
	return out;
}

/** The colors several traces of one role take, in order: apart under the common color-vision deficiencies. */
const OVERLAY = ['blue', 'orange', 'green', 'vermillion', 'purple', 'sky'].map((id) => TRACE_COLORS.find((c) => c.id === id)!.hex);

/**
 * Several forces, drives or reference spectra in one plot, overlaid: each its own color, the
 * role's line kept, so each has its own legend entry to tell it by. One trace keeps its role's look.
 */
export function overlayStyles(view: SpectrumView | null): SeriesOverride[] {
	const n = view?.series.length ?? 0;
	if (n < 2) return [];
	return view!.series.map((_, k) => ({ color: OVERLAY[k % OVERLAY.length], width: 1.3 }));
}

/** A second impact in words, from the service's report: where, and how large against the peak. */
export function secondImpactWords(d: { frame: number; peak: number; ratio: number } | null | undefined, rate: number, unit: string): string | null {
	if (!d) return null;
	const at = rate > 0 ? `${num(d.frame / rate, 's')} into the block` : `sample ${d.frame}`;
	return `Second impact at ${at}, ${num(d.peak, unit)}: ${num(d.ratio * 100)}% of the peak.`;
}

/** The live traces of one role (shaker mode: the references, the responses, the drives). */
export function roleView(live: ModalTestLive | null, role: ModalTraceRole, dofs: { references: ModalRowDof[]; responses: ModalRowDof[] }, title: string): SpectrumView | null {
	if (!live) return null;
	const t = tracesOf(live.time.channels, role, dofs);
	if (!t.length || !t[0].values.length || !(live.time.rate_hz > 0)) return null;
	const x = timeAxis(t[0].values.length, live.time.rate_hz);
	const names = role === 'reference' ? dofs.references : role === 'response' ? dofs.responses : [];
	const units = [...new Set(t.map((c, k) => c.unit ?? names[k]?.unit ?? (role === 'drive' ? 'V' : '')))];
	return {
		view: 'spectrum',
		title,
		x: { label: 'Time', unit: 's', scale: 'linear' },
		y: { label: title, unit: units.length === 1 ? units[0] : '', scale: 'linear' },
		series: t.map((c, k) => ({ name: role === 'drive' ? (c.index !== undefined ? `Drive row ${c.index}` : `Drive ${k + 1}`) : dofName(names[k], `Channel ${k + 1}`), role: role === 'drive' ? 'drive' : 'measured', x, y: arr(c.values) }))
	};
}

export interface FrfViews {
	magnitude: SpectrumView | null;
	phase: SpectrumView | null;
	coherence: SpectrumView | null;
	averages: number;
}

/** The FRF of the chosen response and reference for the average so far, its phase and coherence. */
export function frfViews(live: ModalTestLive | null, dofs: { references: ModalRowDof[]; responses: ModalRowDof[] }, response: number, reference: number): FrfViews {
	const out: FrfViews = { magnitude: null, phase: null, coherence: null, averages: live?.frf.averages ?? 0 };
	if (!live || !live.frf.frequency_hz.length) return out;
	const f = arr(live.frf.frequency_hz);
	const pair = live.frf.pairs.find((p) => p.response === response && p.reference === reference) ?? live.frf.pairs[0];
	const r = dofs.responses[pair?.response ?? 0];
	const q = dofs.references[pair?.reference ?? 0];
	const name = `${dofName(r, 'Response')} / ${dofName(q, 'Reference')}`;
	const unit = r && q ? `${r.unit}/${q.unit}` : '';
	if (pair) {
		out.magnitude = { view: 'spectrum', title: 'FRF Magnitude', x: { label: 'Frequency', unit: 'Hz', scale: 'linear' }, y: { label: 'Magnitude', unit, scale: 'log' }, series: [{ name, role: 'measured', x: f, y: arr(pair.magnitude) }] };
		out.phase = { view: 'spectrum', title: 'FRF Phase', x: { label: 'Frequency', unit: 'Hz', scale: 'linear' }, y: { label: 'Phase', unit: 'deg', scale: 'linear' }, series: [{ name, role: 'measured', x: f, y: arr(pair.phase_deg) }] };
	}
	const coh = live.frf.coherence.find((c) => c.response === (pair?.response ?? response)) ?? live.frf.coherence[0];
	if (coh) {
		const several = dofs.references.length > 1;
		out.coherence = { view: 'spectrum', title: several ? 'Multiple Coherence' : 'Coherence', x: { label: 'Frequency', unit: 'Hz', scale: 'linear' }, y: { label: 'Coherence', unit: '', scale: 'linear' }, series: [{ name: dofName(r, 'Response'), role: 'measured', x: f, y: arr(coh.values) }] };
	}
	return out;
}

/**
 * The latest hit's force autospectrum on the analysis lines, with the tip's roll-off the service
 * measured in the plot's header line, and Soft Tip beside it when the service flagged it.
 */
export function forceSpectrumView(live: ModalTestLive | null, softTip = false): SpectrumView | null {
	const s = live?.hit?.force_spectrum;
	if (!s || !s.frequency_hz.length) return null;
	return {
		view: 'spectrum',
		title: 'Force Spectrum',
		x: { label: 'Frequency', unit: 'Hz', scale: 'linear' },
		y: { label: 'Autospectrum', unit: s.unit, scale: 'log' },
		series: [{ name: 'Force', role: 'drive', x: arr(s.frequency_hz), y: arr(s.values) }],
		readouts: [{ label: 'Tip roll-off', text: `${s.tip_drop_db === null ? 'no force at the band top' : num(s.tip_drop_db, 'dB')}${softTip ? ', Soft Tip' : ''}`, status: softTip ? 'warn' : 'info' }]
	};
}

/** The reference autospectra in the present average (shaker mode). */
export function referenceSpectraView(live: ModalTestLive | null, dofs: { references: ModalRowDof[] }): SpectrumView | null {
	const s = live?.references;
	if (!s || !s.frequency_hz.length || !s.autospectra.length) return null;
	const f = arr(s.frequency_hz);
	const units = [...new Set(s.autospectra.map((a) => a.unit ?? ''))];
	return {
		view: 'spectrum',
		title: 'Reference Autospectra',
		x: { label: 'Frequency', unit: 'Hz', scale: 'linear' },
		y: { label: 'Autospectrum', unit: units.length === 1 ? units[0] : '', scale: 'log' },
		series: s.autospectra.map((a, k) => {
			const row = a.index !== undefined ? dofs.references.find((r) => r.index === a.index) : dofs.references[k];
			return { name: dofName(row, a.index !== undefined ? `Row ${a.index}` : `Reference ${k + 1}`), role: 'reference', x: f, y: arr(a.values) };
		})
	};
}
