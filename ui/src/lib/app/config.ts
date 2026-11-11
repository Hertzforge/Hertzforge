/** Shell configuration from the page URL. */

import type { ControlServerOptions } from '$lib/protocol/mock/control';
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
	/**
	 * `?popout=1&page=<id>`: a window that shows one display page of the run, opened from the main
	 * window to place on another monitor. It observes the run with its own subscription.
	 */
	popout: boolean;
	/** `?runlog=1`: the log alone fills the window (`--run-log` on the desktop app). */
	runLog: boolean;
	/** `?runlog_file=<path>`: the log to open there. */
	runLogFile: string | null;
	page: string | null;
	/** `&test=mimo_random`, `&test=shock`, `&test=srs`, `&test=sine`, `&test=twr`, `&test=explorer` or `&test=recorder`: the pop-out shows that run's page; otherwise Random's. */
	popoutTest: 'random' | 'mimo_random' | 'shock' | 'srs' | 'sine' | 'mixed' | 'twr' | 'explorer' | 'recorder';
	/**
	 * How the mock control service behaves, from `mock_*` parameters: `mock_speed` (run seconds
	 * per second), `mock_ni=real|absent`, `mock_abort_at` (seconds after engaging at which a
	 * resonance grows to an envelope abort), `mock_feasibility=exceeds`, `mock_coherence`.
	 */
	mockControl: ControlServerOptions;
}

function num(value: string | null): number | undefined {
	if (value === null || value.trim() === '') return undefined;
	const n = Number(value);
	return Number.isFinite(n) ? n : undefined;
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
		theme: theme === 'dark' || theme === 'light' || theme === 'system' ? theme : null,
		popout: q.get('popout') === '1',
		runLog: q.get('runlog') === '1',
		runLogFile: q.get('runlog_file'),
		page: q.get('page') && /^[\w-]{1,64}$/.test(q.get('page') as string) ? q.get('page') : null,
		popoutTest: q.get('test') === 'mimo_random' ? 'mimo_random' : q.get('test') === 'shock' ? 'shock' : q.get('test') === 'srs' ? 'srs' : q.get('test') === 'sine' ? 'sine' : q.get('test') === 'mixed' ? 'mixed' : q.get('test') === 'twr' ? 'twr' : q.get('test') === 'explorer' ? 'explorer' : q.get('test') === 'recorder' ? 'recorder' : 'random',
		mockControl: {
			speed: num(q.get('mock_speed')),
			niReal: q.get('mock_ni') === 'real' ? true : undefined,
			niInstalled: q.get('mock_ni') === 'absent' ? false : undefined,
			abortDemoAtS: num(q.get('mock_abort_at')),
			feasibilityExceeds: q.get('mock_feasibility') === 'exceeds' ? true : undefined,
			coherence: num(q.get('mock_coherence'))
		}
	};
}
