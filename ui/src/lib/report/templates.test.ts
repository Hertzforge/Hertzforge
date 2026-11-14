import { describe, expect, it } from 'vitest';
import { defaultTemplate, TEMPLATES } from './templates';

describe('the template a module makes by default', () => {
	it('is the module own template', () => {
		expect(defaultTemplate('modal')).toBe('modal');
		expect(defaultTemplate('geometry')).toBe('model');
		expect(defaultTemplate('specify')).toBe('specify');
		expect(defaultTemplate('signals')).toBe('signals');
	});

	it('is the test that ran, from Acquire and Control', () => {
		expect(defaultTemplate('acquire', 'mimo_random')).toBe('mimo');
		expect(defaultTemplate('acquire', 'srs')).toBe('shock');
		expect(defaultTemplate('acquire', null)).toBe('modal');
	});

	it('is the modal template with nothing to go on, and always one offered', () => {
		expect(defaultTemplate(null)).toBe('modal');
		expect(defaultTemplate('report')).toBe('modal');
		const offered = new Set<string>(TEMPLATES.map(([t]) => t));
		for (const m of ['modal', 'random', 'shock', 'transient', 'sine', 'sysid', 'geometry', 'specify', 'signals', 'simulate'] as const) {
			expect(offered.has(defaultTemplate(m))).toBe(true);
		}
	});

	it('is each recorded test own template, from Acquire and Control', () => {
		expect(defaultTemplate('acquire', 'mixed')).toBe('mixed_mode');
		expect(defaultTemplate('acquire', 'explorer')).toBe('explorer');
		expect(defaultTemplate('acquire', 'recorder')).toBe('recorder');
		expect(defaultTemplate('acquire', 'mimo_shock')).toBe('mimo_shock');
		expect(defaultTemplate('acquire', 'twr')).toBe('transient');
	});

	it('is the run choice for a project that holds a run record', () => {
		expect(defaultTemplate('sine', null, true)).toBe('run');
		expect(defaultTemplate('acquire', 'mixed', true)).toBe('run');
		expect(defaultTemplate(null, null, true)).toBe('run');
		// a model, a specification or signals keeps its own report
		expect(defaultTemplate('specify', null, true)).toBe('specify');
		expect(defaultTemplate('geometry', null, true)).toBe('model');
		expect(defaultTemplate('signals', null, true)).toBe('signals');
		// and without a record nothing changes
		expect(defaultTemplate('sine', null, false)).toBe('sine');
	});

	it('offers every template the analysis service builds', () => {
		const offered = new Set<string>(TEMPLATES.map(([t]) => t));
		for (const t of ['mixed_mode', 'explorer', 'recorder', 'mimo_shock', 'run']) {
			expect(offered.has(t)).toBe(true);
		}
	});
});
