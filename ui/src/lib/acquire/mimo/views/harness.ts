/**
 * Mounts the MIMO run view on its own, against a control service, for the end-to-end check on the
 * simulator (`tools/e2e-sim/mimo_views.mjs`), until the MIMO screen hosts it in the shell. The
 * shell's build never imports this module; the check loads it through the development server.
 */

import { mount } from 'svelte';
import { ServiceClient } from '$lib/protocol/client.svelte';
import type { MimoGeometry } from '$lib/viz/geometry';
import type { RowBand } from '$lib/viz/scales';
import MimoRunView from './MimoRunView.svelte';
import { MimoViewsState } from './mimoViews.svelte';

export interface HarnessOptions {
	controlUrl: string;
	title: string;
	monitors?: number;
	windowIndex?: number;
	page?: string | null;
	compact?: boolean;
	geometry?: MimoGeometry | null;
	bands?: RowBand[];
}

export function mountRunView(o: HarnessOptions): MimoViewsState {
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
	const control = new ServiceClient({ service: 'control', url: o.controlUrl });
	const views = new MimoViewsState(control, {
		setupTitle: () => o.title,
		showing: () => true,
		monitors: o.monitors ?? 1,
		windowIndex: o.windowIndex ?? 0,
		page: o.page ?? null,
		session: null
	});
	views.watch();
	control.connect();
	mount(MimoRunView, { target: host, props: { views, geometry: o.geometry ?? null, bands: o.bands ?? [], compact: o.compact ?? false } });
	(globalThis as Record<string, unknown>).__mimoViews = views;
	return views;
}
