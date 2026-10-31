/** Shell configuration from the page URL. */

import type { ThemePref } from '$lib/theme/theme.svelte';

export const DEFAULT_ANALYSIS_URL = 'ws://127.0.0.1:8791/';
export const DEFAULT_CONTROL_URL = 'ws://127.0.0.1:8792/';

export interface ShellConfig {
	/** `?mock=1`: in-process mock services with synthetic data. */
	mock: boolean;
	/** `?analysis=ws://host:port/` */
	analysisUrl: string;
	/** `?control=ws://host:port/` */
	controlUrl: string;
	/** `?theme=dark|light|system` forces a theme for this load (screenshots, tests). */
	theme: ThemePref | null;
}

function wsUrl(value: string | null, fallback: string): string {
	if (!value) return fallback;
	try {
		const u = new URL(value);
		return u.protocol === 'ws:' || u.protocol === 'wss:' ? u.toString() : fallback;
	} catch {
		return fallback;
	}
}

export function readConfig(search: string): ShellConfig {
	const q = new URLSearchParams(search);
	const mock = q.get('mock');
	const theme = q.get('theme');
	return {
		mock: mock === '1' || mock === 'true',
		analysisUrl: wsUrl(q.get('analysis'), DEFAULT_ANALYSIS_URL),
		controlUrl: wsUrl(q.get('control'), DEFAULT_CONTROL_URL),
		theme: theme === 'dark' || theme === 'light' || theme === 'system' ? theme : null
	};
}
