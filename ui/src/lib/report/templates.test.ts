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
});
