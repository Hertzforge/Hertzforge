/**
 * An in-process, WebSocket-shaped transport for mock mode and tests.
 *
 * A `MockServer` answers requests and pushes events the way a real service would. Every message
 * crosses through the real codec (`encodeFrame` on the way out, the client's `decodeFrame` on the
 * way in), so binary frames, alignment and array references are exercised exactly as on a socket.
 */

import { decodeFrame, encodeFrame } from '../codec';
import type { SocketFactory, SocketLike } from '../client.svelte';
import type { ErrorCode, Hello } from '../types';

/** Throw from a mock method to answer with a protocol error. */
export class MockError extends Error {
	constructor(
		readonly code: ErrorCode,
		message: string
	) {
		super(message);
	}
}

export type MockMethod = (params: Record<string, unknown>, conn: MockConnection) => unknown | Promise<unknown>;

export interface MockServer {
	hello: Hello;
	methods: Record<string, MockMethod>;
	/** Simulated processing time per method, ms. Defaults to a small constant. */
	latencyMs?: (method: string) => number;
	attach?(conn: MockConnection): void;
	detach?(conn: MockConnection): void;
}

/** The server-side end of one mock socket. */
export class MockConnection {
	private seq = 0;
	constructor(private readonly socket: MockSocket) {}

	emit(topic: string, data: Record<string, unknown>): void {
		this.socket.deliver(encodeFrame({ type: 'event', topic, seq: this.seq++, data }));
	}
}

const CONNECTING = 0;
const OPEN = 1;
const CLOSED = 3;

export class MockSocket implements SocketLike {
	binaryType = 'arraybuffer';
	readyState = CONNECTING;
	onopen: ((ev: unknown) => void) | null = null;
	onmessage: ((ev: { data: unknown }) => void) | null = null;
	onclose: ((ev: { code?: number; reason?: string }) => void) | null = null;
	onerror: ((ev: unknown) => void) | null = null;

	private readonly conn: MockConnection;

	constructor(
		readonly url: string,
		private readonly server: MockServer,
		private readonly connectDelayMs = 60
	) {
		this.conn = new MockConnection(this);
		setTimeout(() => {
			if (this.readyState !== CONNECTING) return;
			this.readyState = OPEN;
			this.onopen?.({});
			this.deliver(encodeFrame(server.hello));
			server.attach?.(this.conn);
		}, connectDelayMs);
	}

	send(data: string | ArrayBufferLike | ArrayBufferView): void {
		if (this.readyState !== OPEN) throw new Error('Mock socket is not open.');
		const message = decodeFrame(data as string | ArrayBuffer);
		if (message.type !== 'request') return;
		const { id, method, params } = message;
		const fn = this.server.methods[method];
		const delay = this.server.latencyMs?.(method) ?? 20;
		const respond = (frame: Record<string, unknown>) => setTimeout(() => this.deliver(encodeFrame(frame)), delay);
		if (!fn) {
			respond({
				type: 'response',
				id,
				error: { code: 'unknown_method', message: `The mock ${this.server.hello.service} service has no method "${method}".` }
			});
			return;
		}
		Promise.resolve()
			.then(() => fn(params ?? {}, this.conn))
			.then(
				(result) => respond({ type: 'response', id, result: (result ?? {}) as Record<string, unknown> }),
				(err: unknown) => {
					const code: ErrorCode = err instanceof MockError ? err.code : 'internal';
					respond({ type: 'response', id, error: { code, message: (err as Error).message || 'Mock method failed.' } });
				}
			);
	}

	close(): void {
		if (this.readyState === CLOSED) return;
		const wasOpen = this.readyState === OPEN;
		this.readyState = CLOSED;
		if (wasOpen) this.server.detach?.(this.conn);
		setTimeout(() => this.onclose?.({ code: 1000, reason: 'closed' }), 0);
	}

	/** Server to client. Binary frames arrive as an ArrayBuffer, as with `binaryType = "arraybuffer"`. */
	deliver(frame: string | Uint8Array): void {
		if (this.readyState !== OPEN) return;
		const data = typeof frame === 'string' ? frame : frame.buffer;
		this.onmessage?.({ data });
	}
}

/** A socket factory that connects every URL to `server`. */
export function mockSocketFactory(server: MockServer, connectDelayMs?: number): SocketFactory {
	return (url) => new MockSocket(url, server, connectDelayMs);
}
