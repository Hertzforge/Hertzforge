/**
 * The test types Acquire & Control offers, and which screen the workspace shows: the chooser, or
 * one test type's own screen.
 *
 * - The workspace opens on the chooser. Only a test type that is ready can be chosen; the others
 *   are shown as coming.
 * - The choice is kept for the session: moving to another workspace and back returns to it.
 * - While a run is live, its screen has no way back to the chooser, and opening the workspace goes
 *   straight to the run. A chooser already on screen when a run goes live (started from another
 *   screen) stays, and the run's card and the top bar's run pill return to the run.
 */

import { untrack } from 'svelte';
import type { ConnectionStatus } from '$lib/protocol/client.svelte';
import type { RandomState } from '$lib/protocol/types';
import { isLive } from './actions';

export type TestTypeId = 'random' | 'sine' | 'shock' | 'srs' | 'twr' | 'recorder' | 'explorer';

export interface TestType {
	id: TestTypeId;
	title: string;
	/** What the test is, in one plain line. */
	summary: string;
	/** It can be chosen; the others are shown as coming. */
	ready: boolean;
}

export const TEST_TYPES: readonly TestType[] = [
	{ id: 'random', title: 'Random', summary: 'Broadband random vibration shaped to a target spectrum and held for a set time.', ready: true },
	{ id: 'sine', title: 'Sine', summary: 'One frequency at a time, swept across a range or held, at a controlled amplitude.', ready: false },
	{ id: 'shock', title: 'Classical Shock', summary: 'Short pulses of a standard shape, such as a half-sine, at a set peak and duration.', ready: false },
	{ id: 'srs', title: 'SRS', summary: 'Shock response spectrum: a short transient built to meet a target response spectrum.', ready: false },
	{ id: 'twr', title: 'TWR', summary: 'Time waveform replication: a recorded signal reproduced at the control point.', ready: false },
	{ id: 'recorder', title: 'Recorder', summary: 'Records the inputs with no drive, continuously or from a level trigger.', ready: false },
	{ id: 'explorer', title: 'Explorer', summary: 'A sine you steer live by hand, to find resonances and watch the response.', ready: false }
];

/** What the workspace shows. */
export type AcquireView = 'chooser' | TestTypeId;

/** The test type whose run is live; null when none is; undefined until the control service has said. */
export type LiveTest = TestTypeId | null | undefined;

/**
 * Which test type holds a live run, from the Random store and its connection. Before the first
 * read the answer is not known yet; a service that cannot be reached, or that refused the read,
 * has nothing live to show.
 */
export function liveTestOf(acq: { run: RandomState | null; syncError: string | null }, status: ConnectionStatus): LiveTest {
	if (acq.run !== null) return isLive(acq.run) ? 'random' : null;
	const waiting = status === 'offline' || status === 'connecting' || status === 'open';
	return waiting && acq.syncError === null ? undefined : null;
}

export class TestTypeView {
	readonly types: readonly TestType[];
	private readonly liveTest: () => LiveTest;

	/** Where the operator is: the chooser, or a test type's screen. */
	view = $state<AcquireView>('chooser');
	/** The test type on screen last this session. */
	last = $state<TestTypeId | null>(null);
	/** The workspace opened, and the control service's answer on a live run is not applied yet. */
	private settling = $state(true);
	/** A change of screen the operator asked for: the new screen takes the keyboard focus once. */
	private focusNext = false;

	constructor(liveTest: () => LiveTest, types: readonly TestType[] = TEST_TYPES) {
		this.liveTest = liveTest;
		this.types = types;
	}

	type(id: TestTypeId): TestType | undefined {
		return this.types.find((t) => t.id === id);
	}

	/** The test type whose run is live, or null. */
	get live(): TestTypeId | null {
		return this.liveTest() ?? null;
	}

	/**
	 * The screen to show, or null while the workspace is opening and the service has not yet said
	 * whether a run is live. A live run's answer is shown as soon as it is known; `settle` then
	 * makes it the operator's place.
	 */
	get screen(): AcquireView | null {
		if (this.settling) {
			const live = this.liveTest();
			if (live === undefined) return null;
			if (live) return live;
		}
		return this.view;
	}

	/** Neither the chooser nor a test type's screen is shown yet. */
	get deciding(): boolean {
		return this.screen === null;
	}

	/** Why a test type cannot be chosen now, in words, or null when it can. */
	whyNot(id: TestTypeId): string | null {
		const t = this.type(id);
		if (!t?.ready) return 'Coming';
		const live = this.live;
		if (live && live !== id) return `Opens when the ${this.type(live)?.title ?? ''} run ends.`;
		return null;
	}

	canChoose(id: TestTypeId): boolean {
		return this.whyNot(id) === null;
	}

	/** The chooser can be reached from a test type's screen: no run is live. */
	get canLeave(): boolean {
		return this.live === null;
	}

	/** The workspace opens: a live run comes straight to the screen. */
	open(): void {
		const live = this.liveTest();
		if (live === undefined) {
			this.settling = true;
			return;
		}
		this.settling = false;
		if (live) this.show(live, false);
	}

	/** Open a test type's screen, if it can be chosen now. */
	choose(id: TestTypeId): boolean {
		if (!this.canChoose(id)) return false;
		this.show(id, this.view !== id);
		return true;
	}

	/** Back to the chooser; refused while a run is live. */
	toChooser(): boolean {
		if (!this.canLeave) return false;
		if (this.view !== 'chooser') this.focusNext = true;
		this.settling = false;
		this.view = 'chooser';
		return true;
	}

	/** The live run's screen, from wherever the operator is (the top bar's run pill). */
	showRun(): boolean {
		const live = this.live;
		if (!live) return false;
		this.show(live, true);
		return true;
	}

	/** The service has said what is live (or it changed): a workspace still opening goes to a live run. */
	settle(live: LiveTest): void {
		if (live === undefined || !this.settling) return;
		this.settling = false;
		if (live) this.show(live, false);
	}

	/** The screen that has just come up takes the focus if the operator moved to it. */
	takeFocus(): boolean {
		const f = this.focusNext;
		this.focusNext = false;
		return f;
	}

	/** Settle as the live answer arrives and changes. Returns the stopper. */
	watch(): () => void {
		return $effect.root(() => {
			$effect(() => {
				const live = this.liveTest();
				untrack(() => this.settle(live));
			});
		});
	}

	private show(id: TestTypeId, focus: boolean): void {
		if (focus) this.focusNext = true;
		this.settling = false;
		this.view = id;
		this.last = id;
	}
}
