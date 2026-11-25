/**
 * The test types Acquire & Control offers, and which screen the workspace shows: the chooser, or
 * one test type's own screen.
 *
 * - The workspace opens on the chooser, in three sections: Single Output (one drive controlled),
 *   MIMO (multiple-input multiple-output: several shakers or drives controlled together), and
 *   Specialized (the Recorder, Explorer, Playback and Modal Test: inputs recorded with no drive, a
 *   sine steered by hand, a drive file played open loop, or FRFs measured by hammer or shaker). Only a test type that is ready can be chosen; the
 *   others are shown as coming.
 * - The choice is kept for the session: moving to another workspace and back returns to it.
 * - While a run is live, its screen has no way back to the chooser, and opening the workspace goes
 *   straight to the run. A chooser already on screen when a run goes live (started from another
 *   screen) stays, and the run's card and the top bar's run pill return to the run.
 * - Configure, at the upper right of every one of these screens, sets the station every test type
 *   runs on (the simulator and its models, or NI-DAQmx devices); it goes back to where it was
 *   opened from.
 */

import { untrack } from 'svelte';
import type { ConnectionStatus } from '$lib/protocol/client.svelte';
import type { RandomRunState, RandomState } from '$lib/protocol/types';
import type { ShockRunState } from '$lib/protocol/shock';
import type { TwrRunState } from '$lib/protocol/twr';
import type { ExplorerRunState } from '$lib/protocol/explorer';
import type { RecorderRunState } from '$lib/protocol/recorder';
import type { PlaybackRunState } from '$lib/protocol/playback';
import type { MimoShockRunState } from '$lib/protocol/mimoShock';
import { MS_LIVE_STATES } from './mimoshock/msVerbs';
import type { MimoSineRunState } from '$lib/protocol/mimoSine';
import { MSINE_LIVE_STATES } from './mimosine/msineVerbs';
import type { MimoMixedRunState } from '$lib/protocol/mimoMixed';
import { isLive, LIVE_STATES } from './actions';
import { TWR_LIVE_STATES } from './twr/twrVerbs';
import { EXPLORER_LIVE_STATES } from './explorer/explorerVerbs';
import { RECORDER_LIVE_STATES } from './recorder/recorderVerbs';
import { PLAYBACK_LIVE_STATES } from './playback/playbackVerbs';
import type { ModalRunState } from '$lib/protocol/modal_test';
import { MODAL_LIVE_STATES } from './modal_test/modalTestVerbs';

export type TestTypeId =
	| 'random'
	| 'sine'
	| 'mixed'
	| 'shock'
	| 'srs'
	| 'twr'
	| 'recorder'
	| 'explorer'
	| 'playback'
	| 'modal_test'
	| 'mimo_random'
	| 'mimo_shock'
	| 'mimo_sine'
	| 'mimo_mixed'
	| 'mimo_twr';

/** The chooser's sections: one drive controlled, several controlled together, or the specialized tests. */
export type TestGroupId = 'single' | 'mimo' | 'specialized';

export interface TestGroup {
	id: TestGroupId;
	title: string;
}

export const TEST_GROUPS: readonly TestGroup[] = [
	{ id: 'single', title: 'Single Output' },
	{ id: 'mimo', title: 'MIMO' },
	{ id: 'specialized', title: 'Specialized' }
];

export interface TestType {
	id: TestTypeId;
	title: string;
	/** What the test is, in one plain line. */
	summary: string;
	/** It can be chosen; the others are shown as coming. */
	ready: boolean;
	/** The chooser section it is listed under. */
	group: TestGroupId;
}

export const TEST_TYPES: readonly TestType[] = [
	{ id: 'random', group: 'single', title: 'Random', summary: 'Broadband random vibration shaped to a target spectrum and held for a set time.', ready: true },
	{ id: 'sine', group: 'single', title: 'Sine', summary: 'One frequency at a time, swept across a range or held, at a controlled amplitude.', ready: true },
	{ id: 'mixed', group: 'single', title: 'Mixed Mode', summary: 'Sine tones and narrow bands of random on a random floor, each dwelling or sweeping.', ready: true },
	{ id: 'shock', group: 'single', title: 'Classical Shock', summary: 'Short pulses of a standard shape, such as a half-sine, at a set peak and duration.', ready: true },
	{ id: 'srs', group: 'single', title: 'SRS', summary: 'Shock response spectrum: a short transient built to meet a target response spectrum.', ready: true },
	{ id: 'twr', group: 'single', title: 'TWR', summary: 'Time waveform replication: a recorded signal reproduced at the control point.', ready: true },
	{ id: 'mimo_random', group: 'mimo', title: 'MIMO Random', summary: 'Random vibration from several shakers or drives at once, controlled together to target spectra.', ready: true },
	{ id: 'mimo_shock', group: 'mimo', title: 'MIMO Shock', summary: 'Shock pulses from several shakers at once, controlled together at every control point.', ready: true },
	{ id: 'mimo_sine', group: 'mimo', title: 'MIMO Sine', summary: 'A sine from several shakers at once, with the amplitude and phase between them controlled.', ready: true },
	{ id: 'mimo_mixed', group: 'mimo', title: 'MIMO Mixed Mode', summary: 'Sine tones and narrowband random on a random floor, on several shakers controlled together.', ready: true },
	{ id: 'mimo_twr', group: 'mimo', title: 'MIMO TWR', summary: 'Recorded signals reproduced at several control points at once, from several shakers.', ready: false },
	{ id: 'recorder', group: 'specialized', title: 'Recorder', summary: 'Records the inputs with no drive, continuously or from a level trigger.', ready: true },
	{ id: 'explorer', group: 'specialized', title: 'Explorer', summary: 'A sine you steer live by hand, to find resonances and watch the response.', ready: true },
	{ id: 'playback', group: 'specialized', title: 'Playback', summary: 'Plays a drive file you made on every drive, open loop, and records every input.', ready: true },
	{ id: 'modal_test', group: 'specialized', title: 'Modal Test', summary: 'Measures FRFs at every point by hammer or by shakers, for the modal fit.', ready: true }
];

/** The test types under each section of the chooser, in order. */
export function typesIn(group: TestGroupId, types: readonly TestType[] = TEST_TYPES): TestType[] {
	return types.filter((t) => t.group === group);
}

/** What the workspace shows: the chooser, a test type's screen, or Configure (the station, for every test type). */
export type AcquireView = 'chooser' | 'configure' | TestTypeId;

/** The test type whose run is live; null when none is; undefined until the control service has said. */
export type LiveTest = TestTypeId | null | undefined;

/**
 * Which test type holds a live run, from the Random, MIMO Random, classical shock, SRS shock, Sine,
 * TWR, Explorer, Recorder, MIMO Shock, Playback, MIMO Sine, Modal Test and MIMO Mixed Mode stores and their connection. Before the first read the answer is not known yet; a service that
 * cannot be reached, or that refused the read, has nothing live to show.
 */
export function liveTestOf(
	acq: { run: RandomState | null; syncError: string | null },
	status: ConnectionStatus,
	mimo?: { status: { state: RandomRunState } | null; syncError: string | null },
	shock?: { status: { state: ShockRunState } | null; syncError: string | null },
	srs?: { status: { state: ShockRunState } | null; syncError: string | null },
	sine?: { status: { state: RandomRunState } | null; syncError: string | null },
	twr?: { status: { state: TwrRunState } | null; syncError: string | null },
	explorer?: { status: { state: ExplorerRunState } | null; syncError: string | null },
	recorder?: { status: { state: RecorderRunState } | null; syncError: string | null },
	mixed?: { status: { state: RandomRunState } | null; syncError: string | null },
	mimoShock?: { status: { state: MimoShockRunState } | null; syncError: string | null },
	playback?: { status: { state: PlaybackRunState } | null; syncError: string | null },
	mimoSine?: { status: { state: MimoSineRunState } | null; syncError: string | null },
	modalTest?: { status: { state: ModalRunState } | null; syncError: string | null },
	mimoMixed?: { status: { state: MimoMixedRunState } | null; syncError: string | null }
): LiveTest {
	if (acq.run !== null && isLive(acq.run)) return 'random';
	if (mimo?.status && LIVE_STATES.includes(mimo.status.state)) return 'mimo_random';
	if (shock?.status && LIVE_STATES.includes(shock.status.state)) return 'shock';
	if (srs?.status && LIVE_STATES.includes(srs.status.state)) return 'srs';
	if (sine?.status && LIVE_STATES.includes(sine.status.state)) return 'sine';
	if (mixed?.status && LIVE_STATES.includes(mixed.status.state)) return 'mixed';
	if (twr?.status && TWR_LIVE_STATES.includes(twr.status.state)) return 'twr';
	if (explorer?.status && EXPLORER_LIVE_STATES.includes(explorer.status.state)) return 'explorer';
	if (recorder?.status && RECORDER_LIVE_STATES.includes(recorder.status.state)) return 'recorder';
	if (mimoShock?.status && MS_LIVE_STATES.includes(mimoShock.status.state)) return 'mimo_shock';
	if (playback?.status && PLAYBACK_LIVE_STATES.includes(playback.status.state)) return 'playback';
	if (mimoSine?.status && MSINE_LIVE_STATES.includes(mimoSine.status.state)) return 'mimo_sine';
	if (modalTest?.status && MODAL_LIVE_STATES.includes(modalTest.status.state)) return 'modal_test';
	if (mimoMixed?.status && LIVE_STATES.includes(mimoMixed.status.state)) return 'mimo_mixed';
	// Every run read has answered (or one could not be read): nothing is live.
	const answered = (s?: { status: unknown; syncError: string | null }) => !s || s.status !== null || s.syncError !== null;
	if (acq.run !== null && answered(mimo) && answered(shock) && answered(srs) && answered(sine) && answered(twr) && answered(explorer) && answered(recorder) && answered(mixed) && answered(mimoShock) && answered(playback) && answered(mimoSine) && answered(modalTest) && answered(mimoMixed)) return null;
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
	/** Where Configure was opened from, and goes back to. */
	configureFrom = $state<'chooser' | TestTypeId>('chooser');
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

	/**
	 * Open Configure from the screen on show. It can be opened while a run is live, to read the
	 * station; the service refuses a change then, and the view says so.
	 */
	toConfigure(): void {
		const here = this.screen;
		if (here === null || here === 'configure') return;
		this.configureFrom = here;
		this.focusNext = true;
		this.settling = false;
		this.view = 'configure';
	}

	/** Back from Configure to where it was opened. */
	fromConfigure(): void {
		if (this.view !== 'configure') return;
		const to = this.configureFrom;
		this.focusNext = true;
		if (to === 'chooser') this.view = 'chooser';
		else this.show(to, true);
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
