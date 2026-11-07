/** Measured motion played back on a mesh (schema/PROTOCOL.md, `shape3d` `motion`). */
import type { Motion } from '$lib/protocol/types';

/** Seconds a record takes to play through at normal speed, as in Visual Dynamics. */
export const SECONDS_PER_RECORD = 10;
/** Playback speeds, slowest first; Visual Dynamics' list. */
export const SPEEDS = [0.125, 0.25, 0.5, 1, 2, 4, 8] as const;
export const NORMAL_SPEED = SPEEDS.indexOf(1);

/** The number of samples in each DOF's record, or each mode's. */
export function sampleCount(motion: Motion): number {
	if (!motion.node.length) return 0;
	const rows = motion.modes ? motion.modes.length / (3 * motion.node.length) : motion.node.length;
	return motion.values.length / rows;
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
	if (motion.modes) {
		// the modes' shapes scaled by their values at this sample and summed
		const count = motion.node.length;
		const modes = motion.modes.length / (3 * count);
		for (let m = 0; m < modes; m++) {
			const q = motion.values[m * samples + k];
			const at = 3 * m * count;
			for (let d = 0; d < count; d++) {
				const n = 3 * motion.node[d];
				const from = at + 3 * d;
				out[n] += motion.modes[from] * q;
				out[n + 1] += motion.modes[from + 1] * q;
				out[n + 2] += motion.modes[from + 2] * q;
			}
		}
		return;
	}
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

/** The sample nearest time `t`, held to the record. */
export function sampleAt(motion: Motion, t: number): number {
	const k = motion.step > 0 ? Math.round((t - motion.start) / motion.step) : 0;
	return Math.min(Math.max(0, k), sampleCount(motion) - 1);
}
