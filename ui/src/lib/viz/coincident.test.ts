import { describe, expect, it } from 'vitest';
import { coincidentGroups, nodeCaption } from './coincident';

describe('coincident nodes', () => {
	const nodes = new Float32Array([0, 0, 0, 1, 0, 0, 0, 0, 0, 2, 2, 2, 1, 0, 0]);
	const ids = new Uint32Array([104, 7, 200, 9, 8]);

	it('groups the nodes at one place, and leaves lone nodes out', () => {
		const groups = coincidentGroups(nodes);
		expect(groups.get(0)).toEqual([0, 2]);
		expect(groups.get(2)).toEqual([0, 2]);
		expect(groups.get(1)).toEqual([1, 4]);
		expect(groups.has(3)).toBe(false);
	});

	it('captions a lone node by its id and a shared place by every id there', () => {
		const groups = coincidentGroups(nodes);
		expect(nodeCaption(3, ids, groups)).toBe('node 9');
		expect(nodeCaption(0, ids, groups)).toBe('nodes 104, 200 (2 here)');
		expect(nodeCaption(3, undefined, groups)).toBe('node 4');
	});
});
