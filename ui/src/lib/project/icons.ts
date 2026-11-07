/**
 * The project tree's icons, each in a 64-unit box. Shape
 * carries the meaning: every icon has to work at 16 px and without color vision. An object's icon
 * says what kind of object it is; a record's icon says what that record measures, the kinematic
 * three told apart by Newton's dots; a sub-item's icon is drawn in its parent's color, or gray when
 * the category is empty. Each drawer returns SVG markup for a `viewBox="0 0 64 64"`.
 */

/** The object colors, by the kind the summary gives. */
export const KIND_COLORS: Record<string, string> = {
	geometry: '#1f77b4',
	time_history: '#2ca02c',
	frf: '#ff7f0e',
	psd: '#9467bd',
	spectrum: '#17becf',
	specification: '#d62728',
	srs: '#00a878',
	shock_specification: '#d62728',
	transient_specification: '#d62728',
	sine_sweep_specification: '#d62728',
	sine_level: '#17becf',
	sine_level_set: '#17becf',
	sine_target: '#d62728',
	coherence: '#bcbd22',
	multiple_coherence: '#bcbd22',
	shape_set: '#d94f8a',
	channel_table: '#4bb3fd',
	report: '#d2c14e',
	photos: '#b07d62',
	matched_modes: '#3fb950',
	predictions: '#c9822b',
	run_log_extract: '#d2c14e',
	project: '#6e7681'
};

/** What a record is, by the dimension it carries. */
export const QUANTITY_COLORS: Record<string, string> = {
	length: '#1f77b4',
	strain: '#8c564b',
	pressure: '#7f7f7f',
	velocity: '#17becf',
	acceleration: '#9467bd',
	force: '#2ca02c',
	temperature: '#d62728',
	voltage: '#e8a33d',
	angle: '#1f77b4',
	angular_velocity: '#17becf',
	angular_acceleration: '#9467bd',
	moment: '#2ca02c',
	modal_length: '#1f77b4',
	modal_velocity: '#17becf',
	modal_acceleration: '#9467bd',
	modal_force: '#2ca02c'
};

/** A category with nothing in it, and a type with no drawing. */
export const EMPTY_COLOR = '#6a6a72';
export const DEFAULT_COLOR = '#7f7f7f';
/** The controls' gray: a solid's shape, a mate. */
export const CONTROL_COLOR = '#8b949e';
/** Amber: units not yet defined. */
export const BADGE_COLOR = '#e6a23c';

/** The order the tree lists kinds in; a specification sits beside the measurement it is for. */
export const KIND_ORDER = [
	'geometry',
	'photos',
	'channel_table',
	'time_history',
	'transient_specification',
	'spectrum',
	'sine_target',
	'sine_level',
	'psd',
	'specification',
	'srs',
	'shock_specification',
	'sine_sweep_specification',
	'sine_level_set',
	'frf',
	'coherence',
	'multiple_coherence',
	'shape_set',
	'matched_modes',
	'predictions',
	'run_log_extract',
	'report'
];

export function kindRank(kind: string): number {
	const at = KIND_ORDER.indexOf(kind);
	return at < 0 ? KIND_ORDER.length : at;
}

type Point = [number, number];

const pts = (points: Point[]) => points.map(([x, y]) => `${x},${y}`).join(' ');
const stroke = (color: string, width: number, alpha = 1) =>
	`stroke="${color}" stroke-width="${width}" stroke-linecap="round" stroke-linejoin="round" fill="none"${alpha < 1 ? ` stroke-opacity="${alpha}"` : ''}`;
const line = (a: Point, b: Point, color: string, width: number, alpha = 1) =>
	`<line x1="${a[0]}" y1="${a[1]}" x2="${b[0]}" y2="${b[1]}" ${stroke(color, width, alpha)}/>`;
const polyline = (points: Point[], color: string, width: number, alpha = 1, dashed = false) =>
	`<polyline points="${pts(points)}" ${stroke(color, width, alpha)}${dashed ? ' stroke-dasharray="9 7"' : ''}/>`;
const polygon = (points: Point[], color: string, width: number, fillAlpha: number | null = null) =>
	`<polygon points="${pts(points)}" ${width ? stroke(color, width) : 'stroke="none"'}${fillAlpha === null ? '' : ` fill="${color}" fill-opacity="${fillAlpha}"`}/>`.replace(
		'fill="none" fill=',
		'fill='
	);
const dot = (c: Point, r: number, color: string, alpha = 1) =>
	`<circle cx="${c[0]}" cy="${c[1]}" r="${r}" fill="${color}"${alpha < 1 ? ` fill-opacity="${alpha}"` : ''}/>`;
const rect = (x: number, y: number, w: number, h: number, color: string, width: number, fillAlpha: number | null, r = 0) =>
	`<rect x="${x}" y="${y}" width="${w}" height="${h}"${r ? ` rx="${r}"` : ''} ${width ? stroke(color, width).replace(' fill="none"', '') : 'stroke="none"'} fill="${fillAlpha === null ? 'none' : color}"${fillAlpha !== null && fillAlpha < 1 ? ` fill-opacity="${fillAlpha}"` : ''}/>`;
const ellipse = (cx: number, cy: number, rx: number, ry: number, color: string, width: number, fillAlpha: number | null) =>
	`<ellipse cx="${cx}" cy="${cy}" rx="${rx}" ry="${ry}" ${width ? stroke(color, width).replace(' fill="none"', '') : 'stroke="none"'} fill="${fillAlpha === null ? 'none' : color}"${fillAlpha !== null && fillAlpha < 1 ? ` fill-opacity="${fillAlpha}"` : ''}/>`;
/** The lower half of an ellipse, as Qt's drawArc from 180° through 180° draws it. */
const lowerArc = (cx: number, cy: number, rx: number, ry: number, color: string, width: number) =>
	`<path d="M${cx - rx},${cy} A${rx},${ry} 0 0 0 ${cx + rx},${cy}" ${stroke(color, width)}/>`;
const upperArc = (cx: number, cy: number, rx: number, ry: number, color: string, width: number) =>
	`<path d="M${cx - rx},${cy} A${rx},${ry} 0 0 1 ${cx + rx},${cy}" ${stroke(color, width)}/>`;

const RINGDOWN: Point[] = [[8, 32], [17, 32], [21, 12], [26, 48], [31, 25], [36, 38], [41, 29], [46, 34], [51, 32], [56, 32]];
const PSD_SPINE: Point[] = [[9, 46], [17, 39], [23, 25], [31, 35], [39, 19], [47, 33], [55, 42]];
const TOLERANCE = 9;
const TOLERANCE_ALPHA = 120 / 255;
const FRAME_TOP: Point[] = [[10, 22], [40, 22], [54, 12], [24, 12]];
const FRAME_FEET: Point[] = [[10, 54], [40, 54], [54, 44]];

const shifted = (points: Point[], dy: number): Point[] => points.map(([x, y]) => [x, y + dy]);
const tolerance = (spine: Point[], color: string) =>
	polyline(shifted(spine, -TOLERANCE), color, 4, TOLERANCE_ALPHA) + polyline(shifted(spine, TOLERANCE), color, 4, TOLERANCE_ALPHA);
const sine = (count: number, top: number, amplitude: number, cycles: number): Point[] =>
	Array.from({ length: count }, (_, i) => [10 + i, top - amplitude * Math.sin((i / 44) * cycles * Math.PI)]);
const round = (points: Point[]): Point[] => points.map(([x, y]) => [Math.round(x * 100) / 100, Math.round(y * 100) / 100]);

function psdBody(color: string): string {
	const area = [...PSD_SPINE, [55, 55] as Point, [9, 55] as Point];
	return polygon(area, color, 0, 90 / 255) + polyline(PSD_SPINE, color, 5);
}

function chirp(): Point[] {
	const points: Point[] = [];
	for (let i = 0; i < 33; i++) {
		const x = 8 + (48 * i) / 32;
		const phase = 2.6 * Math.PI * (i / 32) ** 2 * 3;
		points.push([x, 32 - 16 * Math.sin(phase)]);
	}
	return round(points);
}

function elementShape(points: Point[], color: string): string {
	const outline = points.length > 2 ? polygon(points, color, 7) : polyline(points, color, 7);
	return outline + points.map((p) => dot(p, 7, color)).join('');
}

const depth = (color: string, ...edges: [Point, Point][]) => edges.map(([a, b]) => line(a, b, color, 5)).join('');

/** A ring segment with an arrowhead at its end, clockwise on screen from `start` degrees through `sweep`, as Qt's drawArc counts them. */
function arcArrow(color: string, center: Point, radius: number, start: number, sweep: number, width = 6): string {
	const [cx, cy] = center;
	const rad = (deg: number) => (deg * Math.PI) / 180;
	const at = (deg: number): Point => [cx + radius * Math.cos(rad(deg)), cy - radius * Math.sin(rad(deg))];
	const end = start - sweep;
	const [sx, sy] = at(start);
	const [px, py] = at(end);
	const [tx, ty] = [Math.sin(rad(end)), Math.cos(rad(end))];
	const [nx, ny] = [ty, -tx];
	const head: Point[] = [
		[px + 10 * tx, py + 10 * ty],
		[px - 3 * tx + 7 * nx, py - 3 * ty + 7 * ny],
		[px - 3 * tx - 7 * nx, py - 3 * ty - 7 * ny]
	];
	const arc = `<path d="M${sx.toFixed(2)},${sy.toFixed(2)} A${radius},${radius} 0 ${sweep > 180 ? 1 : 0} 1 ${px.toFixed(2)},${py.toFixed(2)}" ${stroke(color, width)}/>`;
	return arc + polygon(round(head), color, 0, 1);
}

function dots(color: string, count: number): string {
	const spacing = 19;
	let out = '';
	for (let i = 0; i < count; i++) out += dot([32 + (i - (count - 1) / 2) * spacing, 13], 7, color);
	return out;
}

const trace = (color: string, top: number, amplitude: number) => polyline(round(sine(45, top, amplitude, 2)), color, 6);

function modal(draw: (color: string) => string): (color: string) => string {
	return (color) =>
		`<g transform="scale(0.75)">${draw(color)}</g><text x="51" y="59" font-size="26" font-weight="bold" font-family="system-ui, sans-serif" text-anchor="middle" fill="${color}">M</text>`;
}

let clipSeq = 0;
const twoCircles = { left: [23, 31, 17] as const, right: [41, 31, 17] as const };

/** The object kinds' drawings, by icon key. */
export const OBJECT_DRAW: Record<string, (color: string) => string> = {
	geometry: (c) => {
		let out = '';
		for (let k = 0; k < 4; k++) out += line(FRAME_TOP[k], FRAME_TOP[(k + 1) % 4], c, 5);
		for (let k = 0; k < 3; k++) out += line(FRAME_TOP[k], FRAME_FEET[k], c, 5);
		return out + [...FRAME_TOP, ...FRAME_FEET].map((p) => dot(p, 5.5, c)).join('');
	},
	time_history: (c) => line([8, 32], [56, 32], c, 3) + polyline(RINGDOWN, c, 6),
	frf: (c) => polyline([[8, 47], [18, 45], [25, 41], [29, 15], [33, 41], [39, 43], [45, 31], [49, 43], [56, 45]], c, 6),
	psd: psdBody,
	octave_psd: (c) => {
		let left = 9;
		let out = '';
		for (const [width, top] of [[5.8, 17], [8.7, 30], [13, 40], [19.5, 24]] as const) {
			out += rect(left, 52 - top, width - 1.2, top, c, 0, 1);
			left += width;
		}
		return out;
	},
	spectrum: (c) =>
		([[13, 20], [23, 36], [33, 27], [43, 45], [53, 15]] as const).map(([x, h]) => rect(x - 4, 52 - h, 8, h, c, 0, 1, 2)).join(''),
	specification: (c) => tolerance(PSD_SPINE, c) + psdBody(c),
	octave_specification: (c) => OBJECT_DRAW.octave_psd(c),
	srs: (c) => polyline([[10, 52], [20, 44], [28, 26], [36, 18], [46, 20], [55, 20]], c, 6),
	shock_specification: (c) => {
		const spine: Point[] = [[10, 46], [20, 38], [28, 22], [36, 15], [46, 17], [55, 17]];
		return tolerance(spine, c) + polyline(spine, c, 6);
	},
	sine_sweep_specification: (c) => polyline(chirp(), c, 6),
	sine_level: (c) => OBJECT_DRAW.sine_sweep_specification(c),
	sine_level_set: (c) => OBJECT_DRAW.sine_sweep_specification(c),
	sine_target: (c) => OBJECT_DRAW.sine_sweep_specification(c),
	transient_specification: (c) => tolerance(RINGDOWN, c) + line([8, 32], [56, 32], c, 3) + polyline(RINGDOWN, c, 6),
	coherence: (c) =>
		line([8, 15], [56, 15], c, 4, 130 / 255) +
		polyline([[8, 24], [16, 23], [22, 25], [27, 38], [32, 50], [37, 38], [42, 25], [48, 23], [56, 24]], c, 6),
	multiple_coherence: (c) =>
		line([8, 13], [56, 13], c, 4, 130 / 255) +
		line([14, 46], [50, 46], c, 4, 90 / 255) +
		line([14, 53], [50, 53], c, 4, 90 / 255) +
		polyline([[8, 22], [18, 21], [25, 24], [32, 36], [39, 24], [46, 21], [56, 22]], c, 6),
	channel_table: (c) =>
		rect(11, 15, 42, 10, c, 0, 110 / 255) +
		rect(11, 15, 42, 34, c, 4, null, 3) +
		line([11, 25], [53, 25], c, 4) +
		line([11, 37], [53, 37], c, 4) +
		line([32, 15], [32, 49], c, 4),
	report: (c) =>
		rect(14, 8, 36, 48, c, 4, 60 / 255, 4) +
		line([21, 18], [43, 18], c, 3) +
		line([21, 25], [43, 25], c, 3) +
		polyline([[21, 46], [27, 38], [33, 42], [39, 32], [43, 35]], c, 4),
	photos: (c) =>
		rect(8, 12, 48, 40, c, 5, null, 6) + dot([21, 25], 5.5, c) + polygon([[13, 47], [28, 30], [37, 40], [45, 28], [51, 47]], c, 0, 1),
	shape_set: (c) => {
		const points = round(sine(45, 32, 16, 2));
		return polyline(points, c, 5) + [0, 11, 22, 33, 44].map((i) => dot(points[i], 5, c)).join('');
	},
	matched_modes: (c) => {
		let out = '';
		for (let i = 0; i < 3; i++) for (let j = 0; j < 3; j++) out += rect(12 + j * 14, 12 + i * 14, 11, 11, c, 0, 70 / 255);
		return out + rect(12, 12, 11, 11, c, 0, 1) + rect(40, 40, 11, 11, c, 0, 1);
	},
	predictions: (c) =>
		polyline([[8, 46], [20, 42], [27, 34], [32, 12], [37, 34], [44, 42], [56, 46]], c, 5, 1, true) +
		line([20, 55], [44, 55], c, 4) +
		line([20, 50], [20, 58], c, 4) +
		line([44, 50], [44, 58], c, 4),
	run_log_extract: (c) => OBJECT_DRAW.report(c),
	project: (c) => rect(12, 40, 40, 14, c, 4, 1, 3) + rect(12, 28, 40, 14, c, 4, 90 / 255, 3) + rect(12, 16, 40, 14, c, 4, 90 / 255, 3)
};

/** What a record measures: the kinematic three share a trace and are told apart by Newton's dots. */
export const QUANTITY_DRAW: Record<string, (color: string) => string> = {
	length: (c) => trace(c, 34, 15),
	velocity: (c) => trace(c, 42, 11) + dots(c, 1),
	acceleration: (c) => trace(c, 42, 11) + dots(c, 2),
	force: (c) => line([32, 8], [32, 38], c, 7) + polygon([[32, 48], [22, 32], [42, 32]], c, 0, 1) + line([12, 55], [52, 55], c, 6, 150 / 255),
	temperature: (c) =>
		line([26, 14], [26, 40], c, 6) + dot([26, 47], 10, c) + [18, 27, 36].map((y) => line([38, y], [50, y], c, 5, 150 / 255)).join(''),
	voltage: (c) => polygon([[38, 8], [18, 34], [29, 34], [24, 56], [46, 28], [34, 28], [40, 8]], c, 0, 1),
	strain: (c) =>
		line([22, 32], [42, 32], c, 7) +
		polygon([[6, 32], [20, 24], [20, 40]], c, 0, 1) +
		polygon([[58, 32], [44, 24], [44, 40]], c, 0, 1) +
		line([22, 18], [22, 46], c, 5, 150 / 255) +
		line([42, 18], [42, 46], c, 5, 150 / 255),
	pressure: (c) =>
		dot([32, 32], 22, 'none').replace('fill="none"', `fill="none" ${stroke(c, 5)}`) +
		line([32, 32], [46, 20], c, 6) +
		dot([32, 32], 5, c) +
		([[14, 22], [32, 8], [50, 22]] as Point[]).map((p) => dot(p, 2.5, c, 150 / 255)).join(''),
	angle: (c) => arcArrow(c, [32, 34], 17, 200, 290),
	angular_velocity: (c) => arcArrow(c, [32, 40], 14, 200, 290) + dots(c, 1),
	angular_acceleration: (c) => arcArrow(c, [32, 40], 14, 200, 290) + dots(c, 2),
	moment: (c) => line([32, 6], [32, 58], c, 6, 150 / 255) + arcArrow(c, [32, 32], 17, 150, 250, 7),
	modal_length: modal((c) => trace(c, 34, 15)),
	modal_velocity: modal((c) => trace(c, 42, 11) + dots(c, 1)),
	modal_acceleration: modal((c) => trace(c, 42, 11) + dots(c, 2)),
	modal_force: modal((c) => QUANTITY_DRAW.force(c))
};

/** A sub-item's drawing, in its parent's color. */
export const CHILD_DRAW: Record<string, (color: string) => string> = {
	nodes: (c) => ([[18, 22], [44, 18], [32, 46]] as Point[]).map((p) => dot(p, 7, c)).join(''),
	coordinate_systems: (c) =>
		line([20, 46], [46, 46], c, 5) + line([20, 46], [20, 20], c, 5) + line([20, 46], [36, 32], c, 5) + dot([20, 46], 5, c),
	elements: (c) => rect(13, 16, 38, 32, c, 5, null) + line([32, 16], [32, 48], c, 5) + line([13, 32], [51, 32], c, 5),
	groups: (c) => rect(11, 18, 18, 28, c, 5, 1) + rect(35, 18, 18, 28, c, 5, null) + line([35, 32], [53, 32], c, 5),
	beams: (c) => elementShape([[12, 50], [52, 14]], c),
	triangles: (c) => elementShape([[32, 10], [54, 52], [10, 52]], c),
	quads: (c) => elementShape([[11, 11], [53, 11], [53, 53], [11, 53]], c),
	tetras: (c) => elementShape([[32, 10], [54, 50], [10, 50]], c) + depth(c, [[32, 10], [34, 36]], [[54, 50], [34, 36]], [[10, 50], [34, 36]]),
	wedges: (c) => elementShape([[24, 24], [40, 54], [8, 54]], c) + depth(c, [[24, 24], [38, 10]], [[40, 54], [54, 40]], [[38, 10], [54, 40]]),
	hexes: (c) =>
		elementShape([[10, 22], [42, 22], [42, 54], [10, 54]], c) +
		depth(c, [[10, 22], [22, 10]], [[42, 22], [54, 10]], [[42, 54], [54, 42]], [[22, 10], [54, 10]], [[54, 10], [54, 42]]),
	points: (c) => CHILD_DRAW.nodes(c),
	pyramids: (c) =>
		elementShape([[10, 50], [42, 50], [54, 40], [22, 40]], c) +
		depth(c, [[10, 50], [32, 10]], [[42, 50], [32, 10]], [[54, 40], [32, 10]], [[22, 40], [32, 10]]),
	mode: (c) => polyline(round(sine(45, 32, 14, 1)), c, 6),
	record: (c) => polyline(round(sine(45, 32, 14, 2)), c, 6),
	channel: (c) => rect(11, 26, 42, 12, c, 0, 120 / 255) + line([11, 18], [53, 18], c, 4) + line([11, 46], [53, 46], c, 4),
	mates: (c) => {
		let out = rect(6, 8, 52, 14, c, 5, null) + rect(6, 42, 52, 14, c, 5, null);
		for (const x of [16, 32, 48]) out += polyline([[x, 22], [x - 5, 27], [x + 5, 32], [x - 5, 37], [x, 42]], c, 5);
		return out;
	},
	solid_block: (c) =>
		polygon([[8, 24], [40, 24], [40, 56], [8, 56]], c, 5, 90 / 255) +
		polygon([[8, 24], [24, 10], [56, 10], [40, 24]], c, 5, 40 / 255) +
		polygon([[40, 24], [56, 10], [56, 42], [40, 56]], c, 5, 150 / 255),
	solid_cylinder: (c) =>
		rect(14, 14, 36, 36, c, 5, 90 / 255) + ellipse(32, 50, 18, 8, c, 0, 90 / 255) + lowerArc(32, 50, 18, 8, c, 5) + ellipse(32, 14, 18, 8, c, 5, 40 / 255),
	solid_cone: (c) =>
		polygon([[10, 50], [24, 12], [40, 12], [54, 50]], c, 5, 90 / 255) +
		ellipse(32, 50, 22, 8, c, 0, 90 / 255) +
		lowerArc(32, 50, 22, 8, c, 5) +
		ellipse(32, 12, 8, 4, c, 5, 40 / 255),
	solid_sphere: (c) => ellipse(32, 32, 24, 24, c, 5, 90 / 255) + lowerArc(32, 32, 24, 8, c, 5) + upperArc(32, 32, 24, 8, c, 2),
	solid_plate: (c) => polygon([[6, 46], [22, 18], [58, 18], [42, 46]], c, 5, 90 / 255),
	solid_disk: (c) => ellipse(32, 32, 26, 14, c, 5, 90 / 255),
	solid_section: (c) =>
		polygon([[10, 8], [54, 8], [54, 18], [37, 18], [37, 46], [54, 46], [54, 56], [10, 56], [10, 46], [27, 46], [27, 18], [10, 18]], c, 4, 110 / 255),
	solid_hole: (c) =>
		polygon([[4, 12], [18, 12], [18, 26], [26, 26], [26, 56], [4, 56]], c, 4, 110 / 255) +
		polygon([[60, 12], [46, 12], [46, 26], [38, 26], [38, 56], [60, 56]], c, 4, 110 / 255),
	solid_union: (c) => {
		const [lx, ly, r] = twoCircles.left;
		const [rx, ry] = twoCircles.right;
		return `<path d="M${lx},${ly - r} A${r},${r} 0 1 0 ${lx},${ly + r} A${r},${r} 0 0 0 ${rx},${ry + r} A${r},${r} 0 1 0 ${rx},${ry - r} A${r},${r} 0 0 0 ${lx},${ly - r} Z" ${stroke(c, 4).replace(' fill="none"', '')} fill="${c}" fill-opacity="${150 / 255}"/>`;
	},
	solid_subtract: (c) => {
		const id = `bite${++clipSeq}`;
		const [lx, ly, r] = twoCircles.left;
		const [rx, ry] = twoCircles.right;
		return (
			`<defs><mask id="${id}"><rect width="64" height="64" fill="white"/><circle cx="${rx}" cy="${ry}" r="${r}" fill="black"/></mask></defs>` +
			`<circle cx="${lx}" cy="${ly}" r="${r}" mask="url(#${id})" ${stroke(c, 4).replace(' fill="none"', '')} fill="${c}" fill-opacity="${150 / 255}"/>` +
			`<circle cx="${rx}" cy="${ry}" r="${r}" ${stroke(c, 2)}/>`
		);
	},
	solid_intersect: (c) => {
		const id = `lens${++clipSeq}`;
		const [lx, ly, r] = twoCircles.left;
		const [rx, ry] = twoCircles.right;
		return (
			`<defs><clipPath id="${id}"><circle cx="${lx}" cy="${ly}" r="${r}"/></clipPath></defs>` +
			`<circle cx="${rx}" cy="${ry}" r="${r}" clip-path="url(#${id})" fill="${c}" fill-opacity="${170 / 255}"/>` +
			`<circle cx="${lx}" cy="${ly}" r="${r}" ${stroke(c, 3)}/><circle cx="${rx}" cy="${ry}" r="${r}" ${stroke(c, 3)}/>`
		);
	},
	solid_pattern: (c) => {
		let out = '';
		[20, 44].forEach((y, row) =>
			[14, 32, 50].forEach((x, col) => {
				out += `<circle cx="${x}" cy="${y}" r="7" ${stroke(c, 3).replace(' fill="none"', '')} fill="${c}" fill-opacity="${(row === 0 && col === 0 ? 200 : 70) / 255}"/>`;
			})
		);
		return out;
	}
};

/** The icon a solid wears, by its kind: a section of any shape wears the I-beam. */
export function solidIcon(kind: string): string {
	return kind in CHILD_DRAW && kind.startsWith('solid_') ? kind : `solid_${kind}` in CHILD_DRAW ? `solid_${kind}` : 'solid_section';
}

const defaultDraw = (c: string) => dot([32, 32], 16, c);

/** The units badge: a small amber '?' in the corner. */
export const BADGE = `<circle cx="49" cy="49" r="14" fill="${BADGE_COLOR}" stroke="#ffffff" stroke-width="3"/><text x="49" y="57" font-size="22" font-weight="bold" font-family="system-ui, sans-serif" text-anchor="middle" fill="#ffffff">?</text>`;

/** An object's icon key: its kind, or the banded variant of a PSD or specification. */
export function objectIcon(kind: string, banded = false): string {
	if (banded && kind === 'psd') return 'octave_psd';
	if (banded && kind === 'specification') return 'octave_specification';
	return kind;
}

/** The markup of an object kind's icon, in its own color. */
export function drawObject(key: string, badge = false, color?: string): string {
	const draw = OBJECT_DRAW[key] ?? defaultDraw;
	const base = key === 'octave_psd' ? 'psd' : key === 'octave_specification' ? 'specification' : key;
	return draw(color ?? KIND_COLORS[base] ?? DEFAULT_COLOR) + (badge ? BADGE : '');
}

/** The markup of a record's icon: a quantity, two as a fraction (response upper-left over reference lower-right), or the object's own. */
export function drawRecord(key: string, objectKey: string, badge = false): string {
	if (!key) return drawObject(objectKey, badge);
	const [response, reference] = key.split('/');
	if (reference) {
		const scale = 0.58;
		const shift = 64 * (1 - scale);
		return (
			`<g transform="scale(${scale})">${QUANTITY_DRAW[response](QUANTITY_COLORS[response])}</g>` +
			`<g transform="translate(${shift} ${shift}) scale(${scale})">${QUANTITY_DRAW[reference](QUANTITY_COLORS[reference])}</g>` +
			line([19.2, 58.9], [44.8, 5.1], '#8e8e93', 3) +
			(badge ? BADGE : '')
		);
	}
	const draw = QUANTITY_DRAW[response];
	return draw ? draw(QUANTITY_COLORS[response]) + (badge ? BADGE : '') : drawObject(objectKey, badge);
}

/** The markup of a sub-item's icon, in its parent kind's color, gray when the category is empty. */
export function drawChild(key: string, parentKind: string, empty = false, badge = false): string {
	const draw = CHILD_DRAW[key] ?? defaultDraw;
	const color = key.startsWith('solid_') || key === 'mates' ? CONTROL_COLOR : empty ? EMPTY_COLOR : (KIND_COLORS[parentKind] ?? DEFAULT_COLOR);
	return draw(color) + (badge ? BADGE : '');
}
