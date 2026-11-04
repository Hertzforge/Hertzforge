import { describe, expect, it } from 'vitest';
import {
	CHILD_DRAW,
	KIND_COLORS,
	KIND_ORDER,
	OBJECT_DRAW,
	QUANTITY_COLORS,
	QUANTITY_DRAW,
	drawChild,
	drawObject,
	drawRecord,
	kindRank,
	objectIcon,
	solidIcon
} from './icons';

/** The tags the markup may hold: SVG shapes and the two it needs for a cut and a badge. */
const ALLOWED = /<\/?(line|polyline|polygon|circle|ellipse|rect|path|g|text|defs|mask|clipPath|title)\b/g;

function wellFormed(markup: string): boolean {
	const tags = markup.match(/<[^>]+>/g) ?? [];
	return tags.every((tag) => ALLOWED.test(tag) || /^<\/?(line|polyline|polygon|circle|ellipse|rect|path|g|text|defs|mask|clipPath)[\s>/]/.test(tag));
}

describe('every kind the tree lists has a drawing of its own', () => {
	it('object kinds draw, in their colors, and no two drawings are the same', () => {
		const seen = new Map<string, string>();
		for (const kind of KIND_ORDER) {
			const markup = drawObject(objectIcon(kind));
			expect(markup.length, kind).toBeGreaterThan(40);
			expect(wellFormed(markup), kind).toBe(true);
			expect(markup, kind).toContain(KIND_COLORS[kind]);
			seen.set(kind, markup);
		}
		// the sine family and the two coherences share drawings on purpose; everything else differs
		const distinct = new Set(seen.values());
		expect(distinct.size).toBeGreaterThanOrEqual(KIND_ORDER.length - 5);
		expect(seen.get('psd')).not.toEqual(drawObject('octave_psd'));
		expect(seen.get('specification')).not.toEqual(drawObject('octave_specification'));
	});

	it('an unknown kind falls back to the plain circle, never to nothing', () => {
		expect(drawObject('mystery')).toContain('<circle');
	});

	it('quantities draw, and the kinematic three differ by their dots', () => {
		for (const quantity of Object.keys(QUANTITY_COLORS)) {
			const markup = QUANTITY_DRAW[quantity](QUANTITY_COLORS[quantity]);
			expect(markup.length, quantity).toBeGreaterThan(40);
			expect(wellFormed(markup), quantity).toBe(true);
		}
		const dots = (q: string) => (drawRecord(q, 'time_history').match(/cy="13"/g) ?? []).length;
		expect([dots('length'), dots('velocity'), dots('acceleration')]).toEqual([0, 1, 2]);
		expect([dots('angle'), dots('angular_velocity'), dots('angular_acceleration')]).toEqual([0, 1, 2]);
	});

	it('a fraction draws both quantities and a slash; an empty key draws the object', () => {
		const frf = drawRecord('acceleration/force', 'frf');
		expect(frf).toContain(QUANTITY_COLORS.acceleration);
		expect(frf).toContain(QUANTITY_COLORS.force);
		expect(frf).toContain('#8e8e93');
		expect(drawRecord('', 'coherence')).toEqual(drawObject('coherence'));
		expect(drawRecord('acceleration', 'time_history', true)).toContain('?');
	});

	it('sub-items draw in the parent’s color, or gray when empty', () => {
		for (const kind of Object.keys(CHILD_DRAW)) expect(wellFormed(drawChild(kind, 'geometry')), kind).toBe(true);
		expect(drawChild('beams', 'geometry')).toContain(KIND_COLORS.geometry);
		expect(drawChild('beams', 'geometry', true)).not.toContain(KIND_COLORS.geometry);
		expect(drawChild('mode', 'shape_set')).toContain(KIND_COLORS.shape_set);
		expect(solidIcon('block')).toBe('solid_block');
		expect(solidIcon('I-beam')).toBe('solid_section');
	});

	it('kinds rank in Visual Dynamics’ order, specifications beside their measurements', () => {
		expect(kindRank('geometry')).toBeLessThan(kindRank('channel_table'));
		expect(kindRank('specification') - kindRank('psd')).toBe(1);
		expect(kindRank('shock_specification') - kindRank('srs')).toBe(1);
		expect(kindRank('report')).toBe(KIND_ORDER.length - 1);
		expect(kindRank('mystery')).toBe(KIND_ORDER.length);
	});

	it('the shapes with cuts give each instance ids of its own', () => {
		const one = CHILD_DRAW.solid_subtract('#fff');
		const two = CHILD_DRAW.solid_subtract('#fff');
		expect(one).not.toEqual(two);
	});
});
