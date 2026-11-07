import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { decodeFrame } from '$lib/protocol/codec';
import type { Motion, Shape3DView } from '$lib/protocol/types';
import { advance, movedNodes, NORMAL_SPEED, sampleAt, sampleCount, SPEEDS, writeFrame } from './motion';

/** The golden frame's motion: three DOFs of four samples, two of them on node 1. */
function golden(): Motion {
	const bytes = readFileSync(new URL('../../../../schema/golden/response_motion.bin', import.meta.url));
	const message = decodeFrame(new Uint8Array(bytes)) as unknown as { result: { views: Shape3DView[] } };
	return message.result.views[0].motion!;
}

describe('motion playback', () => {
	it('counts the samples of each DOF', () => {
		expect(sampleCount(golden())).toBe(4);
	});

	it('finds the sample nearest a time, held to the record', () => {
		// samples at 0, 0.25, 0.5 and 0.75 s
		const motion = golden();
		expect([0, 0.12, 0.13, 0.5, 0.74].map((t) => sampleAt(motion, t))).toEqual([0, 0, 1, 2, 3]);
		expect(sampleAt(motion, -1)).toBe(0);
		expect(sampleAt(motion, 9)).toBe(3);
	});

	it('lists each moved node once', () => {
		expect(Array.from(movedNodes(golden()))).toEqual([1, 2]);
	});

	it('sums the DOFs on one node and leaves unmoved nodes alone', () => {
		const out = new Float32Array(9).fill(7);
		const motion = golden();
		writeFrame(motion, 1, movedNodes(motion), out);
		// node 0 has no DOF; node 1 has Z 1.5 and Y 2.0; node 2 has Z 0 at sample 1
		expect(Array.from(out)).toEqual([7, 7, 7, 0, 2, 1.5, 0, 0, 0]);
		expect(Math.hypot(out[3], out[4], out[5])).toBe(motion.peak);
	});

	it('clears a moved node before the next sample adds to it', () => {
		const motion = golden();
		const out = new Float32Array(9);
		const moved = movedNodes(motion);
		writeFrame(motion, 1, moved, out);
		writeFrame(motion, 2, moved, out);
		expect(Array.from(out)).toEqual([0, 0, 0, 0, 0, 0, 0, 0, -0.5]);
	});

	it('plays a record in ten seconds at normal speed', () => {
		expect(SPEEDS[NORMAL_SPEED]).toBe(1);
		expect(advance(2048, 10, 1)).toBe(2048);
		expect(advance(2048, 10, 0.125)).toBe(256);
	});
});

describe('modal motion', () => {
	/** Two modes over two nodes, three samples: node 3 and node 5 of the mesh. */
	const modal: Motion = {
		label: 'Time',
		unit: 's',
		start: 0,
		step: 0.1,
		quantity: 'Length',
		value_unit: 'm',
		node: Uint32Array.from([3, 5]),
		direction: new Float32Array(0),
		// mode 1 moves node 3 in X and node 5 in Y; mode 2 moves node 3 in Z
		modes: Float32Array.from([1, 0, 0, 0, 2, 0, 0, 0, 4, 0, 0, 0]),
		values: Float32Array.from([1, 2, 3, 10, 20, 30]),
		peak: 1
	};

	it('counts each mode\'s samples', () => {
		expect(sampleCount(modal)).toBe(3);
	});

	it('sums the modes scaled by their values at the sample', () => {
		const out = new Float32Array(3 * 6).fill(9);
		writeFrame(modal, 1, movedNodes(modal), out);
		expect(Array.from(out.slice(9, 12))).toEqual([2, 0, 80]);
		expect(Array.from(out.slice(15, 18))).toEqual([0, 4, 0]);
		expect(out[0]).toBe(9);
	});
});
