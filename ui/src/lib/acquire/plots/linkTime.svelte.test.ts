import { describe, expect, it } from 'vitest';
import { TimeLinks } from './linkTime.svelte';

describe('linked time axes', () => {
	it('gives every plot of a link the same range, freeze and window', () => {
		const links = new TimeLinks();
		links.join('page-1');
		links.join('page-1');
		links.set('page-1', { x: [1, 2], frozen: true });
		expect(links.get('page-1')).toEqual({ x: [1, 2], frozen: true, window: undefined });
		links.set('page-1', { window: 0.5 });
		expect(links.get('page-1').window).toBe(0.5);
		expect(links.get('page-1').x).toEqual([1, 2]);
	});

	it('keeps links apart', () => {
		const links = new TimeLinks();
		links.set('a', { frozen: true });
		expect(links.get('b').frozen).toBe(false);
	});

	it('ends a link when its last plot leaves', () => {
		const links = new TimeLinks();
		const leaveA = links.join('k');
		const leaveB = links.join('k');
		links.set('k', { x: [0, 1] });
		leaveA();
		expect(links.size('k')).toBe(1);
		expect(links.get('k').x).toEqual([0, 1]);
		leaveB();
		expect(links.size('k')).toBe(0);
		expect(links.get('k').x).toBeNull();
	});
});
