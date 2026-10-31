import { describe, expect, it } from 'vitest';
import { fuzzyMatch, highlightRuns } from './fuzzy';

describe('fuzzy matching', () => {
	it('matches characters in order and rejects the rest', () => {
		expect(fuzzyMatch('psd', 'PSD Against Specification')).not.toBeNull();
		expect(fuzzyMatch('stsim', 'Start Simulation')).not.toBeNull();
		expect(fuzzyMatch('zzz', 'Start Simulation')).toBeNull();
	});

	it('ranks word starts and substrings above scattered hits', () => {
		const start = fuzzyMatch('sim', 'Start Simulation')!.score;
		const scattered = fuzzyMatch('sim', 'Transmissibility Map')!.score;
		expect(start).toBeGreaterThan(scattered);
	});

	it('returns indices for highlighting', () => {
		const m = fuzzyMatch('modal', 'Go to Modal')!;
		expect(highlightRuns('Go to Modal', m.indices)).toEqual([
			{ text: 'Go to ', hit: false },
			{ text: 'Modal', hit: true }
		]);
	});
});
