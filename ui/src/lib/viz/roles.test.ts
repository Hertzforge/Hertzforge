import { describe, expect, it } from 'vitest';
import { ROLE_STYLES, seriesLooks } from './roles';

describe('series looks', () => {
	it('steps measured series through the blue family', () => {
		const looks = seriesLooks([{ role: 'measured' }, { role: 'measured' }, { role: 'reference' }]);
		expect(looks.map((l) => l.colorVar)).toEqual(['--series-measured', '--series-measured-2', '--series-reference']);
		expect(looks.every((l) => !l.follows)).toBe(true);
	});

	it('draws a follower dashed in its measurement\'s color, just above it', () => {
		const looks = seriesLooks([{ role: 'measured' }, { role: 'measured' }, { role: 'measured', follows: 0 }, { role: 'measured', follows: 1 }]);
		expect(looks[2].colorVar).toBe(looks[0].colorVar);
		expect(looks[3].colorVar).toBe(looks[1].colorVar);
		expect(looks[2].style.dash).toEqual(ROLE_STYLES.reference.dash);
		expect(looks[2].style.z).toBeGreaterThan(looks[0].style.z);
		expect(looks[2].follows && looks[3].follows).toBe(true);
	});

	it('does not count a follower among the colors, so the next measurement keeps its own', () => {
		const looks = seriesLooks([{ role: 'measured' }, { role: 'measured', follows: 0 }, { role: 'measured' }]);
		expect(looks[2].colorVar).toBe('--series-measured-2');
	});

	it('ignores a follows that names itself, a later series or another follower', () => {
		// 1 follows 0 properly; 2 names a later series, 3 a follower, 4 itself
		const looks = seriesLooks([
			{ role: 'measured' },
			{ role: 'measured', follows: 0 },
			{ role: 'measured', follows: 3 },
			{ role: 'measured', follows: 1 },
			{ role: 'measured', follows: 4 }
		]);
		expect(looks.map((l) => l.follows)).toEqual([false, true, false, false, false]);
	});
});

describe('many measured curves', () => {
	it('gives each its own color along viridis once the blue family runs out', () => {
		const series = Array.from({ length: 5 }, () => ({ role: 'measured' as const }));
		const looks = seriesLooks(series);
		const colors = looks.map((l) => l.color);
		expect(new Set(colors).size).toBe(5);
		expect(colors[0]).toBe('rgb(68 1 84)');
	});

	it('keeps the blue family for three or fewer, and a follower takes its measurement\'s color', () => {
		expect(seriesLooks([{ role: 'measured' }, { role: 'measured' }]).every((l) => l.color === undefined)).toBe(true);
		const looks = seriesLooks([
			{ role: 'measured' }, { role: 'measured' }, { role: 'measured' }, { role: 'measured' }, { role: 'measured', follows: 1 }
		]);
		expect(looks[4].follows).toBe(true);
		expect(looks[4].color).toBe(looks[1].color);
		expect(new Set(looks.slice(0, 4).map((l) => l.color)).size).toBe(4);
	});
});
