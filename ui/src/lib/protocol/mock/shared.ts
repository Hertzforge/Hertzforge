/**
 * Mock mode across windows: a display window opened from the main window reaches the main
 * window's mock control service, so both show the same run, as two windows on the real service
 * do.
 *
 * The main window offers a socket maker on `window`; a display window it opened (same origin)
 * calls it through `window.opener`. Frames cross between the two windows as text, or as bytes
 * copied into this window's own buffer, so nothing from the other window's realm reaches this
 * window's codec.
 */

import type { SocketFactory, SocketLike } from '../client.svelte';
import { mockSocketFactory, type MockServer } from './socket';

const KEY = '__hertzforgeMockControl';

type Maker = (url: string) => SocketLike;

/** Offer this window's mock control service to the display windows it opens. */
export function shareMock(server: MockServer, connectDelayMs?: number): void {
	if (typeof window === 'undefined') return;
	const make = mockSocketFactory(server, connectDelayMs);
	(window as unknown as Record<string, Maker>)[KEY] = (url: string) => make(url);
}

/** The opener's mock control service, as this window's socket factory; null when there is none. */
export function openerMock(): SocketFactory | null {
	if (typeof window === 'undefined') return null;
	let make: Maker | undefined;
	try {
		make = (window.opener as Record<string, Maker> | null)?.[KEY];
	} catch {
		return null;
	}
	if (typeof make !== 'function') return null;
	const maker = make;
	return (url) => new LinkedSocket(maker(url));
}

const copy = (data: unknown): string | ArrayBuffer => (typeof data === 'string' ? data : new Uint8Array(data as ArrayBuffer).slice().buffer);

/** This window's end of a socket that lives in the opener. */
class LinkedSocket implements SocketLike {
	binaryType = 'arraybuffer';
	onopen: ((ev: unknown) => void) | null = null;
	onmessage: ((ev: { data: unknown }) => void) | null = null;
	onclose: ((ev: { code?: number; reason?: string }) => void) | null = null;
	onerror: ((ev: unknown) => void) | null = null;
	private readonly unload = () => this.close();

	constructor(private readonly inner: SocketLike) {
		inner.onopen = () => this.onopen?.({});
		inner.onmessage = (ev) => this.onmessage?.({ data: copy(ev.data) });
		inner.onclose = (ev) => {
			window.removeEventListener('pagehide', this.unload);
			this.onclose?.({ code: ev?.code, reason: ev?.reason });
		};
		inner.onerror = () => this.onerror?.({});
		// The opener's service must see this window go, or it would keep sending to it.
		window.addEventListener('pagehide', this.unload);
	}

	get readyState(): number {
		return this.inner.readyState;
	}

	send(data: string | ArrayBufferLike | ArrayBufferView): void {
		this.inner.send(typeof data === 'string' ? data : copy(data instanceof ArrayBuffer ? data : (data as ArrayBufferView).buffer));
	}

	close(): void {
		this.inner.close();
	}
}
