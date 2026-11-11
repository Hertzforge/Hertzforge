import { describe, expect, it } from 'vitest';
import { readReport } from './read';

function base64(text: string): string {
	return btoa(String.fromCharCode(...new TextEncoder().encode(text)));
}

describe('reading the report an exported page holds', () => {
	it('decodes the frame the core wrote', () => {
		const result = { name: 'R', title: 'Modal Test Report', marking: '', css: '', blocks: [{ kind: 'text', block: 0, label: '', caption: '', html: '<p>Hi</p>' }] };
		const page = readReport(base64(JSON.stringify({ type: 'response', id: 0, result })));
		expect(page.title).toBe('Modal Test Report');
		expect(page.blocks[0].html).toBe('<p>Hi</p>');
	});

	it('says so when the page holds none', () => {
		expect(() => readReport('<!--HERTZFORGE-REPORT-DATA-->')).toThrow('holds no report');
		expect(() => readReport('   ')).toThrow('holds no report');
	});
});
