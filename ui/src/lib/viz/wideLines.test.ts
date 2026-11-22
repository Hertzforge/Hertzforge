import { describe, expect, it } from 'vitest';
import { BufferAttribute, Color, Vector2, type WebGLRenderer } from 'three';
import { fixedWideLines, LINE_WIDTH, wideLines } from './wideLines';

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
		lines.onBeforeRender(renderer);
		expect(ends()).toEqual([0.5, -0.25, 2]);
		expect(lines.material.resolution.toArray()).toEqual([800, 600]);
		shape.setXYZ(2, 3, 0, 0);
		shape.needsUpdate = true;
		lines.onBeforeRender(renderer);
		expect(ends()).toEqual([3, 0, 2]);
	});
});

describe('tracelines colored by deflection', () => {
	// the material's own shaders, as three hands them to onBeforeCompile
	const compiled = (scalar: boolean) => {
		const shape = new BufferAttribute(new Float32Array(9), 3);
		const shapeImag = new BufferAttribute(new Float32Array(9), 3);
		const held = { uPeak: { value: 1 }, uMap: { value: null }, uScalar: { value: 1 } };
		const lines = wideLines(index, nodes, shape, shapeImag, uniforms, scalar ? { scalar: held } : {});
		const material = lines.material;
		const shader = { uniforms: {} as Record<string, unknown>, vertexShader: material.vertexShader, fragmentShader: material.fragmentShader };
		material.onBeforeCompile(shader as never, undefined as never);
		return { shader, held, key: material.customProgramCacheKey() };
	};

	it('look each end\'s travel up in the colormap, as the faces do', () => {
		const { shader, held, key } = compiled(true);
		expect(shader.uniforms.uMap).toBe(held.uMap);
		expect(shader.uniforms.uPeak).toBe(held.uPeak);
		expect(shader.uniforms.uScalar).toBe(held.uScalar);
		expect(shader.vertexShader).toContain('vTravel = length( position.y < 0.5 ? shapeStart * uRe + shapeImagStart * uIm : shapeEnd * uRe + shapeImagEnd * uIm ) / uPeak;');
		expect(shader.fragmentShader).toContain('if (uScalar > 0.5) diffuseColor.rgb = texture2D(uMap, vec2(clamp(vTravel, 0.0, 1.0), 0.5)).rgb;');
		// still moved with the shape
		expect(shader.vertexShader).toContain('instanceStart + shapeStart * uRe + shapeImagStart * uIm');
		expect(key).toBe('sds-wide-displaced-scalar');
	});

	it('keep their one color when not asked to', () => {
		const { shader, key } = compiled(false);
		expect(shader.vertexShader).not.toContain('vTravel');
		expect(shader.fragmentShader).not.toContain('uMap');
		expect(key).toBe('sds-wide-displaced');
	});
});

describe('fixed wide lines', () => {
	it('draws one wide segment per pair of rows, over the surface, in its color', () => {
		const xyz = new Float32Array([0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 1, 2]);
		const lines = fixedWideLines(xyz, new Color(1, 0.5, 0));
		const start = lines.geometry.getAttribute('instanceStart');
		const end = lines.geometry.getAttribute('instanceEnd');
		expect(start.count).toBe(2);
		expect([start.getX(1), start.getY(1), start.getZ(1)]).toEqual([0, 1, 0]);
		expect([end.getX(1), end.getY(1), end.getZ(1)]).toEqual([0, 1, 2]);
		expect(lines.material.linewidth).toBe(LINE_WIDTH);
		expect(lines.material.depthTest).toBe(false);
		expect(lines.material.color.getHex()).toBe(new Color(1, 0.5, 0).getHex());
	});
});
