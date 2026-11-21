/**
 * The Measure screen's plots, as views the shared plot pane draws, from the control service's
 * `modal_test.live` frame, its state and the check it gave the setup. Nothing is computed here:
 * each curve is a series the service sent, against its own axis (a time trace's samples at the
 * service's rate; the FRF, phase and coherence at the service's lines). The force window is
 * marked by the service's frame counts as a span, the exponential window named with the time
 * constant the service reports, and a second impact marked at the frame the service found it.
 */

import type { ModalCheckImpact, ModalLatest, ModalRowDof, ModalTestLive, ModalTrace, ModalTraceRole } from '$lib/protocol/modal_test';
import type { NumericArray, Region, SpectrumView } from '$lib/protocol/types';
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
	/** The force window's span on the force trace, from the block's start, or null. */
	window: Region[];
	/** The traces are the latest hit's own block (true), or the most recent frames (false). */
	ownBlock: boolean;
}

/**
 * The latest hit's force and response traces. When the service sends the hit's own block, the
 * force window is drawn as the span the service gives in frames and the second impact at its
 * frame; otherwise the most recent frames are drawn and the windows are not placed on them.
 */
export function hitViews(live: ModalTestLive | null, latest: ModalLatest | null, impact: ModalCheckImpact | null | undefined, dofs: { references: ModalRowDof[]; responses: ModalRowDof[] }, response: number): HitViews {
	const none: HitViews = { force: null, response: null, second: null, window: [], ownBlock: false };
	if (!live) return none;
	const own = !!live.hit && (!latest || live.hit.seq === latest.seq);
	const block = own && live.hit ? live.hit : { rate_hz: live.time.rate_hz, channels: live.time.channels };
	const forces = tracesOf(block.channels, 'reference', dofs);
	const resps = tracesOf(block.channels, 'response', dofs);
	const rate = block.rate_hz;
	const make = (t: ModalTrace[], names: ModalRowDof[], title: string, label: string): SpectrumView | null => {
		if (!t.length || !t[0].values.length || !(rate > 0)) return null;
		const x = timeAxis(t[0].values.length, rate);
		const units = [...new Set(t.map((c, k) => c.unit ?? names[k]?.unit ?? ''))];
		return {
			view: 'spectrum',
			title,
			x: { label: own ? 'Time from the block start' : 'Time', unit: 's', scale: 'linear' },
			y: { label, unit: units.length === 1 ? units[0] : '', scale: 'linear' },
			series: t.map((c, k) => ({ name: dofName(names[k], `Channel ${k + 1}`), role: label === 'Force' ? 'drive' : 'measured', x, y: arr(c.values) }))
		};
	};
	const force = make(forces, dofs.references, 'Force', 'Force');
	const chosen = resps[response] ? [resps[response]] : resps.slice(0, 1);
	const resp = make(chosen, [dofs.responses[resps[response] ? response : 0]], 'Response', 'Response');
	const window: Region[] = [];
	let second: number | null = null;
	if (own && rate > 0) {
		const fw = impact?.force_window;
		if (fw) window.push({ x0: 0, x1: (fw.pre_frames + fw.width_frames + fw.fall_frames) / rate, kind: 'span', label: 'Force window' });
		const d = latest?.report?.double_hit;
		if (d) second = d.frame / rate;
	}
	if (force && impact?.exponential_window) {
		const e = impact.exponential_window;
		force.readouts = [{ label: 'Exponential window', text: `tau ${num(e.tau_s, 's')}, ${num(e.end * 100)}% at the block end`, status: 'info' }];
	}
	if (force && second !== null) force.readouts = [...(force.readouts ?? []), { label: 'Double hit', text: `second impact at ${num(second, 's')}`, status: 'fail' }];
	return { force, response: resp, second, window, ownBlock: own };
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

/** The latest hit's force spectrum, when the service sends it. */
export function forceSpectrumView(live: ModalTestLive | null): SpectrumView | null {
	const s = live?.force_spectrum;
	if (!s || !s.frequency_hz.length) return null;
	return { view: 'spectrum', title: 'Force Spectrum', x: { label: 'Frequency', unit: 'Hz', scale: 'linear' }, y: { label: 'Autospectrum', unit: s.unit, scale: 'log' }, series: [{ name: 'Force', role: 'drive', x: arr(s.frequency_hz), y: arr(s.values) }] };
}

/** The reference autospectra as the average builds (shaker mode), when the service sends them. */
export function referenceSpectraView(live: ModalTestLive | null, dofs: { references: ModalRowDof[] }): SpectrumView | null {
	const s = live?.reference_spectra;
	if (!s || !s.frequency_hz.length || !s.references.length) return null;
	const f = arr(s.frequency_hz);
	return {
		view: 'spectrum',
		title: 'Reference Autospectra',
		x: { label: 'Frequency', unit: 'Hz', scale: 'linear' },
		y: { label: 'Autospectrum', unit: s.unit, scale: 'log' },
		series: s.references.map((r) => ({ name: dofName(dofs.references[r.reference], `Reference ${r.reference + 1}`), role: 'reference', x: f, y: arr(r.values) }))
	};
}
