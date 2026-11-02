/** Measured motion played back on a mesh (schema/PROTOCOL.md, `shape3d` `motion`). */
import type { Motion } from '$lib/protocol/types';

/** Seconds a record takes to play through at normal speed, as in Visual Dynamics. */
export const SECONDS_PER_RECORD = 10;
/** Playback speeds, slowest first; Visual Dynamics' list. */
export const SPEEDS = [0.125, 0.25, 0.5, 1, 2, 4, 8] as const;
export const NORMAL_SPEED = SPEEDS.indexOf(1);

/** The number of samples in each DOF's record. */
export function sampleCount(motion: Motion): number {
	return motion.node.length ? motion.values.length / motion.node.length : 0;
}

/** The nodes any DOF moves, each once, in ascending order. */
export function movedNodes(motion: Motion): Uint32Array {
	return Uint32Array.from(new Set(motion.node)).sort();
}

/**
 * Write sample `k`'s offsets into `out` (f32 [N, 3]): every moved node is cleared, then each DOF
 * adds its direction times its value. Nodes no DOF moves are not touched.
 */
export function writeFrame(motion: Motion, k: number, moved: Uint32Array, out: Float32Array): void {
	const samples = sampleCount(motion);
	for (const n of moved) out[3 * n] = out[3 * n + 1] = out[3 * n + 2] = 0;
	for (let d = 0; d < motion.node.length; d++) {
		const v = motion.values[d * samples + k];
		const n = 3 * motion.node[d];
		out[n] += motion.direction[3 * d] * v;
		out[n + 1] += motion.direction[3 * d + 1] * v;
		out[n + 2] += motion.direction[3 * d + 2] * v;
	}
}

/** Samples to advance over `dt` seconds, so a record of `samples` plays in SECONDS_PER_RECORD / speed. */
export function advance(samples: number, dt: number, speed: number): number {
	return (samples / SECONDS_PER_RECORD) * dt * speed;
}
