/**
 * Bin edges for the mock's synthetic spectra, by the rule the services use: midway between
 * lines, the outer edges mirrored. The mock stands in for a service, so it computes them here.
 */
export function binEdges(centers: ArrayLike<number>): Float64Array {
	const n = centers.length;
	const e = new Float64Array(n + 1);
	if (n < 2) return e;
	e[0] = centers[0] - 0.5 * (centers[1] - centers[0]);
	for (let i = 1; i < n; i++) e[i] = 0.5 * (centers[i - 1] + centers[i]);
	e[n] = centers[n - 1] + 0.5 * (centers[n - 1] - centers[n - 2]);
	return e;
}
