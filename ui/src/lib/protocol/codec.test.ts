import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { FrameError, decodeFrame, dtypeOf, encodeFrame, shapeOf, withShape } from './codec';
import type { Dtype, NumericArray } from './types';

const golden = (name: string) => fileURLToPath(new URL(`../../../../schema/golden/${name}`, import.meta.url));
const readBin = (name: string) => new Uint8Array(readFileSync(golden(name)));
const readJson = (name: string) => JSON.parse(readFileSync(golden(name), 'utf-8'));

type Expected = { dtype: Dtype; shape: number[]; values: number[] };

function isExpectedArray(v: unknown): v is Expected {
	return typeof v === 'object' && v !== null && 'dtype' in v && 'shape' in v && 'values' in v;
}

/** Compare a decoded message against its `.expected.json` form, arrays included. */
function expectMatches(actual: unknown, expected: unknown, path = '$'): void {
	if (isExpectedArray(expected)) {
		expect(ArrayBuffer.isView(actual), `${path} should be a typed array`).toBe(true);
		const arr = actual as NumericArray;
		expect(dtypeOf(arr), `${path} dtype`).toBe(expected.dtype);
		expect(shapeOf(arr), `${path} shape`).toEqual(expected.shape);
		expect(Array.from(arr).length, `${path} length`).toBe(expected.values.length);
		if (expected.dtype === 'f32') {
			expected.values.forEach((v, i) => expect(arr[i]).toBeCloseTo(v, 6));
		} else {
			expect(Array.from(arr)).toEqual(expected.values);
		}
		return;
	}
	if (Array.isArray(expected)) {
		expect(Array.isArray(actual), `${path} should be a list`).toBe(true);
		expect((actual as unknown[]).length, `${path} length`).toBe(expected.length);
		expected.forEach((item, i) => expectMatches((actual as unknown[])[i], item, `${path}[${i}]`));
		return;
	}
	if (expected !== null && typeof expected === 'object') {
		expect(Object.keys(actual as object).sort(), `${path} keys`).toEqual(Object.keys(expected).sort());
		for (const [k, v] of Object.entries(expected)) expectMatches((actual as Record<string, unknown>)[k], v, `${path}.${k}`);
		return;
	}
	expect(actual, path).toEqual(expected);
}

/** Turn an expected-form message into one carrying typed arrays (with shapes), for encoding. */
function toTyped(node: unknown): unknown {
	if (isExpectedArray(node)) {
		const ctor = { f32: Float32Array, f64: Float64Array, i32: Int32Array, u32: Uint32Array, u8: Uint8Array }[node.dtype];
		return withShape(ctor.from(node.values), node.shape);
	}
	if (Array.isArray(node)) return node.map(toTyped);
	if (node !== null && typeof node === 'object') {
		return Object.fromEntries(Object.entries(node).map(([k, v]) => [k, toTyped(v)]));
	}
	return node;
}

describe('decodeFrame against golden frames', () => {
	for (const name of ['response_spectrum', 'event_shape']) {
		it(`decodes ${name}.bin to ${name}.expected.json`, () => {
			const decoded = decodeFrame(readBin(`${name}.bin`));
			expect('arrays' in decoded).toBe(false);
			expectMatches(decoded, readJson(`${name}.expected.json`));
		});
	}

	it('decodes from an ArrayBuffer and from an unaligned view', () => {
		const bytes = readBin('response_spectrum.bin');
		expectMatches(decodeFrame(bytes.slice().buffer), readJson('response_spectrum.expected.json'));
		const shifted = new Uint8Array(bytes.byteLength + 3);
		shifted.set(bytes, 3);
		expectMatches(decodeFrame(shifted.subarray(3)), readJson('response_spectrum.expected.json'));
	});

	it('decodes the text frames', () => {
		const hello = readFileSync(golden('hello.json'), 'utf-8');
		expect(decodeFrame(hello)).toEqual({ type: 'hello', protocol: 'sds/1', service: 'analysis', version: '0.1.0' });
		const err = decodeFrame(readFileSync(golden('response_error.json'), 'utf-8'));
		expect(err).toMatchObject({ type: 'response', id: 8, error: { code: 'not_found' } });
	});
});

describe('encodeFrame', () => {
	it('reproduces response_spectrum.bin byte for byte', () => {
		const message = toTyped(readJson('response_spectrum.expected.json')) as Record<string, unknown>;
		const bytes = encodeFrame(message);
		expect(bytes).toBeInstanceOf(Uint8Array);
		expect(Array.from(bytes as Uint8Array)).toEqual(Array.from(readBin('response_spectrum.bin')));
	});

	it('round-trips event_shape through encode and decode', () => {
		const expected = readJson('event_shape.expected.json');
		const bytes = encodeFrame(toTyped(expected) as Record<string, unknown>) as Uint8Array;
		expect((8 + new DataView(bytes.buffer).getUint32(4, true)) % 8).toBe(0);
		expectMatches(decodeFrame(bytes), expected);
	});

	it('writes a text frame when the message carries no arrays', () => {
		const frame = encodeFrame({ type: 'request', id: 3, method: 'catalog.list', params: {} });
		expect(frame).toBe('{"type":"request","id":3,"method":"catalog.list","params":{}}');
	});
});

describe('decodeFrame rejects frames that break the contract', () => {
	const good = () => readBin('response_spectrum.bin').slice();

	it('bad magic', () => {
		const b = good();
		b[0] = 0x58;
		expect(() => decodeFrame(b)).toThrow(FrameError);
	});

	it('truncated payload', () => {
		expect(() => decodeFrame(good().subarray(0, 300))).toThrow(/past the end/);
	});

	it('unknown message type in a text frame', () => {
		expect(() => decodeFrame('{"type":"ping"}')).toThrow(/Unknown message type/);
	});

	it('a reference the header does not list', () => {
		const frame = encodeFrame({ type: 'event', topic: 't', seq: 0, data: { a: new Float64Array([1]) } }) as Uint8Array;
		const text = new TextDecoder().decode(frame);
		const broken = new TextEncoder().encode(text.replace('{"$array":"a0"}', '{"$array":"zz"}'));
		expect(() => decodeFrame(broken)).toThrow(FrameError);
	});
});
