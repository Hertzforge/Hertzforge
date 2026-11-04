/**
 * The MIMO Random and control-law goldens (schema/golden/) through the shell's codec and types.
 * Typed literals are the goldens' content written against the TypeScript types, so the type
 * check proves the types accept what the contract says.
 */
import { readdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { decodeFrame, encodeFrame, shapeOf, withShape, dtypeOf } from './codec';
import type { Dtype, Event, HeatmapView, Request, ResponseErr, ResponseOk, SdmView, SpectrumView } from './types';
import type {
	LawsImportResult,
	LawsListResult,
	MimoReportResult,
	MimoSetupParams,
	MimoSetupResult,
	MimoStatus,
	MimoValidation,
	MimoOverrideParams,
	MimoData
} from './mimo';
import { MIMO_TOPICS, packedCoherence, packedElement, packedIndex } from './mimo';

const golden = (name: string) => fileURLToPath(new URL(`../../../../schema/golden/${name}`, import.meta.url));
const readJson = (name: string) => JSON.parse(readFileSync(golden(name), 'utf-8'));

/**
 * Every MIMO Random and control-law text golden in schema/golden/, read from the folder itself so
 * this suite, the Rust suite and the Python suite all read the same files: a golden added there
 * is checked here without a list to update.
 */
const TEXT = readdirSync(fileURLToPath(new URL('../../../../schema/golden/', import.meta.url)))
	.filter((f) => /^(request|response|event)_.*(mimo_random|laws)_.*\.json$/.test(f) && !f.endsWith('.expected.json'))
	.map((f) => f.replace(/\.json$/, ''))
	.concat(['response_error_setup_invalid'])
	.sort();

describe('MIMO text goldens', () => {
	for (const name of TEXT) {
		it(`${name} decodes and re-encodes as the same compact text frame`, () => {
			const text = readFileSync(golden(`${name}.json`), 'utf-8');
			const message = decodeFrame(text);
			expect(message).toEqual(JSON.parse(text));
			expect(encodeFrame(message)).toBe(JSON.stringify(JSON.parse(text)));
		});
	}
});

describe('the goldens this suite reads', () => {
	it('include every new MIMO golden by name', () => {
		for (const name of [
			'request_mimo_random_subscribe_coh_error',
			'request_mimo_random_identification_load',
			'request_mimo_random_override_prediction',
			'request_mimo_random_setup_virtual',
			'request_mimo_random_subscribe_pred_sdm'
		])
			expect(TEXT).toContain(name);
	});
});

describe('the wave D additions match their types', () => {
	it('a prediction override names what it acknowledges', () => {
		const req = readJson('request_mimo_random_override_prediction.json') as Request<MimoOverrideParams>;
		expect(req.params.reason).toBe('prediction_exceeded');
	});

	it('virtual control carries one band pair per degree of freedom', () => {
		const req = readJson('request_mimo_random_setup_virtual.json') as Request<MimoSetupParams>;
		const space = req.params.setup.space;
		if (space?.kind !== 'virtual') throw new Error('virtual expected');
		expect(space.dof_bands?.map((b) => b.abort_db)).toEqual([
			[4, 4],
			[6, 6]
		]);
		expect(req.params.setup.control?.law_misses).toBe(2);
	});

	it('the status carries each quantity\'s bands, every override and the prediction\'s additions', () => {
		const ev = readJson('event_mimo_random_status.json') as Event<MimoStatus>;
		expect(ev.data.shape?.bands?.length).toBe(2);
		expect(ev.data.overrides).toEqual([]);
		expect(ev.data.prediction?.drive_accel).toEqual([1.25, 1.1]);
		expect(ev.data.prediction?.feasibility?.[1].verdict).toBe('not_evaluated');
	});
});

describe('MIMO messages match their types', () => {
	it('the setup request carries a packed two-channel reference', () => {
		const req = readJson('request_mimo_random_setup.json') as Request<MimoSetupParams>;
		expect(req.method).toBe('mimo_random.setup');
		const { setup } = req.params;
		expect(setup.channels.controls.map((c) => c.channel)).toEqual([1, 2]);
		expect(setup.channels.drives.map((d) => d.ceiling_vrms)).toEqual([2, 2]);
		const ref = setup.reference;
		if (ref.kind !== 'inline') throw new Error('inline reference expected');
		expect(ref.re.length).toBe(ref.freqs_hz.length * ((ref.n * (ref.n + 1)) / 2));
		// G01 on the first line: coherence 0.25
		const g01 = Math.hypot(ref.re[1], ref.im[1]);
		expect((g01 * g01) / (ref.re[0] * ref.re[2])).toBeCloseTo(0.25, 12);
		expect(setup.law).toEqual({ id: 'pinv_closed', params: { convergence: 0.5 } });
	});

	it('the setup result is a configured status with its validation', () => {
		const res = readJson('response_mimo_random_setup.json') as ResponseOk<MimoSetupResult>;
		const { state, validation } = res.result;
		expect([state.state, state.phase, state.loop]).toEqual(['configured', 'idle', 0]);
		expect(state.shape?.labels).toEqual(['Control X', 'Control Y']);
		expect(state.grid).toEqual({ fs_hz: 8192, n_pts: 4096, df_hz: 2, lines: 8, loop_s: 2.25, actual: false });
		const v: MimoValidation = validation;
		expect(v.ok).toBe(true);
		expect(v.reference).toMatchObject({ source: 'inline', rank_min: 2, rank_max: 2, repaired_lines: 0 });
		expect(v.feasibility.map((f) => f.verdict)).toEqual(['not_evaluated', 'not_evaluated']);
		expect(v.law?.admitted).toBe(true);
	});

	it('an invalid setup is refused with the whole report as its detail', () => {
		const err = readJson('response_error_setup_invalid.json') as ResponseErr;
		expect([err.error.code, err.error.reason]).toEqual(['bad_request', 'setup_invalid']);
		const v = err.error.detail as unknown as MimoValidation;
		expect(v.ok).toBe(false);
		expect(v.errors.map((e) => [e.reason, e.channel])).toEqual([['control_unknown_channel', 9]]);
	});

	it('the running status event carries per-control errors, gates and caps', () => {
		const event = readJson('event_mimo_random_status.json') as Event<MimoStatus>;
		expect(event.topic).toBe(MIMO_TOPICS.status);
		const s = event.data;
		expect([s.state, s.phase, s.level_db, s.loop]).toEqual(['running', 'engaged', -6, 14]);
		expect(s.control_error_db).toEqual([0.42, -0.31]);
		expect(s.gates?.passed).toBe(true);
		expect(s.caps).toEqual([1, 0.875]);
		expect(s.prediction?.drives_pass).toBe(true);
		expect(s.end).toBeNull();
	});

	it('the override request and the report', () => {
		const ov = readJson('request_mimo_random_override.json') as Request<MimoOverrideParams>;
		expect(ov.params.ack.trim().length).toBeGreaterThan(0);
		const rep = (readJson('response_mimo_random_report.json') as ResponseOk<MimoReportResult>).result.report;
		expect(rep?.end?.outcome).toBe('completed');
		expect(rep?.drives.map((d) => d.min_cap)).toEqual([0.875, 1]);
	});

	it('the law catalog says which world each law is for and whether it may drive hardware', () => {
		const list = (readJson('response_laws_list.json') as ResponseOk<LawsListResult>).result;
		const [pinv] = list.laws;
		expect([pinv.id, pinv.world, pinv.builtin, pinv.check.hardware]).toEqual(['pinv_closed', 'mimo', true, true]);
		expect(pinv.shapes).toContain('square');
		const choice = pinv.params.find((p) => p.key === 'regularization');
		expect(choice?.kind).toBe('choice');
		expect(list.problems).toHaveLength(1);
		const imported = (readJson('response_laws_import.json') as ResponseOk<LawsImportResult>).result.law;
		expect(imported.sha256).toBe(imported.id);
		expect([imported.check.status, imported.check.hardware]).toEqual(['unchecked', false]);
	});
});

type Expected = { dtype: Dtype; shape: number[]; values: number[] };
const isExpected = (v: unknown): v is Expected => typeof v === 'object' && v !== null && 'dtype' in v && 'values' in v;
function toTyped(node: unknown): unknown {
	if (isExpected(node)) {
		const ctor = { f32: Float32Array, f64: Float64Array, i32: Int32Array, u32: Uint32Array, u8: Uint8Array }[node.dtype];
		return withShape(ctor.from(node.values), node.shape);
	}
	if (Array.isArray(node)) return node.map(toTyped);
	if (node !== null && typeof node === 'object') return Object.fromEntries(Object.entries(node).map(([k, v]) => [k, toTyped(v)]));
	return node;
}
const readBin = (name: string) => new Uint8Array(readFileSync(golden(name)));

describe('mimo_random.data golden', () => {
	it('encodes byte for byte from its expected form', () => {
		const message = toTyped(readJson('event_mimo_random_data.expected.json')) as Record<string, unknown>;
		expect(Array.from(encodeFrame(message) as Uint8Array)).toEqual(Array.from(readBin('event_mimo_random_data.bin')));
	});

	it('decodes a packed matrix block, a coherence pair and a heatmap', () => {
		const event = decodeFrame(readBin('event_mimo_random_data.bin')) as unknown as Event<MimoData>;
		expect(event.topic).toBe(MIMO_TOPICS.data);
		const [grid, coh, heat] = event.data.plots;
		expect(grid.selector).toEqual({ block: [0, 1] });
		const sdm = grid.views[0] as SdmView;
		expect(sdm.view).toBe('sdm');
		expect(dtypeOf(sdm.measured!.re)).toBe('f64');
		expect(shapeOf(sdm.measured!.re)).toEqual([3, 3]);
		// G01 on line 0: 0.01171875 + 0.015625j; G10 is its conjugate.
		expect(packedElement(sdm.measured!.re, sdm.measured!.im, 2, 0, 0, 1)).toEqual([0.01171875, 0.015625]);
		expect(packedElement(sdm.measured!.re, sdm.measured!.im, 2, 0, 1, 0)).toEqual([0.01171875, -0.015625]);
		const gamma = packedCoherence(sdm.reference!.re, sdm.reference!.im, 2, 0, 0, 1);
		expect(gamma).toBeCloseTo((0.01171875 ** 2 + 0.015625 ** 2) / 0.0390625 ** 2, 12);
		const spec = coh.views[0] as SpectrumView;
		expect(spec.series.map((s) => s.role)).toEqual(['measured', 'reference']);
		expect(spec.series[0].y_min).toBeUndefined();
		const map = heat.views[0] as HeatmapView;
		expect(dtypeOf(map.values)).toBe('f32');
		expect(shapeOf(map.values)).toEqual([2, 4]);
		expect(map.row_bands).toHaveLength(2);
		expect(heat.labels).toEqual(['Control X', 'Control Y']);
	});

	it('packs the upper triangle row by row', () => {
		// n = 3: (0,0) (0,1) (0,2) (1,1) (1,2) (2,2)
		expect([packedIndex(3, 0, 0), packedIndex(3, 0, 2), packedIndex(3, 1, 1), packedIndex(3, 2, 1), packedIndex(3, 2, 2)]).toEqual([0, 2, 3, 4, 5]);
	});
});
