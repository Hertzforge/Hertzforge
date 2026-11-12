import { describe, expect, it } from 'vitest';
import { splitOnFaces } from './edges';

describe('the lines on the faces', () => {
	// a unit quad as two triangles (0 1 2, 0 2 3), its four sides, and a beam out to node 4
	const triangles = Uint32Array.from([0, 1, 2, 0, 2, 3]);

	it('an element edge is on a face, a beam is not, whichever way round it is written', () => {
		const lines = Uint32Array.from([0, 1, 2, 1, 2, 3, 3, 0, 3, 4]);
		const { onFaces, apart } = splitOnFaces(lines, triangles);
		expect(Array.from(onFaces)).toEqual([0, 1, 2, 1, 2, 3, 3, 0]);
		expect(Array.from(apart)).toEqual([3, 4]);
	});

	it('with no faces every line is apart', () => {
		const lines = Uint32Array.from([0, 1, 1, 2]);
		const { onFaces, apart } = splitOnFaces(lines);
		expect(onFaces.length).toBe(0);
		expect(apart).toBe(lines);
	});
});
