/**
 * Mock analysis service: a catalog, three synthetic random runs, two synthetic plate models and
 * four operations. Every title says "Synthetic" so mock output is never mistaken for a result.
 */

import { withShape } from '../codec';
import type {
	Catalog,
	DatasetInfo,
	OpenedDataset,
	OperationDescriptor,
	ReadingRow,
	SpectrumView,
	View
} from '../types';
import { MockError, type MockServer } from './socket';
import { binEdges } from './bins';
import {
	breakpointArrays,
	channelPsd,
	frequencyLines,
	gauss,
	hashSeed,
	plate,
	QUAL_SPEC,
	rms,
	rng,
	RUN_CHANNELS,
	scaleSpec,
	sdofT2,
	specAt,
	specRms,
	WORKMANSHIP_SPEC,
	type Breakpoints,
	type PlateModel
} from './synth';

interface RunDataset extends DatasetInfo {
	kind: 'random_run';
	spec: Breakpoints;
	levelDb: number;
	durationS: number;
}

interface ModelDataset extends DatasetInfo {
	kind: 'modal_model';
	nx: number;
	ny: number;
}

const RUNS: RunDataset[] = [
	{
		id: 'syn-run-qual-z',
		kind: 'random_run',
		title: 'Synthetic Qualification Run, Z Axis',
		description: 'Generated in the shell. 20 to 2000 Hz at full level, four channels, 397 lines.',
		spec: QUAL_SPEC,
		levelDb: 0,
		durationS: 120
	},
	{
		id: 'syn-run-low-x',
		kind: 'random_run',
		title: 'Synthetic Low Level Survey, X Axis',
		description: 'Generated in the shell. Same profile at -12 dB, used to find resonances before full level.',
		spec: QUAL_SPEC,
		levelDb: -12,
		durationS: 60
	},
	{
		id: 'syn-run-wm',
		kind: 'random_run',
		title: 'Synthetic Workmanship Screen',
		description: 'Generated in the shell. Workmanship profile, 0.04 g²/Hz plateau to 500 Hz.',
		spec: WORKMANSHIP_SPEC,
		levelDb: 0,
		durationS: 60
	}
];

const MODELS: ModelDataset[] = [
	{
		id: 'syn-plate',
		kind: 'modal_model',
		title: 'Synthetic Plate, Simply Supported',
		description: 'Generated in the shell. 0.6 x 0.4 m, 3 mm aluminum, 60 x 40 elements, analytic modes.',
		nx: 60,
		ny: 40
	},
	{
		id: 'syn-plate-fine',
		kind: 'modal_model',
		title: 'Synthetic Plate, Fine Mesh',
		description: 'Generated in the shell. The same plate at 160 x 100 elements, for frame-rate checks.',
		nx: 160,
		ny: 100
	}
];

const OPERATIONS: OperationDescriptor[] = [
	{
		id: 'random.psd_vs_spec',
		title: 'PSD Against Specification',
		workspace: 'analyze',
		accepts: 'random_run',
		summary: 'Overlay a channel PSD on the test specification with tolerance and abort bands.',
		params: [
			{ name: 'channel', label: 'Channel', type: 'enum', options_from: 'channels', default: 'ctrl1' },
			{ name: 'tolerance_db', label: 'Tolerance', type: 'number', unit: 'dB', default: 3, min: 0.5, max: 6, step: 0.5 },
			{ name: 'abort_db', label: 'Abort Limit', type: 'number', unit: 'dB', default: 6, min: 1, max: 12, step: 0.5 },
			{ name: 'label', label: 'Trace Label', type: 'string', default: 'Control' }
		],
		outputs: ['spectrum', 'reading']
	},
	{
		id: 'random.transmissibility',
		title: 'Transmissibility',
		workspace: 'analyze',
		accepts: 'random_run',
		summary: 'Ratio of a response channel to a reference channel, with the peaks it finds.',
		params: [
			{ name: 'response', label: 'Response', type: 'enum', options_from: 'channels', default: 'resp1' },
			{ name: 'reference', label: 'Reference', type: 'enum', options_from: 'channels', default: 'ctrl1' },
			{ name: 'show_unity', label: 'Unity Line', type: 'boolean', default: true }
		],
		outputs: ['spectrum', 'reading']
	},
	{
		id: 'random.coherence',
		title: 'Coherence',
		workspace: 'analyze',
		accepts: 'random_run',
		summary: 'Ordinary coherence between two channels, against a quality threshold.',
		params: [
			{ name: 'response', label: 'Response', type: 'enum', options_from: 'channels', default: 'resp2' },
			{ name: 'reference', label: 'Reference', type: 'enum', options_from: 'channels', default: 'ctrl1' },
			{ name: 'averages', label: 'Averages', type: 'integer', default: 64, min: 8, max: 512, step: 8 },
			{ name: 'threshold', label: 'Threshold', type: 'number', default: 0.9, min: 0, max: 1, step: 0.05 }
		],
		outputs: ['spectrum', 'reading']
	},
	{
		id: 'modal.modes',
		title: 'Mode Shapes',
		workspace: 'modal',
		accepts: 'modal_model',
		summary: 'Natural frequencies, damping and mode shapes of the model.',
		params: [{ name: 'count', label: 'Modes', type: 'integer', default: 8, min: 1, max: 12 }],
		outputs: ['shape3d']
	}
];

interface Handle {
	dataset: RunDataset | ModelDataset;
	plateCache?: Map<number, PlateModel>;
}

function fmt(value: number, digits = 3): number {
	return Number(value.toPrecision(digits));
}

function db(ratio: number): number {
	return 10 * Math.log10(ratio);
}

function frequencyAxis() {
	return { label: 'Frequency', unit: 'Hz', scale: 'log' as const };
}

function requireNumber(params: Record<string, unknown>, name: string, min: number, max: number): number {
	const v = params[name];
	if (typeof v !== 'number' || !Number.isFinite(v)) throw new MockError('bad_request', `${name} must be a number.`);
	if (v < min || v > max) throw new MockError('bad_request', `${name} must be between ${min} and ${max}; got ${v}.`);
	return v;
}

function requireChannel(params: Record<string, unknown>, name: string) {
	const ch = RUN_CHANNELS.find((c) => c.value === params[name]);
	if (!ch) throw new MockError('bad_request', `${name} must name one of this run's channels.`);
	return ch;
}

export function createAnalysisServer(): MockServer {
	const handles = new Map<string, Handle>();
	let handleCount = 0;

	function handleFor(params: Record<string, unknown>): Handle {
		const h = handles.get(String(params.handle));
		if (!h) throw new MockError('not_found', 'That dataset handle is not open; open the dataset again.');
		return h;
	}

	function psdVsSpec(ds: RunDataset, p: Record<string, unknown>): View[] {
		const tol = requireNumber(p, 'tolerance_db', 0.5, 6);
		const abort = requireNumber(p, 'abort_db', 1, 12);
		if (abort <= tol) throw new MockError('bad_request', 'The abort limit must be wider than the tolerance band.');
		const ch = requireChannel(p, 'channel');
		const label = typeof p.label === 'string' && p.label.trim() ? p.label.trim() : ch.label;
		const spec = scaleSpec(ds.spec, ds.levelDb);
		const f = frequencyLines();
		const psd = channelPsd(f, spec, ch, hashSeed(ds.id, ch.value));

		let inTol = 0;
		let worst = 0;
		let worstF = f[0];
		for (let i = 0; i < f.length; i++) {
			const dev = db(psd[i] / specAt(spec, f[i]));
			if (Math.abs(dev) <= tol) inTol++;
			if (Math.abs(dev) > Math.abs(worst)) {
				worst = dev;
				worstF = f[i];
			}
		}
		const sp = breakpointArrays(spec);
		const band = (d: number) => breakpointArrays(scaleSpec(spec, d));
		const isResponse = ch.modes.length > 0;

		const spectrum: SpectrumView = {
			view: 'spectrum',
			title: `PSD Against Specification, ${ch.label}`,
			x: frequencyAxis(),
			y: { label: 'PSD', unit: 'g²/Hz', scale: 'log' },
			series: [
				{ name: label, role: 'measured', x: f, y: psd, shape: 'steps', edges: binEdges(f) },
				{ name: 'Specification', role: 'reference', ...sp },
				{ name: `+${tol} dB Tolerance`, role: 'tolerance', ...band(tol) },
				{ name: `-${tol} dB Tolerance`, role: 'tolerance', ...band(-tol) },
				{ name: `+${abort} dB Abort`, role: 'abort', ...band(abort) },
				{ name: `-${abort} dB Abort`, role: 'abort', ...band(-abort) }
			],
			link: 'frequency'
		};
		const total = f.length;
		const rows: ReadingRow[] = [
			{ label: `${ch.label} rms`, value: fmt(rms(f, psd)), unit: 'g', status: isResponse ? 'info' : inTol === total ? 'pass' : 'warn' },
			{ label: 'Specification rms', value: fmt(specRms(spec)), unit: 'g', status: 'info' },
			{
				label: 'Lines in tolerance',
				value: inTol === total ? 'all' : `${inTol} of ${total}`,
				unit: null,
				status: inTol === total ? 'pass' : Math.abs(worst) > abort ? 'fail' : 'warn'
			},
			{
				label: 'Largest deviation',
				value: `${worst >= 0 ? '+' : ''}${worst.toFixed(1)} dB at ${worstF} Hz`,
				unit: null,
				status: Math.abs(worst) <= tol ? 'pass' : Math.abs(worst) <= abort ? 'warn' : 'fail'
			}
		];
		return [spectrum, { view: 'reading', title: 'Summary', rows }];
	}

	function ratioSpectra(ds: RunDataset, p: Record<string, unknown>) {
		const resp = requireChannel(p, 'response');
		const ref = requireChannel(p, 'reference');
		if (resp.value === ref.value) throw new MockError('bad_request', 'Pick two different channels for response and reference.');
		return { resp, ref, f: frequencyLines(), spec: scaleSpec(ds.spec, ds.levelDb) };
	}

	function transmissibility(ds: RunDataset, p: Record<string, unknown>): View[] {
		const { resp, ref, f } = ratioSpectra(ds, p);
		const next = rng(hashSeed(ds.id, resp.value, ref.value, 't'));
		const t = new Float64Array(f.length);
		for (let i = 0; i < f.length; i++) {
			let r2 = 1;
			for (const m of resp.modes) r2 *= sdofT2(f[i], m.fn, m.q);
			for (const m of ref.modes) r2 /= sdofT2(f[i], m.fn, m.q);
			t[i] = Math.sqrt(r2) * 10 ** ((0.25 * gauss(next)) / 20);
		}
		// The two largest local maxima, at least an octave apart.
		const peaks: { f: number; v: number }[] = [];
		for (let i = 1; i < f.length - 1; i++) if (t[i] > t[i - 1] && t[i] >= t[i + 1]) peaks.push({ f: f[i], v: t[i] });
		peaks.sort((a, b) => b.v - a.v);
		const picked: typeof peaks = [];
		for (const pk of peaks) {
			if (picked.every((q) => Math.abs(Math.log2(pk.f / q.f)) >= 1)) picked.push(pk);
			if (picked.length === 2) break;
		}
		picked.sort((a, b) => a.f - b.f);

		const series: SpectrumView['series'] = [{ name: `${resp.label} / ${ref.label}`, role: 'measured', x: f, y: t }];
		if (p.show_unity !== false) series.push({ name: 'Unity', role: 'reference', x: Float64Array.of(20, 2000), y: Float64Array.of(1, 1) });
		const rows: ReadingRow[] = picked.map((pk, i) => ({
			label: `Peak ${i + 1}`,
			value: `${fmt(pk.v)} at ${pk.f} Hz`,
			unit: null,
			status: pk.v > 15 ? 'warn' : 'info'
		}));
		rows.push({ label: 'Band', value: '20 to 2000 Hz', unit: null, status: 'info' });
		return [
			{
				view: 'spectrum',
				title: `Transmissibility, ${resp.label}`,
				x: frequencyAxis(),
				y: { label: 'Transmissibility', unit: 'g/g', scale: 'log' },
				series,
				link: 'frequency'
			},
			{ view: 'reading', title: 'Peaks', rows }
		];
	}

	function coherence(ds: RunDataset, p: Record<string, unknown>): View[] {
		const { resp, ref, f } = ratioSpectra(ds, p);
		const averages = requireNumber(p, 'averages', 8, 512);
		if (!Number.isInteger(averages)) throw new MockError('bad_request', 'averages must be a whole number.');
		const threshold = requireNumber(p, 'threshold', 0, 1);
		const next = rng(hashSeed(ds.id, resp.value, ref.value, averages, 'c'));
		const modes = [...resp.modes, ...ref.modes];
		const c = new Float64Array(f.length);
		let above = 0;
		let min = 1;
		let minF = f[0];
		for (let i = 0; i < f.length; i++) {
			// Coherence sags between modes (low response, noise floor dominates) and at the band edge.
			let resp2 = 0;
			for (const m of modes) resp2 += sdofT2(f[i], m.fn, m.q);
			const snr = 40 * resp2 * Math.min(1, f[i] / 60);
			const ideal = snr / (1 + snr);
			const scatter = Math.abs(gauss(next)) * (1 - ideal + 0.02) * (6 / Math.sqrt(averages));
			c[i] = Math.max(0, Math.min(1, ideal - scatter));
			if (c[i] >= threshold) above++;
			if (c[i] < min) {
				min = c[i];
				minF = f[i];
			}
		}
		return [
			{
				view: 'spectrum',
				title: `Coherence, ${resp.label}`,
				x: frequencyAxis(),
				y: { label: 'Coherence', unit: null, scale: 'linear' },
				series: [
					{ name: `${resp.label} with ${ref.label}`, role: 'measured', x: f, y: c },
					{ name: `Threshold ${threshold}`, role: 'tolerance', x: Float64Array.of(20, 2000), y: Float64Array.of(threshold, threshold) }
				],
				link: 'frequency'
			},
			{
				view: 'reading',
				title: 'Quality',
				rows: [
					{
						label: 'Lines above threshold',
						value: `${above} of ${f.length}`,
						unit: null,
						status: above / f.length > 0.9 ? 'pass' : above / f.length > 0.6 ? 'warn' : 'fail'
					},
					{ label: 'Minimum', value: `${min.toFixed(2)} at ${minF} Hz`, unit: null, status: min >= threshold ? 'pass' : 'warn' },
					{ label: 'Averages', value: averages, unit: null, status: 'info' }
				]
			}
		];
	}

	function modes(h: Handle, p: Record<string, unknown>): View[] {
		const ds = h.dataset as ModelDataset;
		const count = requireNumber(p, 'count', 1, 12);
		h.plateCache ??= new Map();
		let model = h.plateCache.get(count);
		if (!model) {
			model = plate(0.6, 0.4, ds.nx, ds.ny, count, hashSeed(ds.id));
			h.plateCache.set(count, model);
		}
		const n = model.nodes.length / 3;
		// Fresh arrays per response so shapes can be attached without touching the cache.
		return [
			{
				view: 'shape3d',
				title: ds.title,
				unit: 'm',
				nodes: withShape(model.nodes.slice(), [n, 3]),
				lines: withShape(model.lines.slice(), [model.lines.length / 2, 2]),
				triangles: withShape(model.triangles.slice(), [model.triangles.length / 3, 3]),
				modes: model.modes.map((m) => ({ ...m, shape: withShape(m.shape.slice(), [n, 3]) }))
			}
		];
	}

	return {
		hello: { type: 'hello', protocol: 'sds/1', service: 'analysis', version: '0.1.0-mock' },
		latencyMs: (method) => (method === 'op.run' ? 140 : 30),
		methods: {
			'catalog.list': (): Catalog => ({
				operations: OPERATIONS,
				datasets: [...RUNS, ...MODELS].map(({ id, kind, title, description }) => ({ id, kind, title, description }))
			}),
			'dataset.open': (params): OpenedDataset => {
				const ds = [...RUNS, ...MODELS].find((d) => d.id === params.id);
				if (!ds) throw new MockError('not_found', `No dataset named "${String(params.id)}"; catalog.list names the ones that exist.`);
				const handle = `h${++handleCount}`;
				handles.set(handle, { dataset: ds });
				if (ds.kind === 'random_run') {
					return {
						handle,
						kind: ds.kind,
						title: ds.title,
						summary: {
							duration_s: ds.durationS,
							level_db: ds.levelDb,
							band_hz: '20 to 2000',
							lines: frequencyLines().length,
							spec_rms_g: fmt(specRms(scaleSpec(ds.spec, ds.levelDb)))
						},
						channels: RUN_CHANNELS.map(({ value, label }) => ({ value, label }))
					};
				}
				return {
					handle,
					kind: ds.kind,
					title: ds.title,
					summary: { nodes: (ds.nx + 1) * (ds.ny + 1), triangles: ds.nx * ds.ny * 2, size_m: '0.6 x 0.4' },
					channels: []
				};
			},
			'op.run': (params) => {
				const h = handleFor(params);
				const op = OPERATIONS.find((o) => o.id === params.op);
				if (!op) throw new MockError('not_found', `No operation named "${String(params.op)}".`);
				if (op.accepts !== h.dataset.kind) {
					throw new MockError('bad_request', `${op.title} works on ${op.accepts} datasets; this one is ${h.dataset.kind}.`);
				}
				const p = (params.params ?? {}) as Record<string, unknown>;
				const ds = h.dataset as RunDataset;
				switch (op.id) {
					case 'random.psd_vs_spec':
						return { views: psdVsSpec(ds, p) };
					case 'random.transmissibility':
						return { views: transmissibility(ds, p) };
					case 'random.coherence':
						return { views: coherence(ds, p) };
					case 'modal.modes':
						return { views: modes(h, p) };
				}
				throw new MockError('internal', `${op.id} has no mock implementation.`);
			}
		}
	};
}
