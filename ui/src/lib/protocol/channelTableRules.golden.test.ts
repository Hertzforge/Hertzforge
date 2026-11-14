/**
 * The channel table's one rule set, as the golden table file holds it
 * (`schema/golden/channel_table_rules.json`; PROTOCOL.md, "Channel Table"): the shell's mock control
 * service answers each case as the control service does, and knows every unit of the vocabulary.
 */
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import type { ChannelRow } from '$lib/protocol/types';
import { UNIT_VOCABULARY, badRowParam, validateTable } from '$lib/protocol/mock/control';

interface RuleCase {
	name: string;
	rule: string;
	rows: ChannelRow[];
	control: 'ok' | 'table_invalid' | 'bad_params';
	errors: string[];
	warnings: string[];
	table_file: 'held' | 'refused';
}

const RULES = JSON.parse(readFileSync(fileURLToPath(new URL('../../../../schema/golden/channel_table_rules.json', import.meta.url)), 'utf-8')) as {
	unit_vocabulary: string[];
	cases: RuleCase[];
};
const SIM = { kind: 'sim' as const, devices: [] };

describe('the channel table rules', () => {
	for (const c of RULES.cases) {
		it(c.name, () => {
			if (badRowParam(c.rows) !== null) {
				expect(c.control).toBe('bad_params');
				return;
			}
			const report = validateTable(c.rows, SIM, []);
			expect(report.errors.map((e) => e.reason)).toEqual(c.errors);
			expect(report.errors.length ? 'table_invalid' : 'ok').toBe(c.control);
			const warnings = report.warnings.map((w) => w.reason as string);
			for (const w of c.warnings) expect(warnings).toContain(w);
		});
	}

	it('every unit of the vocabulary is known, and the vocabulary is the one the file holds', () => {
		expect(UNIT_VOCABULARY).toEqual(RULES.unit_vocabulary);
		const base = RULES.cases[0].rows;
		for (const unit of RULES.unit_vocabulary) {
			const report = validateTable([...base, { ...base[0], index: 9, role: 'monitor', units: unit }], SIM, []);
			expect(report.warnings.map((w) => w.reason)).not.toContain('unit_unknown');
		}
	});
});
