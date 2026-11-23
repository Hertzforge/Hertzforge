/**
 * Elements colored by strain or stress as a model moves (schema/PROTOCOL.md, `shape3d` `element_values`).
 *
 * The service sends each mode's strain or stress shape once: six components per colored element
 * per mode. At each frame the element's tensor is the modes' shapes scaled by the modal coordinates at that
 * sample and summed, as `motion.ts` sums the modes' offsets, and the color is one scalar of that
 * summed tensor. The sum and the scalar are formed here, per frame, because sending the scalar of
 * every element at every sample would make a clip as large as its length times the model; the
 * shapes are as large as the modes. Von Mises and the principal stresses are not linear in the
 * modal coordinates, so they are formed from the summed tensor, never summed mode by mode.
 *
 * These formulas are the shell's copy of `hertzforge.core.stress_scalars`, which is the reference.
 * Both are held to the same numbers by `schema/golden/stress_scalars.json`, read by this module's
 * test and by the Python test of the reference, so the two cannot drift apart.
 */
import type { ElementValues, StressScalar } from '$lib/protocol/types';

/** Components per element, in this order: xx, yy, zz, xy, yz, xz (tensor shears for stress, engineering shears for strain). */
export const COMPONENTS = 6;

const COMPONENT_INDEX: Record<string, number> = { xx: 0, yy: 1, zz: 2, xy: 3, yz: 4, xz: 5 };

/** The number of colored elements. */
export function elementCount(stress: ElementValues): number {
	return stress.elements;
}

/** The number of shapes `stress.shapes` holds: one per mode. */
export function shapeCount(stress: ElementValues): number {
	return stress.elements ? stress.shapes.length / (COMPONENTS * stress.elements) : 0;
}

/** Whether the scalar can be below zero, so its scale runs from -max to max rather than from 0. */
export function signed(scalar: StressScalar): boolean {
	return scalar !== 'von_mises' && scalar !== 'tresca';
}

/**
 * Add the shapes from `first` on, scaled by `weights` (one per shape), into `out` (f32 [E, 6]).
 * `shapes` is f32 [B, E, 6]; `out` is not cleared.
 */
export function addTensors(shapes: Float32Array, elements: number, first: number, weights: ArrayLike<number>, out: Float32Array): void {
	const per = elements * COMPONENTS;
	for (let b = 0; b < weights.length; b++) {
		const w = weights[b];
		if (w === 0) continue;
		const at = (first + b) * per;
		for (let i = 0; i < per; i++) out[i] += shapes[at + i] * w;
	}
}

/**
 * The summed tensor of each element at sample `k` of a modal motion: each mode's shape times its
 * modal coordinate there (`values` [M, S]).
 */
export function motionTensors(stress: ElementValues, values: Float32Array, samples: number, k: number, out: Float32Array): void {
	const modes = shapeCount(stress);
	const weights = new Float64Array(modes);
	for (let m = 0; m < modes; m++) weights[m] = values[m * samples + k];
	out.fill(0);
	addTensors(stress.shapes, elementCount(stress), 0, weights, out);
}

/**
 * Mode `m`'s tensor at a point of its cycle, as the motion is drawn: Re(S exp(i (theta - pi/2))),
 * `re` times the real part plus `im` times the imaginary part, with `re` = sin(theta) and
 * `im` = cos(theta) at the shape's own scale.
 */
export function modeTensors(stress: ElementValues, m: number, re: number, im: number, out: Float32Array): void {
	const elements = elementCount(stress);
	out.fill(0);
	addTensors(stress.shapes, elements, m, [re], out);
	if (stress.shapes_imag) addTensors(stress.shapes_imag, elements, m, [im], out);
}

/** Von Mises stress of the tensor at `at` in `t`: the square root of three times J2. */
export function vonMises(t: ArrayLike<number>, at = 0): number {
	return Math.sqrt(3 * secondInvariant(t, at));
}

/** J2 of the deviatoric stress. */
function secondInvariant(t: ArrayLike<number>, at: number): number {
	const [xx, yy, zz, xy, yz, xz] = [t[at], t[at + 1], t[at + 2], t[at + 3], t[at + 4], t[at + 5]];
	return ((xx - yy) ** 2 + (yy - zz) ** 2 + (zz - xx) ** 2) / 6 + xy * xy + yz * yz + xz * xz;
}

/**
 * The three principal stresses of the tensor at `at` in `t`, largest first: the closed form for a
 * symmetric 3x3 through its invariants and the Lode angle, so no iteration runs per element.
 */
export function principal(t: ArrayLike<number>, at = 0, out: [number, number, number] = [0, 0, 0]): [number, number, number] {
	const [xx, yy, zz, xy, yz, xz] = [t[at], t[at + 1], t[at + 2], t[at + 3], t[at + 4], t[at + 5]];
	const mean = (xx + yy + zz) / 3;
	const j2 = secondInvariant(t, at);
	if (!(j2 > 0)) {
		out[0] = out[1] = out[2] = mean;
		return out;
	}
	const sx = xx - mean;
	const sy = yy - mean;
	const sz = zz - mean;
	const j3 = sx * (sy * sz - yz * yz) - xy * (xy * sz - yz * xz) + xz * (xy * yz - sy * xz);
	const r = Math.min(1, Math.max(-1, ((3 * Math.sqrt(3)) / 2) * (j3 / j2 ** 1.5)));
	const lode = Math.acos(r) / 3;
	const radius = 2 * Math.sqrt(j2 / 3);
	out[0] = mean + radius * Math.cos(lode);
	out[1] = mean + radius * Math.cos(lode - (2 * Math.PI) / 3);
	out[2] = mean + radius * Math.cos(lode - (4 * Math.PI) / 3);
	return out;
}

const spare: [number, number, number] = [0, 0, 0];

/** One scalar of the tensor at `at` in `t`. */
export function scalarOf(t: ArrayLike<number>, scalar: StressScalar, at = 0): number {
	if (scalar === 'von_mises') return vonMises(t, at);
	if (scalar in COMPONENT_INDEX) return t[at + COMPONENT_INDEX[scalar]];
	const [s1, s2, s3] = principal(t, at, spare);
	if (scalar === 'max_principal') return s1;
	if (scalar === 'mid_principal') return s2;
	if (scalar === 'min_principal') return s3;
	// Tresca: the largest difference of principal stresses, twice the largest shear
	return s1 - s3;
}

/** Each element's scalar from its summed tensor (`tensors` f32 [E, 6]) into `out` [E]. */
export function scalars(tensors: Float32Array, scalar: StressScalar, out: Float32Array): void {
	for (let e = 0; e < out.length; e++) out[e] = scalarOf(tensors, scalar, e * COMPONENTS);
}

/**
 * Where a value falls on the color scale, 0 to 1: 0 to `max` for a scalar that cannot be negative,
 * -`max` to `max` for one that can. Values past the scale are held at its ends; a value that is not
 * a number, or a scale with no top (a mode that strains nothing), is drawn where zero is.
 */
export function scaleAt(value: number, max: number, isSigned: boolean): number {
	if (!(max > 0) || !Number.isFinite(value)) return isSigned ? 0.5 : 0;
	const t = isSigned ? (value / max + 1) / 2 : value / max;
	return Math.min(1, Math.max(0, t));
}

/** The legend's three ticks: the ends of the scale and its middle. */
export function legendValues(max: number, isSigned: boolean): number[] {
	return isSigned ? [-max, 0, max] : [0, max / 2, max];
}
