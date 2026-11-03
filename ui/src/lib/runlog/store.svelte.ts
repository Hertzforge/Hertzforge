/**
 * The Run Log window's state: the log the analysis service holds open for this connection, the
 * form being typed (the level row when none), what the form computes as it is typed, and the
 * keeper's initials and axis. Every read and write goes through `runlog.*`; the service reads
 * each box by its kind and judges the entry, so this only keeps what is on screen.
 */

import type { ServiceClient } from '$lib/protocol/client.svelte';
import type { FormSpec, RunLogView } from './types';

const INITIALS_KEY = 'hertzforge.runlog.initials';
const SHOWN_KEY = 'hertzforge.runlog.shown';
const WIDTH_KEY = 'hertzforge.runlog.width';

/** How long typing rests before a live form is read again, in ms. */
const PREVIEW_REST_MS = 120;

function stored(key: string): string | null {
	try {
		return localStorage.getItem(key);
	} catch {
		return null;
	}
}

function store(key: string, value: string): void {
	try {
		localStorage.setItem(key, value);
	} catch {
		// kept for this session only
	}
}

function words(err: unknown): string {
	return err instanceof Error ? err.message : String(err);
}

/** The texts a form's boxes open with, by field. */
function opening(spec: FormSpec): Record<string, string> {
	const out: Record<string, string> = {};
	for (const f of spec.fields) if (f.box !== 'heading') out[f.field] = f.value ?? '';
	return out;
}

/** A form being typed: what it is, what its boxes hold, its line of free text and its event time. */
export interface Typing {
	spec: FormSpec;
	texts: Record<string, string>;
	text: string;
	time: string;
	preview: string;
	message: string;
	busy: boolean;
}

export class RunLogState {
	/** The window is open beside the workspace. */
	shown = $state(stored(SHOWN_KEY) === '1');
	/** The log alone fills the window, for a keeper who analyzes elsewhere. */
	alone = $state(false);
	/** The window's width beside a workspace, in CSS pixels. */
	width = $state(Number(stored(WIDTH_KEY)) || 560);
	log = $state.raw<RunLogView | null>(null);
	/** The form open; null shows the level row. */
	form = $state<Typing | null>(null);
	/** The level row: one entry per step; its event time is taken at its first keystroke. */
	level = $state<Typing | null>(null);
	levelStarted: string | null = null;
	/** What the window says under the entry. */
	message = $state('');
	initials = $state(stored(INITIALS_KEY) ?? '');
	axis = $state('');
	/** The entry picked in the day, which Correct corrects. */
	picked = $state<number | null>(null);
	/** The test the day and the owed lists show; null for the whole log. */
	test = $state<number | null>(null);
	/** The current test when the day was last filled: a keeper looking at it follows the roll. */
	private following: number | null = null;
	holdsShown = $state(false);
	/** Asking for a log's path, to open or to start one, in a browser that has no file dialog. */
	asking = $state<'open' | null>(null);
	private timer: ReturnType<typeof setTimeout> | null = null;

	constructor(private readonly analysis: ServiceClient) {}

	toggle(): void {
		this.shown = !this.shown;
		if (!this.shown) this.alone = false;
		store(SHOWN_KEY, this.shown ? '1' : '0');
		if (this.shown) void this.sync();
	}

	resize(width: number): void {
		this.width = Math.round(Math.min(Math.max(width, 380), 1400));
		store(WIDTH_KEY, String(this.width));
	}

	setInitials(value: string): void {
		this.initials = value;
		store(INITIALS_KEY, value.trim());
	}

	private async call<R>(method: string, params: Record<string, unknown> = {}): Promise<R> {
		return this.analysis.request<R>(method, params);
	}

	/** Show the log as it stands, and the level row it offers. */
	private show(log: RunLogView | null): void {
		const was = this.log?.status.test ?? null;
		this.log = log;
		if (!log) {
			this.form = null;
			this.level = null;
			this.picked = null;
			return;
		}
		// a keeper looking at the current test follows it to the next when the log rolls
		if (this.test !== null && this.test === this.following && log.status.test !== null) this.test = log.status.test;
		this.following = log.status.test;
		if (log.status.axis && !this.axis.trim()) this.axis = log.status.axis;
		if (was !== log.status.test || !this.levelStarted) void this.offerLevel();
	}

	/** The service's log for this connection, as it stands (one opened before a reload, say). */
	async sync(): Promise<void> {
		if (!this.analysis.isOpen) return;
		try {
			this.show((await this.call<{ log: RunLogView | null }>('runlog.state')).log);
		} catch (err) {
			this.message = words(err);
		}
	}

	private async swap(method: string, params: Record<string, unknown>): Promise<void> {
		try {
			this.show((await this.call<{ log: RunLogView | null }>(method, params)).log);
			this.message = '';
			this.asking = null;
		} catch (err) {
			this.message = words(err);
		}
	}

	openLog(path: string): Promise<void> {
		return this.swap('runlog.open', { path });
	}

	closeLog(): Promise<void> {
		return this.swap('runlog.close', {});
	}

	reload(): Promise<void> {
		return this.swap('runlog.reload', {});
	}

	/** The level row, offering the next level of the step schedule. */
	private async offerLevel(): Promise<void> {
		if (!this.log) return;
		try {
			const spec = await this.call<FormSpec>('runlog.form', { form: 'level row', axis: this.axis });
			this.levelStarted = null;
			this.level = { spec, texts: opening(spec), text: '', time: '', preview: spec.preview, message: '', busy: false };
		} catch (err) {
			this.message = words(err);
		}
	}

	/** Open form `name`, its event time taken now; a correction corrects the entry picked. */
	async open(name: string): Promise<void> {
		if (name === 'level row') {
			this.form = null;
			return;
		}
		if (name === 'correct' && this.picked === null) {
			this.message = 'Pick the entry to correct in the day list first.';
			return;
		}
		try {
			const spec = await this.call<FormSpec>('runlog.form', { form: name, axis: this.axis, target: name === 'correct' ? this.picked : undefined });
			this.form = { spec, texts: opening(spec), text: '', time: spec.time, preview: spec.preview, message: '', busy: false };
			this.message = '';
		} catch (err) {
			this.message = words(err);
		}
	}

	cancel(): void {
		this.form = null;
	}

	/** A box typed into: a live form is read again once typing rests. */
	type(typing: Typing, field: string, value: string): void {
		typing.texts[field] = value;
		if (typing === this.level && !this.levelStarted) {
			this.levelStarted = new Date().toISOString();
			typing.time = new Date().toTimeString().slice(0, 8);
		}
		if (!typing.spec.live) return;
		if (this.timer) clearTimeout(this.timer);
		this.timer = setTimeout(() => void this.read(typing), PREVIEW_REST_MS);
	}

	private async read(typing: Typing): Promise<void> {
		try {
			const res = await this.call<{ text: string }>('runlog.preview', {
				form: typing.spec.name,
				values: { ...typing.texts },
				axis: this.axis,
				target: typing.spec.target
			});
			typing.preview = res.text;
		} catch (err) {
			typing.preview = `not read: ${words(err)}`;
		}
	}

	/** Write the form as typed; it closes once written, or says why on itself. */
	async write(typing: Typing): Promise<void> {
		if (typing.busy) return;
		if (typing.spec.name === 'new log') return this.newLog(typing);
		if (!this.initials.trim()) {
			typing.message = 'Type your initials first: every entry records its author.';
			return;
		}
		const isLevel = typing === this.level;
		typing.busy = true;
		try {
			const res = await this.call<{ log: RunLogView; said: string }>('runlog.write', {
				form: typing.spec.name,
				values: { ...typing.texts },
				text: typing.text,
				time: isLevel ? '' : typing.time,
				taken: isLevel ? (this.levelStarted ?? new Date().toISOString()) : typing.spec.taken,
				author: this.initials.trim(),
				axis: this.axis,
				target: typing.spec.target
			});
			typing.busy = false;
			if (!isLevel) this.form = null;
			this.message = res.said;
			this.levelStarted = null;
			this.show(res.log);
			if (isLevel) void this.offerLevel();
		} catch (err) {
			typing.busy = false;
			typing.message = words(err);
		}
	}

	private async newLog(typing: Typing): Promise<void> {
		try {
			const res = await this.call<{ log: RunLogView }>('runlog.write', { form: 'new log', values: { ...typing.texts }, author: this.initials, taken: '' });
			this.form = null;
			this.message = '';
			this.show(res.log);
		} catch (err) {
			typing.message = words(err);
		}
	}
}
