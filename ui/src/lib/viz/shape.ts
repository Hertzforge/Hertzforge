/**
 * How far each node of a mode shape travels over a cycle.
 *
 * A node's offset at phase t is re cos(t) - im sin(t). Its squared distance is
 * (A+B)/2 + (A-B)/2 cos(2t) - (re.im) sin(2t), with A = re.re and B = im.im, so its largest
 * value is (A+B)/2 + hypot((A-B)/2, re.im): no phase sweep is needed. For a real shape this is
 * the length of each node's vector. The same closed form as the core's `ShapeDeflection`.
 */
export function nodeTravel(re: Float32Array, im: Float32Array | undefined, out: Float32Array): number {
	let largest = 0;
	for (let n = 0, i = 0; i < re.length; n++, i += 3) {
		const a = re[i] * re[i] + re[i + 1] * re[i + 1] + re[i + 2] * re[i + 2];
		let travel2 = a;
		if (im) {
			const b = im[i] * im[i] + im[i + 1] * im[i + 1] + im[i + 2] * im[i + 2];
			const cross = re[i] * im[i] + re[i + 1] * im[i + 1] + re[i + 2] * im[i + 2];
			travel2 = (a + b) / 2 + Math.hypot((a - b) / 2, cross);
		}
		const travel = Math.sqrt(travel2);
		out[n] = travel;
		if (travel > largest) largest = travel;
	}
	return largest;
}
