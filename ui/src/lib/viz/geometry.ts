/**
 * The geometry view's model: sensors and drives placed from the channel table's positions and
 * directions, built as a mesh the shape pane draws. Each sensor is a small octahedron at its
 * position, stretched along its sensing direction; each drive is an arrow at its position along
 * its direction, its length scaled by the drive's rms; thin lines tie each sensor and drive to
 * the reference point. The rigid-body motion is one real shape: every node moves by
 * `t + w x (p - o)`, with `t` and `w` the six degrees of freedom's rms (translations X, Y, Z and
 * rotations RX, RY, RZ). It is a picture of the motion's size and direction, not a time history.
 */

import type { Shape3DView } from '$lib/protocol/types';
import { errorRgb, type RowBand, type ThemeKind, DEFAULT_BAND } from './scales';

export type Vec3 = [number, number, number];

export interface PlacedChannel {
	label: string;
	position: Vec3;
	direction: Vec3;
}

/** Where the sensors and drives are: the channel table's positions and directions. */
export interface MimoGeometry {
	sensors: PlacedChannel[];
	drives: PlacedChannel[];
	/** The point the six degrees of freedom are written at. */
	reference_point: Vec3;
	/** The positions' unit, for example `m`. */
	length_unit: string;
}

export const DOF_ORDER = ['X', 'Y', 'Z', 'RX', 'RY', 'RZ'] as const;

const sub = (a: Vec3, b: Vec3): Vec3 => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
const add = (a: Vec3, b: Vec3): Vec3 => [a[0] + b[0], a[1] + b[1], a[2] + b[2]];
const scale = (a: Vec3, k: number): Vec3 => [a[0] * k, a[1] * k, a[2] * k];
const cross = (a: Vec3, b: Vec3): Vec3 => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
const norm = (a: Vec3) => Math.hypot(a[0], a[1], a[2]);
const unit = (a: Vec3): Vec3 => {
	const n = norm(a);
	return n > 0 ? scale(a, 1 / n) : [0, 0, 1];
};

/** Two unit vectors at right angles to `d` and to each other. */
function basis(d: Vec3): [Vec3, Vec3] {
	const a: Vec3 = Math.abs(d[2]) < 0.9 ? [0, 0, 1] : [1, 0, 0];
	const u = unit(cross(d, a));
	return [u, cross(d, u)];
}

/** The size of the layout: the largest distance between any two placed points, or 1. */
export function extent(g: MimoGeometry): number {
	const pts = [...g.sensors.map((s) => s.position), ...g.drives.map((d) => d.position), g.reference_point];
	let best = 0;
	for (let i = 0; i < pts.length; i++) for (let j = i + 1; j < pts.length; j++) best = Math.max(best, norm(sub(pts[i], pts[j])));
	return best || 1;
}

export interface GeometryMesh {
	view: Shape3DView;
	/** The first node of each sensor's marker and how many nodes it has. */
	sensorNodes: [number, number][];
	/** The first node of each drive's arrow and how many nodes it has. */
	driveNodes: [number, number][];
}

/**
 * The mesh for a geometry: sensor markers, drive arrows scaled by `driveRms` (relative to the
 * largest), and the tie lines. `dofRms` (X, Y, Z, RX, RY, RZ), when given, becomes the one mode
 * the pane animates.
 */
export function buildGeometry(g: MimoGeometry, driveRms: (number | null)[] = [], dofRms: number[] | null = null, title = 'Geometry'): GeometryMesh {
	const size = extent(g);
	const marker = size * 0.035;
	const nodes: number[] = [];
	const tris: number[] = [];
	const lines: number[] = [];
	const sensorNodes: [number, number][] = [];
	const driveNodes: [number, number][] = [];
	const push = (p: Vec3) => {
		nodes.push(p[0], p[1], p[2]);
		return nodes.length / 3 - 1;
	};

	// the reference point
	const o = push(g.reference_point);

	for (const s of g.sensors) {
		const d = unit(s.direction);
		const [u, v] = basis(d);
		const first = nodes.length / 3;
		const c = s.position;
		const tip = push(add(c, scale(d, marker * 1.6)));
		const tail = push(add(c, scale(d, -marker * 0.8)));
		const ring = [u, v, scale(u, -1), scale(v, -1)].map((w) => push(add(c, scale(w, marker * 0.75))));
		for (let k = 0; k < 4; k++) {
			const a = ring[k];
			const b = ring[(k + 1) % 4];
			tris.push(tip, a, b, tail, b, a);
		}
		sensorNodes.push([first, 6]);
		lines.push(o, tail);
	}

	const biggest = Math.max(1e-12, ...driveRms.map((v) => (v !== null && Number.isFinite(v) ? v : 0)));
	for (let k = 0; k < g.drives.length; k++) {
		const dr = g.drives[k];
		const d = unit(dr.direction);
		const [u, v] = basis(d);
		const rel = driveRms[k] !== null && driveRms[k] !== undefined && Number.isFinite(driveRms[k]) ? (driveRms[k] as number) / biggest : 0;
		const length = size * (0.08 + 0.17 * Math.max(0, Math.min(1, rel)));
		const r = marker * 0.35;
		const head = marker * 0.9;
		const first = nodes.length / 3;
		const base = dr.position;
		const neck = add(base, scale(d, Math.max(length - head * 1.6, length * 0.4)));
		const tip = push(add(base, scale(d, length)));
		const shaft0 = [u, v, scale(u, -1), scale(v, -1)].map((w) => push(add(base, scale(w, r))));
		const shaft1 = [u, v, scale(u, -1), scale(v, -1)].map((w) => push(add(neck, scale(w, r))));
		const cone = [u, v, scale(u, -1), scale(v, -1)].map((w) => push(add(neck, scale(w, head))));
		for (let i = 0; i < 4; i++) {
			const j = (i + 1) % 4;
			tris.push(shaft0[i], shaft0[j], shaft1[j], shaft0[i], shaft1[j], shaft1[i]);
			tris.push(cone[i], cone[j], tip);
			tris.push(cone[j], cone[i], shaft1[i]);
		}
		driveNodes.push([first, 13]);
		lines.push(o, shaft0[0]);
	}

	const modes: Shape3DView['modes'] = [];
	if (dofRms && dofRms.length >= 6 && dofRms.some((v) => Number.isFinite(v) && v !== 0)) {
		modes.push({ index: 1, frequency_hz: 0, label: 'Rigid-body motion, rms', shape: rigidShape(Float32Array.from(nodes), g.reference_point, dofRms) });
	}
	return {
		view: {
			view: 'shape3d',
			title,
			unit: g.length_unit,
			nodes: Float32Array.from(nodes),
			triangles: Uint32Array.from(tris),
			lines: Uint32Array.from(lines),
			modes
		},
		sensorNodes,
		driveNodes
	};
}

/** Each node's displacement for a rigid-body motion `t + w x (p - o)`, f32 [N, 3]. */
export function rigidShape(nodes: Float32Array, o: Vec3, dof: number[]): Float32Array {
	const t: Vec3 = [dof[0] || 0, dof[1] || 0, dof[2] || 0];
	const w: Vec3 = [dof[3] || 0, dof[4] || 0, dof[5] || 0];
	const out = new Float32Array(nodes.length);
	for (let i = 0; i < nodes.length; i += 3) {
		const r: Vec3 = [nodes[i] - o[0], nodes[i + 1] - o[1], nodes[i + 2] - o[2]];
		const m = add(t, cross(w, r));
		out[i] = m[0];
		out[i + 1] = m[1];
		out[i + 2] = m[2];
	}
	return out;
}

/** The six degrees of freedom's rms in X, Y, Z, RX, RY, RZ order from named readings; null without all six. */
export function dofVector(rows: { label: string; value: number | string }[]): number[] | null {
	const out: number[] = [];
	for (const name of DOF_ORDER) {
		const row = rows.find((r) => r.label.trim().toUpperCase() === name);
		if (!row || typeof row.value !== 'number' || !Number.isFinite(row.value)) return null;
		out.push(row.value);
	}
	return out;
}

/**
 * Node colors, sRGB 0 to 1: each sensor's marker by its control's error on the error scale (with
 * its band), drives and everything else in a neutral gray. `errors[i]` is sensor i's error in
 * dB, or null for none.
 */
export function nodeColors(mesh: GeometryMesh, errors: (number | null)[], bands: RowBand[], theme: ThemeKind, neutral: [number, number, number]): Float32Array {
	const out = new Float32Array(mesh.view.nodes.length);
	for (let i = 0; i < out.length; i += 3) out.set(neutral, i);
	const rgb = [0, 0, 0];
	mesh.sensorNodes.forEach(([first, count], i) => {
		const e = errors[i];
		if (e === null || e === undefined || !Number.isFinite(e)) return;
		errorRgb(e, bands[i] ?? DEFAULT_BAND, theme, rgb);
		for (let k = 0; k < count; k++) out.set([rgb[0] / 255, rgb[1] / 255, rgb[2] / 255], 3 * (first + k));
	});
	return out;
}

/** Drive rms rounded to tenths of the largest, so the arrows are rebuilt only when they visibly change. */
export function arrowKey(driveRms: (number | null)[]): string {
	const biggest = Math.max(1e-12, ...driveRms.map((v) => (v !== null && Number.isFinite(v) ? v : 0)));
	return driveRms.map((v) => (v === null || !Number.isFinite(v) ? '-' : Math.round((v / biggest) * 10))).join(',');
}
