/**
 * The shell's stress scalars and modal sum against `schema/golden/stress_scalars.json`, the numbers
 * the Python reference (`hertzforge.core.stress_scalars`) is held to as well: tensors whose
 * principal stresses are known by construction, and a two-mode sum whose von Mises comes from the
 * summed tensor.
 */
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import type { ElementValues, StressScalar } from '$lib/protocol/types';
import { legendValues, modeTensors, motionTensors, principal, scalarOf, scalars, scaleAt, signed, vonMises } from './stress';

interface Golden {
	cases: { name: string; tensor: number[]; expected: Record<StressScalar, number> }[];
	modal_sum: { modes: number; elements: number; samples: number; k: number; shapes: number[]; values: number[]; expected: Record<StressScalar, number>[] };
}

const GOLDEN = JSON.parse(readFileSync(fileURLToPath(new URL('../../../../schema/golden/stress_scalars.json', import.meta.url)), 'utf-8')) as Golden;
const SCALARS = Object.keys(GOLDEN.cases[0].expected) as StressScalar[];

function field(shapes: number[], elements: number, scalar: StressScalar = 'von_mises'): ElementValues {
	return {
		quantity: 'stress', label: 'Von Mises stress', unit: 'MPa', scalar, elements,
		triangles: new Uint32Array(0), owner: new Uint32Array(0),
		shapes: Float32Array.from(shapes), max: [1]
	};
}

describe('stress scalars', () => {
	for (const c of GOLDEN.cases) {
		it(`forms every scalar of the ${c.name} tensor`, () => {
			// the closed form loses a few digits where two principal stresses are equal (the arc
			// cosine is flat there): parts in a billion of the stress, far below one color step
			for (const s of SCALARS) expect(scalarOf(c.tensor, s), s).toBeCloseTo(c.expected[s], 5);
		});
	}

	it('orders the principal stresses largest first', () => {
		const general = GOLDEN.cases.find((c) => c.name === 'general')!;
		expect(principal(general.tensor)).toEqual([expect.closeTo(120, 9), expect.closeTo(10, 9), expect.closeTo(-35, 9)]);
	});

	it('gives a hydrostatic tensor no von Mises stress and three equal principal stresses', () => {
		expect(vonMises([-30, -30, -30, 0, 0, 0])).toBe(0);
		expect(principal([-30, -30, -30, 0, 0, 0])).toEqual([-30, -30, -30]);
	});
});

describe('the modal sum', () => {
	const g = GOLDEN.modal_sum;

	it("forms each element's scalars from the modes' shapes times the modal coordinates, summed first", () => {
		const stress = field(g.shapes, g.elements);
		const tensors = new Float32Array(g.elements * 6);
		motionTensors(stress, Float32Array.from(g.values), g.samples, g.k, tensors);
		for (let e = 0; e < g.elements; e++) {
			for (const s of SCALARS) expect(scalarOf(tensors, s, e * 6), `element ${e} ${s}`).toBeCloseTo(g.expected[e][s], 3);
		}
	});

	it('is not the modes’ von Mises stresses summed', () => {
		// von Mises is not linear in the modal coordinates: the sum of each mode's scaled von Mises
		// misses the summed tensor's by far more than rounding
		const stress = field(g.shapes, g.elements);
		const q = [g.values[g.k], g.values[g.samples + g.k]];
		const perMode = q.reduce((sum, w, m) => sum + Math.abs(w) * vonMises(stress.shapes, m * g.elements * 6), 0);
		expect(Math.abs(perMode - g.expected[0].von_mises)).toBeGreaterThan(1);
	});

	it('writes every element in one pass', () => {
		const stress = field(g.shapes, g.elements);
		const tensors = new Float32Array(g.elements * 6);
		motionTensors(stress, Float32Array.from(g.values), g.samples, g.k, tensors);
		const out = new Float32Array(g.elements);
		scalars(tensors, 'von_mises', out);
		expect(Array.from(out)).toEqual(g.expected.map((e) => expect.closeTo(e.von_mises, 3)));
	});

	it("draws a mode at a point of its cycle as its shape times the sine there", () => {
		const stress = field(g.shapes, g.elements);
		const tensors = new Float32Array(g.elements * 6);
		modeTensors(stress, 1, 0.5, 0.866, tensors);
		expect(tensors[0]).toBeCloseTo(0.5 * g.shapes[g.elements * 6], 4);
		// a quarter turn on from rest a real mode is at its full shape; at rest it is zero
		modeTensors(stress, 0, 0, 1, tensors);
		expect(Array.from(tensors).every((v) => v === 0)).toBe(true);
	});
});

describe('the color scale', () => {
	it('runs from 0 to the stated maximum for von Mises and from minus it to it for a component', () => {
		expect(scaleAt(50, 200, signed('von_mises'))).toBe(0.25);
		expect(scaleAt(-100, 200, signed('xx'))).toBe(0.25);
		expect(legendValues(200, false)).toEqual([0, 100, 200]);
		expect(legendValues(200, true)).toEqual([-200, 0, 200]);
	});

	it('holds a value past the stated maximum at the top, not stretching the scale', () => {
		expect(scaleAt(900, 200, false)).toBe(1);
		expect(scaleAt(-900, 200, true)).toBe(0);
	});

	it("draws NaN and a zero maximum at zero's place: the bottom, or the middle for a signed scalar", () => {
		expect(scaleAt(Number.NaN, 200, false)).toBe(0);
		expect(scaleAt(5, 0, false)).toBe(0);
		// a rigid-body mode strains nothing: its elements read zero, in the middle of a signed scale
		expect(scaleAt(Number.NaN, 200, true)).toBe(0.5);
		expect(scaleAt(0, 0, true)).toBe(0.5);
	});
});
