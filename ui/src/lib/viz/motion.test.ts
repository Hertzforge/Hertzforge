import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { decodeFrame } from '$lib/protocol/codec';
import type { Motion, Shape3DView } from '$lib/protocol/types';
import { advance, movedNodes, NORMAL_SPEED, sampleCount, SPEEDS, writeFrame } from './motion';

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
