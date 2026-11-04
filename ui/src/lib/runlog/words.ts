/**
 * The Run Log window's words and arithmetic, kept out of the component so they can be tested: the
 * status strip's lines, the time since a run started, the day grouped by date, the keys of the
 * acts, and which act a key press asks for.
 */

import type { RunLogRow, RunLogStatus, RunLogView } from './types';

/** A whole number of seconds as H:MM:SS, or M:SS under an hour. */
export function span(seconds: number): string {
	const s = Math.max(0, Math.floor(seconds));
	const h = Math.floor(s / 3600);
	const m = Math.floor((s % 3600) / 60);
	const r = String(s % 60).padStart(2, '0');
	return h ? `${h}:${String(m).padStart(2, '0')}:${r}` : `${m}:${r}`;
}

/** The strip's three lines: the test, where in it, and running or stopped in words. */
export function statusWords(status: RunLogStatus | undefined): { test: string; place: string; state: string } {
	if (!status) return { test: 'No log open', place: '', state: '' };
	if (status.test === null) return { test: 'No test open', place: '', state: '' };
	const place = [status.axis ? `axis ${status.axis}` : 'no axis'];
	if (status.run != null) place.push(`run ${status.run}`);
	if (status.level_db != null) place.push(status.level_db === 0 ? 'full level' : `${status.level_db > 0 ? '+' : ''}${status.level_db} dB`);
	const state =
		{ running: 'RUNNING', aborted: 'STOPPED: aborted', stopped: 'STOPPED: scheduled stop', completed: 'STOPPED: completed' }[status.outcome ?? ''] ?? 'STOPPED';
	return { test: `Test ${status.test}: ${status.title} (${status.kind})`, place: place.join(', '), state };
}

/** The time since the run started, to `now` while it runs; once stopped, how long it ran. */
export function sinceWords(status: RunLogStatus | undefined, now: number): string {
	if (!status?.started) return '';
	const started = Date.parse(status.started);
	if (!status.stopped) return `${span((now - started) / 1000)} since start`;
	return `stopped after ${span((Date.parse(status.stopped) - started) / 1000)}`;
}

/** The notices under the strip: read-only, a mirror not written, lines not read. */
export function notices(log: RunLogView): string[] {
	const out: string[] = [];
	if (!log.writable) out.push(`Read-only: ${log.read_only || 'opened to read'}`);
	if (log.mirror_problem) out.push(`Mirror: ${log.mirror_problem}`);
	if (log.problems.length)
		out.push(`${log.problems.length} line${log.problems.length === 1 ? '' : 's'} not read or not fitting: ${log.problems.slice(0, 3).join('; ')}`);
	return out;
}

/** The day's rows of one test (or every test for null), grouped under their dates in order. */
export function byDate(rows: RunLogRow[], test: number | null): { date: string; rows: RunLogRow[] }[] {
	const out: { date: string; rows: RunLogRow[] }[] = [];
	for (const row of rows) {
		if (test !== null && row.test !== test) continue;
		const last = out[out.length - 1];
		if (last && last.date === row.date) last.rows.push(row);
		else out.push({ date: row.date, rows: [row] });
	}
	return out;
}

/**
 * The acts and their keys, as Visual Dynamics binds them: each with Shift and the platform's
 * command key, so none is taken from a box being typed into.
 */
export const KEYS: Record<string, string> = {
	abort: 'A',
	'run start': 'R',
	'run stop': 'S',
	note: 'N',
	'clock check': 'K',
	'person in': 'I',
	'person out': 'O',
	'cause found': 'C',
	'level row': 'L',
	header: 'H',
	sweep: 'W',
	shot: 'P',
	result: 'F',
	notch: 'T',
	deviation: 'D',
	anomaly: 'M',
	disposition: 'X',
	'setting change': 'G',
	opinion: 'E'
};

/** The act a key press asks for: Shift and Cmd (Ctrl elsewhere) with an act's letter, or null. */
export function actFor(e: { key: string; shiftKey: boolean; metaKey: boolean; ctrlKey: boolean; altKey: boolean }, mac: boolean): string | null {
	if (!e.shiftKey || e.altKey || !(mac ? e.metaKey : e.ctrlKey)) return null;
	const letter = e.key.toUpperCase();
	return Object.keys(KEYS).find((act) => KEYS[act] === letter) ?? null;
}

/** The key an act is bound to, as the platform writes it. */
export function keyName(act: string, mac: boolean): string {
	const letter = KEYS[act];
	return letter ? `${mac ? 'Cmd' : 'Ctrl'}+Shift+${letter}` : '';
}
