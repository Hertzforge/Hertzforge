/**
 * Mock analysis service: the hello and an empty catalog. The analysis runs on a project, which
 * only the real service can open, so `?mock=1` demonstrates the shell's other halves (the control
 * mock) and leaves Analyze waiting for a file.
 */

import type { Catalog } from '../types';
import type { MockServer } from './socket';

export function createAnalysisServer(): MockServer {
	return {
		hello: { type: 'hello', protocol: 'sds/1', service: 'analysis', version: '0.1.0-mock' },
		methods: {
			'catalog.list': (): Catalog => ({ operations: [] })
		}
	};
}
