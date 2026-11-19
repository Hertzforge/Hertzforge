import { describe, expect, it } from 'vitest';
import { stepIndex, typeAhead } from './keys';

describe('moving through a list from the keyboard', () => {
	it('steps with the arrows and wraps at the ends', () => {
		expect(stepIndex('ArrowDown', 0, 3)).toBe(1);
		expect(stepIndex('ArrowDown', 2, 3)).toBe(0);
		expect(stepIndex('ArrowUp', 0, 3)).toBe(2);
		expect(stepIndex('ArrowRight', 1, 3, 'horizontal')).toBe(2);
		expect(stepIndex('ArrowDown', 1, 3, 'horizontal')).toBeNull();
	});

	it('starts at the first or the last when nothing has focus', () => {
		expect(stepIndex('ArrowDown', -1, 4)).toBe(0);
		expect(stepIndex('ArrowUp', -1, 4)).toBe(3);
	});

	it('goes to the ends with Home and End, and ignores other keys', () => {
		expect(stepIndex('Home', 2, 4)).toBe(0);
		expect(stepIndex('End', 0, 4)).toBe(3);
		expect(stepIndex('a', 0, 4)).toBeNull();
		expect(stepIndex('ArrowDown', 0, 0)).toBeNull();
	});

	it('jumps to the next entry starting with a letter', () => {
		const words = ['Control', 'Coherence', 'Drive', 'Time History'];
		expect(typeAhead(words, -1, 'c')).toBe(0);
		expect(typeAhead(words, 0, 'C')).toBe(1);
		expect(typeAhead(words, 1, 'c')).toBe(0);
		expect(typeAhead(words, 0, 't')).toBe(3);
		expect(typeAhead(words, 0, 'z')).toBeNull();
		expect(typeAhead(words, 0, ' ')).toBeNull();
	});
});
