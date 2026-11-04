/** The MIMO run's state in plain words, for the level card and the run view. */

import type { MimoPhase, MimoStatus } from '$lib/protocol/mimo';
import { formatDb } from '$lib/viz/format';

export const MIMO_PHASE_WORDS: Record<MimoPhase, string> = {
	idle: 'Not set up',
	armed: 'Armed, nothing driving',
	sysid_noise: 'Measuring the noise floor',
	sysid: 'Identifying the plant',
	awaiting_review: 'Identification passed, holding for review',
	gate_failed: 'Held: identification did not pass its gates',
	engaged: 'Controlling',
	full_level: 'Controlling at full level',
	ramping_down: 'Stopping',
	stopped: 'Stopped',
	aborted: 'Aborted'
};

export interface MimoLevelStatus {
	mark: 'idle' | 'identify' | 'hold' | 'control' | 'ramp' | 'stop' | 'ended';
	text: string;
	tone: 'info' | 'pass' | 'warn' | 'fail';
}

/** What the run is doing, as a mark, a sentence and a tone. */
export function mimoLevelStatus(s: MimoStatus | null): MimoLevelStatus {
	if (!s) return { mark: 'idle', text: 'Waiting for the run', tone: 'info' };
	const words = MIMO_PHASE_WORDS[s.phase] ?? s.phase;
	switch (s.phase) {
		case 'sysid_noise':
		case 'sysid':
			return { mark: 'identify', text: words, tone: 'info' };
		case 'awaiting_review':
			return { mark: 'hold', text: words, tone: 'info' };
		case 'gate_failed':
			return { mark: 'hold', text: words, tone: 'warn' };
		case 'engaged':
			return { mark: 'ramp', text: s.level_db !== null ? `Controlling at ${formatDb(s.level_db)}` : words, tone: s.alarm ? 'warn' : 'pass' };
		case 'full_level':
			return { mark: 'control', text: s.level_db !== null ? `Controlling at ${formatDb(s.level_db)}` : words, tone: s.alarm ? 'warn' : 'pass' };
		case 'ramping_down':
			return { mark: 'stop', text: words, tone: 'info' };
		case 'stopped':
			return { mark: 'ended', text: s.end?.message ?? words, tone: 'info' };
		case 'aborted':
			return { mark: 'ended', text: s.end?.message ?? words, tone: 'fail' };
		default:
			return { mark: 'idle', text: words, tone: 'info' };
	}
}
