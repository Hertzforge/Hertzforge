/**
 * Mounts the MIMO run view on its own, against a control service, for the end-to-end check on the
 * simulator (`tools/e2e-sim/mimo_views.mjs`): the same view the MIMO screen's Run step shows, as a
 * window that observes the run. The shell's build never imports this module; the check loads it
 * through the development server.
 */

import { mount } from 'svelte';
import { ServiceClient, type SocketFactory } from '$lib/protocol/client.svelte';
import type { MimoGeometry } from '$lib/viz/geometry';
import type { RowBand } from '$lib/viz/scales';
import { AcquireState } from '../../store.svelte';
import { MimoState } from '../mimoStore.svelte';
import MimoRunView from './MimoRunView.svelte';
import { MimoScreenState } from './mimoScreenState.svelte';

export interface HarnessOptions {
	controlUrl: string;
	title: string;
	/** The page to open on; the first page otherwise. */
	page?: string | null;
	compact?: boolean;
	geometry?: MimoGeometry | null;
	bands?: RowBand[];
	/** A stand-in for the control service's socket; a WebSocket to `controlUrl` otherwise. */
	socketFactory?: SocketFactory;
}

export function mountRunView(o: HarnessOptions): MimoScreenState {
	const host = document.createElement('div');
	host.setAttribute('data-testid', 'mimo-harness');
	Object.assign(host.style, {
		position: 'fixed',
		inset: '0',
		zIndex: '50',
		display: 'flex',
		flexDirection: 'column',
		padding: '12px',
		overflow: 'hidden',
		background: 'var(--bg-0)',
		color: 'var(--text-1)',
		fontFamily: 'var(--font-ui)'
	});
	document.body.appendChild(host);
	const control = new ServiceClient({ service: 'control', url: o.controlUrl, socketFactory: o.socketFactory });
	// An observer of the run, as a window on another monitor is: no lease, no drafts.
	const acq = new AcquireState(control, { session: null, local: null, drafts: false });
	const mimo = new MimoState(acq, { local: null, drafts: false });
	const screen = new MimoScreenState(control, {
		setupTitle: () => o.title,
		showing: () => true,
		page: o.page ?? null,
		storage: null,
		session: null
	});
	screen.watch();
	control.connect();
	const synced = setInterval(() => {
		if (control.status !== 'open') return;
		clearInterval(synced);
		void acq.sync();
		void mimo.sync();
	}, 50);
	mount(MimoRunView, { target: host, props: { mimo, screen, geometry: o.geometry ?? null, bands: o.bands ?? [], compact: o.compact ?? false, abortKeys: 'Ctrl .' } });
	(globalThis as Record<string, unknown>).__mimoViews = screen;
	return screen;
}
