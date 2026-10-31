/**
 * One client per service: connect, check `hello`, request/response by id with a timeout, event
 * subscription, and reconnect with backoff. Connection state is reactive (Svelte runes) so the
 * shell can render it directly.
 *
 * The transport is anything WebSocket-shaped (`SocketLike`). The browser's WebSocket is the
 * default; the mock services implement the same shape in-process, so mock mode exercises this
 * exact client and the binary codec end to end.
 */

import { decodeFrame, encodeFrame, FrameError } from './codec';
import { PROTOCOL, type Event, type Hello, type Message, type ServiceName } from './types';

export interface SocketLike {
	binaryType: string;
	readonly readyState: number;
	send(data: string | ArrayBufferLike | ArrayBufferView): void;
	close(code?: number, reason?: string): void;
	onopen: ((ev: unknown) => void) | null;
	onmessage: ((ev: { data: unknown }) => void) | null;
	onclose: ((ev: { code?: number; reason?: string }) => void) | null;
	onerror: ((ev: unknown) => void) | null;
}

export type SocketFactory = (url: string) => SocketLike;

export const browserSocket: SocketFactory = (url) => new WebSocket(url) as unknown as SocketLike;

export type ConnectionStatus =
	/** Not started, or closed on purpose. */
	| 'offline'
	/** First attempt in progress. */
	| 'connecting'
	/** Hello received and the protocol matches. */
	| 'open'
	/** Lost or failed; waiting out the backoff before the next attempt. */
	| 'retrying'
	/** The service speaks a different protocol version. Retries slowly in case it is upgraded. */
	| 'incompatible';

export type ClientErrorCode =
	| 'bad_request'
	| 'unknown_method'
	| 'not_found'
	| 'refused'
	| 'internal'
	| 'timeout'
	| 'disconnected'
	| 'offline';

/** A failed request. `code` is a protocol error code or one of the client's own. */
export class RequestError extends Error {
	constructor(
		readonly code: ClientErrorCode,
		message: string,
		readonly method: string
	) {
		super(message);
		this.name = 'RequestError';
	}
}

export interface ClientOptions {
	service: ServiceName;
	url: string;
	socketFactory?: SocketFactory;
	/** Per-request default. */
	requestTimeoutMs?: number;
	/** Time allowed between socket open and the server's hello. */
	helloTimeoutMs?: number;
	backoffInitialMs?: number;
	backoffMaxMs?: number;
}

type EventHandler = (data: Record<string, unknown>, event: Event) => void;

interface Pending {
	method: string;
	resolve: (result: unknown) => void;
	reject: (err: RequestError) => void;
	timer: ReturnType<typeof setTimeout>;
}

export class ServiceClient {
	readonly service: ServiceName;
	readonly url: string;

	status = $state<ConnectionStatus>('offline');
	/** The server's hello once the protocol matched. */
	hello = $state<Hello | null>(null);
	/** Human-readable reason for the current non-open status, if there is one. */
	detail = $state<string | null>(null);
	/** Milliseconds until the next attempt while `retrying` or `incompatible`. */
	retryInMs = $state(0);
	attempts = $state(0);

	private readonly factory: SocketFactory;
	private readonly requestTimeoutMs: number;
	private readonly helloTimeoutMs: number;
	private readonly backoffInitialMs: number;
	private readonly backoffMaxMs: number;

	private socket: SocketLike | null = null;
	private nextId = 1;
	private pending = new Map<number, Pending>();
	private handlers = new Map<string, Set<EventHandler>>();
	private openWaiters = new Set<() => void>();
	private retryTimer: ReturnType<typeof setTimeout> | null = null;
	private helloTimer: ReturnType<typeof setTimeout> | null = null;
	private countdown: ReturnType<typeof setInterval> | null = null;
	private backoffMs: number;
	private wanted = false;
	private everOpened = false;

	constructor(options: ClientOptions) {
		this.service = options.service;
		this.url = options.url;
		this.factory = options.socketFactory ?? browserSocket;
		this.requestTimeoutMs = options.requestTimeoutMs ?? 10_000;
		this.helloTimeoutMs = options.helloTimeoutMs ?? 3_000;
		this.backoffInitialMs = options.backoffInitialMs ?? 500;
		this.backoffMaxMs = options.backoffMaxMs ?? 8_000;
		this.backoffMs = this.backoffInitialMs;
	}

	get isOpen(): boolean {
		return this.status === 'open';
	}

	/** Start connecting; keeps reconnecting until `close()`. Idempotent. */
	connect(): void {
		this.wanted = true;
		if (this.socket || this.retryTimer) return;
		this.attempt();
	}

	/** Skip the backoff wait and try again now. */
	retryNow(): void {
		if (!this.wanted) return this.connect();
		if (this.socket) return;
		this.clearRetry();
		this.attempt();
	}

	/** Close on purpose. Pending requests fail with `disconnected`; no reconnect. */
	close(): void {
		this.wanted = false;
		this.clearRetry();
		this.teardown('Closed by the shell.');
		this.status = 'offline';
		this.detail = null;
	}

	/** Resolves when the connection is open (immediately if it already is). */
	whenOpen(): Promise<void> {
		if (this.isOpen) return Promise.resolve();
		return new Promise((resolve) => this.openWaiters.add(resolve));
	}

	/**
	 * Send a request and wait for its response. If the service is not connected yet the request
	 * waits for the connection, within the same timeout.
	 */
	async request<R = Record<string, unknown>>(
		method: string,
		params: Record<string, unknown> = {},
		options: { timeoutMs?: number } = {}
	): Promise<R> {
		const timeoutMs = options.timeoutMs ?? this.requestTimeoutMs;
		const deadline = Date.now() + timeoutMs;
		if (!this.isOpen) {
			if (!this.wanted) throw new RequestError('offline', `The ${this.service} service is not connected.`, method);
			let settle: (opened: boolean) => void = () => {};
			const waiter = () => settle(true);
			const opened = await new Promise<boolean>((resolve) => {
				const timer = setTimeout(() => resolve(false), timeoutMs);
				settle = (value) => {
					clearTimeout(timer);
					resolve(value);
				};
				this.openWaiters.add(waiter);
			});
			this.openWaiters.delete(waiter);
			if (!opened) {
				throw new RequestError(
					'timeout',
					`The ${this.service} service did not connect within ${Math.round(timeoutMs / 1000)} s.`,
					method
				);
			}
		}
		const remaining = Math.max(1, deadline - Date.now());
		const id = this.nextId++;
		return new Promise<R>((resolve, reject) => {
			const timer = setTimeout(() => {
				this.pending.delete(id);
				reject(new RequestError('timeout', `${method} got no response within ${Math.round(timeoutMs / 1000)} s.`, method));
			}, remaining);
			this.pending.set(id, { method, resolve: resolve as (r: unknown) => void, reject, timer });
			try {
				this.socket!.send(encodeFrame({ type: 'request', id, method, params }) as string);
			} catch (err) {
				clearTimeout(timer);
				this.pending.delete(id);
				reject(new RequestError('disconnected', `Could not send ${method}: ${(err as Error).message}`, method));
			}
		});
	}

	/** Subscribe to an event topic. Returns the unsubscribe function. */
	on(topic: string, handler: EventHandler): () => void {
		let set = this.handlers.get(topic);
		if (!set) this.handlers.set(topic, (set = new Set()));
		set.add(handler);
		return () => {
			set!.delete(handler);
		};
	}

	// -------------------------------------------------------------------------------------------

	private attempt(): void {
		this.clearRetry();
		this.attempts += 1;
		if (this.status !== 'incompatible') this.status = 'connecting';
		this.retryInMs = 0;
		let socket: SocketLike;
		try {
			socket = this.factory(this.url);
		} catch (err) {
			this.scheduleRetry(`Could not open ${this.url}: ${(err as Error).message}`);
			return;
		}
		socket.binaryType = 'arraybuffer';
		this.socket = socket;

		socket.onopen = () => {
			if (this.socket !== socket) return;
			this.helloTimer = setTimeout(() => {
				if (this.socket !== socket) return;
				this.drop(socket, `No hello from ${this.url} within ${this.helloTimeoutMs / 1000} s.`);
			}, this.helloTimeoutMs);
		};
		socket.onmessage = (ev) => {
			if (this.socket !== socket) return;
			this.receive(socket, ev.data);
		};
		socket.onerror = () => {
			/* the close event that follows carries the outcome */
		};
		socket.onclose = () => {
			if (this.socket !== socket) return;
			this.drop(socket, this.everOpened ? `Lost the connection to ${this.url}.` : `Nothing is answering at ${this.url}.`);
		};
	}

	private receive(socket: SocketLike, data: unknown): void {
		let message: Message;
		try {
			message = decodeFrame(data as string | ArrayBuffer);
		} catch (err) {
			// A malformed frame is a service bug. Report it and keep the connection.
			console.error(`[${this.service}] dropped a frame:`, err instanceof FrameError ? err.message : err);
			return;
		}

		if (!this.hello) {
			if (message.type !== 'hello') {
				this.drop(socket, `Expected hello first, got ${message.type}.`);
				return;
			}
			if (this.helloTimer) clearTimeout(this.helloTimer);
			this.helloTimer = null;
			if (message.protocol !== PROTOCOL) {
				this.incompatible(socket, `The service speaks ${message.protocol}; this shell speaks ${PROTOCOL}.`);
				return;
			}
			if (message.service !== this.service) {
				this.incompatible(socket, `${this.url} is the ${message.service} service, not ${this.service}.`);
				return;
			}
			this.hello = message;
			this.status = 'open';
			this.detail = null;
			this.retryInMs = 0;
			this.everOpened = true;
			this.backoffMs = this.backoffInitialMs;
			const waiters = [...this.openWaiters];
			this.openWaiters.clear();
			waiters.forEach((w) => w());
			return;
		}

		if (message.type === 'response') {
			const p = this.pending.get(message.id);
			if (!p) return; // late response for a request that already timed out
			this.pending.delete(message.id);
			clearTimeout(p.timer);
			if ('error' in message && message.error) {
				p.reject(new RequestError(message.error.code, message.error.message, p.method));
			} else if ('result' in message) {
				p.resolve(message.result);
			} else {
				p.reject(new RequestError('internal', `${p.method} returned neither a result nor an error.`, p.method));
			}
			return;
		}

		if (message.type === 'event') {
			const set = this.handlers.get(message.topic);
			if (!set) return;
			for (const handler of [...set]) {
				try {
					handler(message.data, message);
				} catch (err) {
					console.error(`[${this.service}] handler for ${message.topic} failed:`, err);
				}
			}
		}
	}

	private incompatible(socket: SocketLike, reason: string): void {
		this.teardown(reason, socket);
		this.status = 'incompatible';
		this.detail = reason;
		// No graceful degradation; retry slowly in case the service is replaced.
		this.backoffMs = this.backoffMaxMs;
		this.scheduleRetry(reason, true);
	}

	private drop(socket: SocketLike, reason: string): void {
		this.teardown(reason, socket);
		if (this.wanted) this.scheduleRetry(reason);
		else this.status = 'offline';
	}

	private teardown(reason: string, only?: SocketLike): void {
		const socket = this.socket;
		if (!socket || (only && socket !== only)) return;
		this.socket = null;
		this.hello = null;
		if (this.helloTimer) clearTimeout(this.helloTimer);
		this.helloTimer = null;
		socket.onopen = socket.onmessage = socket.onclose = socket.onerror = null;
		try {
			socket.close();
		} catch {
			/* already closed */
		}
		for (const [id, p] of this.pending) {
			clearTimeout(p.timer);
			p.reject(new RequestError('disconnected', `${p.method} was cut off: ${reason}`, p.method));
			this.pending.delete(id);
		}
	}

	private scheduleRetry(reason: string, keepStatus = false): void {
		if (!this.wanted) return;
		const wait = this.backoffMs;
		this.backoffMs = Math.min(this.backoffMaxMs, Math.round(this.backoffMs * 2));
		if (!keepStatus) this.status = 'retrying';
		this.detail = reason;
		this.retryInMs = wait;
		const due = Date.now() + wait;
		this.countdown = setInterval(() => {
			this.retryInMs = Math.max(0, due - Date.now());
		}, 250);
		this.retryTimer = setTimeout(() => {
			this.retryTimer = null;
			this.attempt();
		}, wait);
	}

	private clearRetry(): void {
		if (this.retryTimer) clearTimeout(this.retryTimer);
		if (this.countdown) clearInterval(this.countdown);
		this.retryTimer = null;
		this.countdown = null;
	}
}
