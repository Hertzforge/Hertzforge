/**
 * The reference sub-family's goldens (schema/golden/, `mimo_random.reference.*`) through the
 * shell's codec and types: each decodes and re-encodes as the same text frame, and the typed
 * readings below are the goldens' content written against the TypeScript types.
 */
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { decodeFrame, encodeFrame } from './codec';
import type { Request, ResponseOk } from './types';
import type { MimoBuildBreakpointsParams, MimoImportParams, MimoReferenceResult, MimoRigidParams, MimoRigidResult } from './mimo';

const golden = (name: string) => fileURLToPath(new URL(`../../../../schema/golden/${name}`, import.meta.url));
const readJson = (name: string) => JSON.parse(readFileSync(golden(name), 'utf-8'));

const TEXT = [
	'request_mimo_random_reference_build_breakpoints',
	'request_mimo_random_reference_build_rigid_body',
	'response_mimo_random_reference_build_rigid_body',
	'request_mimo_random_reference_import',
	'response_mimo_random_reference_import'
];

describe('reference goldens', () => {
	for (const name of TEXT) {
		it(`${name} decodes and re-encodes as the same compact text frame`, () => {
			const text = readFileSync(golden(`${name}.json`), 'utf-8');
			const message = decodeFrame(text);
			expect(message).toEqual(JSON.parse(text));
			expect(encodeFrame(message)).toBe(JSON.stringify(JSON.parse(text)));
		});
	}

	it('the breakpoint request names three tables, three pairs and the preview pair', () => {
		const req = readJson('request_mimo_random_reference_build_breakpoints.json') as Request<MimoBuildBreakpointsParams>;
		expect(req.method).toBe('mimo_random.reference.build_breakpoints');
		expect(req.params.tables.map((t) => t.label)).toEqual(['Front', 'Rear', 'Side']);
		expect(req.params.pairs?.[2].coherence).toEqual([
			[20, 0.9],
			[500, 0.9]
		]);
		expect(req.params.preview_pair).toEqual([1, 2]);
	});

	it('the rigid-body answer gives the six degrees of freedom and a share per sensor', () => {
		const req = readJson('request_mimo_random_reference_build_rigid_body.json') as Request<MimoRigidParams>;
		const res = readJson('response_mimo_random_reference_build_rigid_body.json') as ResponseOk<MimoRigidResult>;
		expect(res.result.dofs).toEqual(['X', 'Y', 'Z', 'RX', 'RY', 'RZ']);
		expect(res.result.transform?.rows).toBe(6);
		expect(res.result.transform?.cols).toBe(req.params.sensors.length);
		expect(res.result.dof_units?.[3]).toBe('g/m');
		const shares = res.result.sensors.reduce((n, s) => n + s.check_share, 0);
		expect(shares).toBeCloseTo(1, 9);
	});

	it('an import of a file it does not read says what it reads, with no preview', () => {
		const req = readJson('request_mimo_random_reference_import.json') as Request<MimoImportParams>;
		const res = readJson('response_mimo_random_reference_import.json') as ResponseOk<MimoReferenceResult>;
		expect(req.params.path.endsWith('.mat')).toBe(true);
		expect(res.result.ok).toBe(false);
		expect(res.result.reference).toBeNull();
		expect(res.result.errors[0].reason).toBe('reference_file');
		expect(res.result.errors[0].message).toContain('.npz');
		expect(res.result.views).toEqual([]);
	});
});
