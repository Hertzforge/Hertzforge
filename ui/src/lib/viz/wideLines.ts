import {
	InstancedInterleavedBuffer,
	InterleavedBufferAttribute,
	Vector2,
	type BufferAttribute,
	type ColorRepresentation,
	type WebGLRenderer
} from 'three';
import { LineMaterial } from 'three/addons/lines/LineMaterial.js';
import { LineSegments2 } from 'three/addons/lines/LineSegments2.js';
import { LineSegmentsGeometry } from 'three/addons/lines/LineSegmentsGeometry.js';

/** How wide tracelines and bar elements draw, in pixels. At 2 px a lines-only test geometry was
 * hard to see under a finite element mode shape (Brandon, 2026-11-13); WebGL's own lines are 1 px. */
export const LINE_WIDTH = 3;

export interface WideLineOptions {
	color?: ColorRepresentation;
	opacity?: number;
	transparent?: boolean;
	/** Pixels; `LINE_WIDTH` unless the caller asks for another. */
	width?: number;
}

/** The node pairs `index` names, gathered per segment: [start xyz, end xyz] for each. */
export function gathered(index: Uint32Array, values: ArrayLike<number>, out: Float32Array): Float32Array {
	for (let s = 0; s < index.length / 2; s++) {
		const a = index[2 * s] * 3;
		const b = index[2 * s + 1] * 3;
		const o = 6 * s;
		out[o] = values[a];
		out[o + 1] = values[a + 1];
		out[o + 2] = values[a + 2];
		out[o + 3] = values[b];
		out[o + 4] = values[b + 1];
		out[o + 5] = values[b + 2];
	}
	return out;
}

/**
 * The segments `index` names over `position`, drawn `width` pixels wide and moved, as the
 * other displaced materials are, by `shape * uRe + shapeImag * uIm`. The shape attributes are
 * gathered per segment before a frame only when they changed.
 */
export function wideLines(
	index: Uint32Array,
	position: Float32Array,
	shape: BufferAttribute,
	shapeImag: BufferAttribute,
	uniforms: { uRe: { value: number }; uIm: { value: number } },
	options: WideLineOptions = {}
): LineSegments2 {
	const geometry = new LineSegmentsGeometry();
	const count = index.length / 2;
	geometry.setPositions(gathered(index, position, new Float32Array(count * 6)));
	const real = new InstancedInterleavedBuffer(new Float32Array(count * 6), 6, 1);
	const imag = new InstancedInterleavedBuffer(new Float32Array(count * 6), 6, 1);
	geometry.setAttribute('shapeStart', new InterleavedBufferAttribute(real, 3, 0));
	geometry.setAttribute('shapeEnd', new InterleavedBufferAttribute(real, 3, 3));
	geometry.setAttribute('shapeImagStart', new InterleavedBufferAttribute(imag, 3, 0));
	geometry.setAttribute('shapeImagEnd', new InterleavedBufferAttribute(imag, 3, 3));

	const material = new LineMaterial({
		color: options.color ?? 0xffffff,
		linewidth: options.width ?? LINE_WIDTH,
		transparent: options.transparent ?? false,
		opacity: options.opacity ?? 1
	});
	material.onBeforeCompile = (shader) => {
		shader.uniforms.uRe = uniforms.uRe;
		shader.uniforms.uIm = uniforms.uIm;
		shader.vertexShader = shader.vertexShader
			.replace(
				'#include <common>',
				'#include <common>\nattribute vec3 shapeStart;\nattribute vec3 shapeEnd;\nattribute vec3 shapeImagStart;\nattribute vec3 shapeImagEnd;\nuniform float uRe;\nuniform float uIm;'
			)
			.replace(
				'vec4 start = modelViewMatrix * vec4( instanceStart, 1.0 );',
				'vec4 start = modelViewMatrix * vec4( instanceStart + shapeStart * uRe + shapeImagStart * uIm, 1.0 );'
			)
			.replace(
				'vec4 end = modelViewMatrix * vec4( instanceEnd, 1.0 );',
				'vec4 end = modelViewMatrix * vec4( instanceEnd + shapeEnd * uRe + shapeImagEnd * uIm, 1.0 );'
			);
	};
	material.customProgramCacheKey = () => 'sds-wide-displaced';

	const lines = new LineSegments2(geometry, material);
	lines.frustumCulled = false;
	let seen = [-1, -1];
	const size = new Vector2();
	lines.onBeforeRender = (renderer: WebGLRenderer) => {
		material.resolution.copy(renderer.getSize(size));
		if (shape.version !== seen[0]) {
			gathered(index, shape.array, real.array as Float32Array);
			real.needsUpdate = true;
		}
		if (shapeImag.version !== seen[1]) {
			gathered(index, shapeImag.array, imag.array as Float32Array);
			imag.needsUpdate = true;
		}
		seen = [shape.version, shapeImag.version];
	};
	return lines;
}
