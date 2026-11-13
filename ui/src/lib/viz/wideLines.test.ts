import { describe, expect, it } from 'vitest';
import { BufferAttribute, Vector2, type WebGLRenderer } from 'three';
import { wideLines } from './wideLines';

const nodes = new Float32Array([0, 0, 0, 1, 0, 0, 1, 1, 0]);
const index = new Uint32Array([0, 1, 1, 2]);
const uniforms = { uRe: { value: 0 }, uIm: { value: 0 } };
const renderer = { getSize: (v: Vector2) => v.set(800, 600) } as unknown as WebGLRenderer;

function drawn(width?: number) {
	const shape = new BufferAttribute(new Float32Array(9), 3);
	const shapeImag = new BufferAttribute(new Float32Array(9), 3);
	const lines = wideLines(index, nodes, shape, shapeImag, uniforms, width === undefined ? {} : { width });
	return { lines, shape, shapeImag };
}

describe('tracelines and bar elements', () => {
	it('draw 3 px wide unless asked otherwise', () => {
		// at 2 px a lines-only test geometry was hard to see under a finite element mode (Brandon, 2026-11-13)
		expect(drawn().lines.material.linewidth).toBe(3);
	});

	it('keep the width a caller asks for', () => {
		expect(drawn(1).lines.material.linewidth).toBe(1);
	});

	it('draw each segment between its two nodes', () => {
		const start = drawn().lines.geometry.getAttribute('instanceStart');
		const end = drawn().lines.geometry.getAttribute('instanceEnd');
		expect([start.getX(1), start.getY(1), end.getX(1), end.getY(1)]).toEqual([1, 0, 1, 1]);
	});

	it('move with the shape, taking it up again when it changes', () => {
		const { lines, shape, shapeImag } = drawn();
		const ends = () => {
			const end = lines.geometry.getAttribute('shapeEnd');
			const imag = lines.geometry.getAttribute('shapeImagEnd');
			return [end.getX(1), end.getY(1), imag.getZ(1)];
		};
		shape.setXYZ(2, 0.5, -0.25, 0);
		shape.needsUpdate = true;
		shapeImag.setXYZ(2, 0, 0, 2);
		shapeImag.needsUpdate = true;
		lines.onBeforeRender(renderer, null!, null!, null!, null!, null!);
		expect(ends()).toEqual([0.5, -0.25, 2]);
		expect(lines.material.resolution.toArray()).toEqual([800, 600]);
		shape.setXYZ(2, 3, 0, 0);
		shape.needsUpdate = true;
		lines.onBeforeRender(renderer, null!, null!, null!, null!, null!);
		expect(ends()).toEqual([3, 0, 2]);
	});
});
