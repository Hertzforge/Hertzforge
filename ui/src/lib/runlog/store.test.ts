/** The Run Log's entries proposed from a run: listed from the run record, written one at a time. */
import { describe, expect, it } from 'vitest';
import type { ServiceClient } from '$lib/protocol/client.svelte';
import { NO_RUNS, RunLogState } from './store.svelte';
import type { RunEvent, RunLogView } from './types';

const LOG = { path: '/logs/a.hflog', writable: true, status: { test: 1 }, rows: [], run: 2 } as unknown as RunLogView;

function event(index: number, over: Partial<RunEvent> = {}): RunEvent {
	return { index, kind: 'note', label: 'Note', fields: {}, text: `event ${index} [run r-1 #${index}]`, time: '2026-11-14T10:00:0' + index + '-07:00', accepted: false, ...over };
}

/** A service that answers `answers[method]` and keeps every request it was sent. */
function service(answers: Record<string, (params: Record<string, unknown>) => unknown>) {
	const sent: { method: string; params: Record<string, unknown> }[] = [];
	const client = {
		isOpen: true,
		async request(method: string, params: Record<string, unknown> = {}) {
			sent.push({ method, params });
			const answer = answers[method];
			if (!answer) throw new Error(`not expected: ${method}`);
			return answer(params);
		}
	} as unknown as ServiceClient;
	return { client, sent };
}

describe('entries proposed from a run', () => {
	it('lists the run records of the project, and says so when the window holds none', async () => {
		const none = service({ 'project.setups': () => ({ setups: [], runs: [] }) });
		const empty = new RunLogState(none.client);
		await empty.loadRuns();
		expect(empty.runs).toEqual([]);
		expect(empty.eventsMessage).toBe(NO_RUNS);

		const one = service({ 'project.setups': () => ({ setups: [], runs: [{ name: 'Run Record', module: 'sine', title: 'Sine 1 g', run_id: 'sine-0001', outcome: 'completed' }] }) });
		const rl = new RunLogState(one.client);
		await rl.loadRuns();
		expect(rl.runs.map((r) => r.name)).toEqual(['Run Record']);
		expect(rl.eventsMessage).toBe('');
	});

	it('lists a run record\'s events as proposals and writes nothing', async () => {
		const { client, sent } = service({ 'runlog.run_events': (p) => ({ record: p.record, events: [event(0), event(1, { accepted: true })] }) });
		const rl = new RunLogState(client);
		await rl.pickRecord('Run Record');
		expect(rl.events.map((e) => [e.index, e.accepted])).toEqual([[0, false], [1, true]]);
		expect(sent.map((s) => s.method)).toEqual(['runlog.run_events']);
		expect(sent[0].params).toEqual({ record: 'Run Record' });
	});

	it('writes one entry only when the keeper accepts it with their initials', async () => {
		const { client, sent } = service({
			'runlog.run_events': (p) => ({ record: p.record, events: [event(0), event(1)] }),
			'runlog.accept_event': () => ({ log: LOG, entry: 7 })
		});
		const rl = new RunLogState(client);
		rl.log = LOG;
		await rl.pickRecord('Run Record');
		rl.initials = '';
		await rl.acceptEvent(rl.events[0]);
		expect(sent.filter((s) => s.method === 'runlog.accept_event')).toEqual([]);
		expect(rl.eventsMessage).toMatch(/initials/);

		rl.initials = 'KC';
		rl.axis = 'Z';
		await rl.acceptEvent(rl.events[0]);
		const writes = sent.filter((s) => s.method === 'runlog.accept_event');
		expect(writes.map((s) => s.params)).toEqual([{ record: 'Run Record', index: 0, author: 'KC', axis: 'Z' }]);
		expect(rl.events.map((e) => e.accepted)).toEqual([true, false]);
		expect(rl.eventsMessage).toBe('Written as entry #7.');
		// An event accepted already is not sent again.
		await rl.acceptEvent(rl.events[0]);
		expect(sent.filter((s) => s.method === 'runlog.accept_event').length).toBe(1);
	});

	it('asks for a time for an event the record has none for, and sends the one typed', async () => {
		const { client, sent } = service({
			'runlog.run_events': (p) => ({ record: p.record, events: [event(0, { time: null })] }),
			'runlog.accept_event': () => ({ log: LOG, entry: 8 })
		});
		const rl = new RunLogState(client);
		rl.log = LOG;
		rl.initials = 'KC';
		await rl.pickRecord('Run Record');
		await rl.acceptEvent(rl.events[0]);
		expect(sent.filter((s) => s.method === 'runlog.accept_event')).toEqual([]);
		expect(rl.eventsMessage).toMatch(/no time for this event/);
		rl.eventTimes[0] = '2026-11-14T09:30:00';
		await rl.acceptEvent(rl.events[0]);
		expect(sent.filter((s) => s.method === 'runlog.accept_event')[0].params).toMatchObject({ index: 0, time: '2026-11-14T09:30:00' });
	});

	it("shows the service's refusal and leaves the event proposed", async () => {
		const { client } = service({
			'runlog.run_events': (p) => ({ record: p.record, events: [event(0)] }),
			'runlog.accept_event': () => {
				throw new Error('This log is open read-only.');
			}
		});
		const rl = new RunLogState(client);
		rl.initials = 'KC';
		await rl.pickRecord('Run Record');
		await rl.acceptEvent(rl.events[0]);
		expect(rl.events[0].accepted).toBe(false);
		expect(rl.eventsMessage).toBe('This log is open read-only.');
		expect(rl.accepting).toBeNull();
	});
});
