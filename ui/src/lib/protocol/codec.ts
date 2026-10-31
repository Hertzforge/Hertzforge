/**
 * Frame codec for protocol `sds/1`.
 *
 * Text frame: one JSON message with no arrays. Binary frame (little-endian):
 *
 *   0      4   ASCII magic "SDS1"
 *   4      4   u32 header length H
 *   8      H   UTF-8 JSON header, space-padded so 8 + H is a multiple of 8
 *   8 + H  ..  payload: arrays, each starting at a multiple of 8 from the payload start
 *
 * Decoding replaces every `{"$array": id}` with a typed array and drops the `arrays` list.
 * Encoding does the reverse for any typed array found in the message. Shapes are not stored on
 * the typed arrays themselves; `shapeOf` and `withShape` carry them through a WeakMap.
 */

import type { ArrayEntry, Dtype, Message, NumericArray } from './types';

const MAGIC = [0x53, 0x44, 0x53, 0x31]; // "SDS1"

// Typed arrays use host byte order. Every platform the shell targets is little-endian; refuse to
// run elsewhere rather than silently byte-swap the data.
if (new Uint8Array(new Uint16Array([1]).buffer)[0] !== 1) {
	throw new Error('sds/1 codec requires a little-endian host.');
}

type Ctor =
	| Float32ArrayConstructor
	| Float64ArrayConstructor
	| Int32ArrayConstructor
	| Uint32ArrayConstructor
	| Uint8ArrayConstructor;

const CTORS: Record<Dtype, Ctor> = {
	f32: Float32Array,
	f64: Float64Array,
	i32: Int32Array,
	u32: Uint32Array,
	u8: Uint8Array
};

/** Raised for any frame that does not follow the contract; the message says what is wrong. */
export class FrameError extends Error {
	constructor(message: string) {
		super(message);
		this.name = 'FrameError';
	}
}

// ---------------------------------------------------------------------------------------------
// Shapes

const shapes = new WeakMap<NumericArray, number[]>();

/** The shape a decoded array arrived with, or `[length]` if none was recorded. */
export function shapeOf(array: NumericArray): number[] {
	return shapes.get(array) ?? [array.length];
}

/** Record the shape an array should be encoded with. Returns the same array. */
export function withShape<T extends NumericArray>(array: T, shape: number[]): T {
	const count = shape.reduce((a, b) => a * b, 1);
	if (count !== array.length) {
		throw new FrameError(`Shape [${shape.join(', ')}] holds ${count} elements; the array has ${array.length}.`);
	}
	shapes.set(array, shape.slice());
	return array;
}

export function dtypeOf(array: NumericArray): Dtype {
	if (array instanceof Float32Array) return 'f32';
	if (array instanceof Float64Array) return 'f64';
	if (array instanceof Int32Array) return 'i32';
	if (array instanceof Uint32Array) return 'u32';
	if (array instanceof Uint8Array) return 'u8';
	throw new FrameError('Unsupported typed array; sds/1 carries f32, f64, i32, u32 and u8 only.');
}

function isNumericArray(value: unknown): value is NumericArray {
	return (
		value instanceof Float32Array ||
		value instanceof Float64Array ||
		value instanceof Int32Array ||
		value instanceof Uint32Array ||
		value instanceof Uint8Array
	);
}

// ---------------------------------------------------------------------------------------------
// Decode

const utf8Decoder = new TextDecoder('utf-8', { fatal: true });
const utf8Encoder = new TextEncoder();

/** Decode one WebSocket frame: a string (text frame) or bytes (binary frame). */
export function decodeFrame(frame: string | ArrayBuffer | ArrayBufferView): Message {
	if (typeof frame === 'string') return checkMessage(parseJson(frame));
	const bytes =
		frame instanceof ArrayBuffer
			? new Uint8Array(frame)
			: new Uint8Array(frame.buffer, frame.byteOffset, frame.byteLength);
	return decodeBinary(bytes);
}

function parseJson(text: string): unknown {
	try {
		return JSON.parse(text);
	} catch (err) {
		throw new FrameError(`Frame is not valid JSON: ${(err as Error).message}`);
	}
}

function checkMessage(value: unknown): Message {
	if (typeof value !== 'object' || value === null || Array.isArray(value)) {
		throw new FrameError('Frame must hold a JSON object.');
	}
	const type = (value as { type?: unknown }).type;
	if (type !== 'hello' && type !== 'request' && type !== 'response' && type !== 'event') {
		throw new FrameError(`Unknown message type ${JSON.stringify(type)}.`);
	}
	return value as Message;
}

function decodeBinary(bytes: Uint8Array): Message {
	if (bytes.byteLength < 8) throw new FrameError('Binary frame is shorter than its 8-byte prefix.');
	for (let i = 0; i < 4; i++) {
		if (bytes[i] !== MAGIC[i]) throw new FrameError('Binary frame does not start with the magic "SDS1".');
	}
	const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
	const headerLength = view.getUint32(4, true);
	const payloadStart = 8 + headerLength;
	if (payloadStart % 8 !== 0) throw new FrameError(`Header length ${headerLength} leaves the payload unaligned.`);
	if (payloadStart > bytes.byteLength) throw new FrameError('Header length runs past the end of the frame.');

	let header: unknown;
	try {
		header = JSON.parse(utf8Decoder.decode(bytes.subarray(8, payloadStart)));
	} catch (err) {
		throw new FrameError(`Binary frame header is not valid UTF-8 JSON: ${(err as Error).message}`);
	}
	const message = checkMessage(header) as Message & { arrays?: ArrayEntry[] };
	const entries = message.arrays ?? [];
	if (!Array.isArray(entries)) throw new FrameError('Header "arrays" must be a list.');
	delete message.arrays;

	const payloadLength = bytes.byteLength - payloadStart;
	const table = new Map<string, NumericArray>();
	for (const entry of entries) {
		const array = readArray(bytes, payloadStart, payloadLength, entry);
		if (table.has(entry.id)) throw new FrameError(`Array id "${entry.id}" is listed twice.`);
		table.set(entry.id, array);
	}

	const used = new Set<string>();
	const resolved = resolveRefs(message, table, used) as Message;
	for (const id of table.keys()) {
		if (!used.has(id)) throw new FrameError(`Array "${id}" is listed but never referenced.`);
	}
	return resolved;
}

function readArray(bytes: Uint8Array, payloadStart: number, payloadLength: number, entry: ArrayEntry): NumericArray {
	const ctor = CTORS[entry.dtype];
	if (!ctor) throw new FrameError(`Array "${entry.id}" has unknown dtype ${JSON.stringify(entry.dtype)}.`);
	if (!Array.isArray(entry.shape) || entry.shape.some((d) => !Number.isInteger(d) || d < 0)) {
		throw new FrameError(`Array "${entry.id}" has an invalid shape.`);
	}
	const count = entry.shape.reduce((a, b) => a * b, 1);
	const nbytes = count * ctor.BYTES_PER_ELEMENT;
	if (entry.nbytes !== nbytes) {
		throw new FrameError(`Array "${entry.id}" declares ${entry.nbytes} bytes; shape and dtype give ${nbytes}.`);
	}
	if (entry.offset % 8 !== 0) throw new FrameError(`Array "${entry.id}" offset ${entry.offset} is not a multiple of 8.`);
	if (entry.offset + nbytes > payloadLength) throw new FrameError(`Array "${entry.id}" runs past the end of the payload.`);

	const start = bytes.byteOffset + payloadStart + entry.offset;
	let array: NumericArray;
	if (start % ctor.BYTES_PER_ELEMENT === 0) {
		// Zero-copy view; the frame's buffer stays alive as long as the array does.
		array = new ctor(bytes.buffer as ArrayBuffer, start, count);
	} else {
		// The caller handed us a view at an unaligned offset (a pooled buffer, for example): copy.
		const copy = bytes.slice(payloadStart + entry.offset, payloadStart + entry.offset + nbytes);
		array = new ctor(copy.buffer, 0, count);
	}
	shapes.set(array, entry.shape.slice());
	return array;
}

function resolveRefs(node: unknown, table: Map<string, NumericArray>, used: Set<string>): unknown {
	if (Array.isArray(node)) return node.map((item) => resolveRefs(item, table, used));
	if (node === null || typeof node !== 'object') return node;
	const obj = node as Record<string, unknown>;
	const keys = Object.keys(obj);
	if (keys.length === 1 && keys[0] === '$array') {
		const id = obj.$array as string;
		const array = table.get(id);
		if (!array) throw new FrameError(`Reference to array "${id}", which the header does not list.`);
		if (used.has(id)) throw new FrameError(`Array "${id}" is referenced more than once.`);
		used.add(id);
		return array;
	}
	for (const key of keys) obj[key] = resolveRefs(obj[key], table, used);
	return obj;
}

// ---------------------------------------------------------------------------------------------
// Encode

/**
 * Encode a message. Without typed arrays the result is a text frame (string); with any, a binary
 * frame (`Uint8Array` that owns its buffer exactly). Array ids are `a0`, `a1`, ... in the order
 * they are met walking the message, which is also their payload order.
 */
export function encodeFrame(message: Message | Record<string, unknown>): string | Uint8Array {
	const found: NumericArray[] = [];
	const header = replaceArrays(message, found);
	if (found.length === 0) return JSON.stringify(header);

	const entries: ArrayEntry[] = [];
	let offset = 0;
	for (let i = 0; i < found.length; i++) {
		const array = found[i];
		offset = align8(offset);
		entries.push({ id: `a${i}`, dtype: dtypeOf(array), shape: shapeOf(array), offset, nbytes: array.byteLength });
		offset += array.byteLength;
	}
	const payloadLength = offset;

	let headerBytes = utf8Encoder.encode(JSON.stringify({ ...(header as object), arrays: entries }));
	const padding = (8 - ((8 + headerBytes.byteLength) % 8)) % 8;
	if (padding) {
		const padded = new Uint8Array(headerBytes.byteLength + padding).fill(0x20);
		padded.set(headerBytes);
		headerBytes = padded;
	}

	const out = new Uint8Array(8 + headerBytes.byteLength + payloadLength);
	out.set(MAGIC, 0);
	new DataView(out.buffer).setUint32(4, headerBytes.byteLength, true);
	out.set(headerBytes, 8);
	const payloadStart = 8 + headerBytes.byteLength;
	for (let i = 0; i < found.length; i++) {
		const array = found[i];
		out.set(new Uint8Array(array.buffer, array.byteOffset, array.byteLength), payloadStart + entries[i].offset);
	}
	return out;
}

function align8(n: number): number {
	return (n + 7) & ~7;
}

function replaceArrays(node: unknown, found: NumericArray[]): unknown {
	if (isNumericArray(node)) {
		found.push(node);
		return { $array: `a${found.length - 1}` };
	}
	if (ArrayBuffer.isView(node)) throw new FrameError('Unsupported typed array in message.');
	if (Array.isArray(node)) return node.map((item) => replaceArrays(item, found));
	if (node === null || typeof node !== 'object') return node;
	const out: Record<string, unknown> = {};
	for (const [key, value] of Object.entries(node as Record<string, unknown>)) {
		if (value !== undefined) out[key] = replaceArrays(value, found);
	}
	return out;
}
