<script lang="ts" module>
	/** Where the camera stood in each result's view that takes clicks: a click replaces the view, and the view belongs to the person, so the new one opens where they left it. */
	const poses = new Map<string, { position: number[]; target: number[]; up: number[] }>();
</script>

<script lang="ts">
	import { copyCanvas, plotBackground } from './copyImage';
	/**
	 * Mode shape view, drawn with three.js.
	 *
	 * Deformation x0 + a * Re((shape + i shape_imag) exp(i theta)) runs in the vertex shader, so a
	 * frame costs two uniform updates however large the mesh is. A real shape is a standing wave; a
	 * complex one (a complex mode, an operating shape) shows its traveling part. Measured motion
	 * (`view.motion`, with no modes) is played back sample by sample instead: a frame writes the
	 * moved nodes' offsets and colors, and nothing else. A view with `overlays` draws one of them
	 * over the mode in flat orange, aligned by `overlay_phase` and scaled to the mode's amplitude,
	 * through the same shader and uniforms, so the two move together; this is a MAC pair. Normals come from screen-space derivatives (flat
	 * shading), which keeps lighting correct on the deformed surface. Color is the mode shape
	 * node's largest travel over a cycle on a colorblind-safe sequential map with a numeric legend. GPU resources are
	 * released on unmount and the WebGL context is dropped explicitly (WebKit caps live contexts).
	 *
	 * A view with `pick` takes clicks, as Visual Dynamics' Add mode and planning pane do: a press and
	 * release in one place places a point, picks a node, or leaves a surface out or puts it back, and
	 * a drag still turns the view.
	 */
	import { onDestroy, onMount } from 'svelte';
	import {
		AmbientLight,
		BufferAttribute,
		BufferGeometry,
		Color,
		DirectionalLight,
		DoubleSide,
		LineBasicMaterial,
		Box3,
		Box3Helper,
		LineSegments,
		Mesh,
		MeshBasicMaterial,
		MeshLambertMaterial,
		PerspectiveCamera,
		Points,
		PointsMaterial,
		Quaternion,
		Raycaster,
		Scene,
		Vector2,
		Vector3,
		WebGLRenderer,
		type Material
	} from 'three';
	import { OrbitControls } from 'three/addons/controls/OrbitControls.js';
	import type { Shape3DView } from '$lib/protocol/types';
	import { cssVar, theme } from '$lib/theme/theme.svelte';
	import { along, labeledEdges, niceTicks, type Point } from './axes';
	import { colormap, colormapGradient } from './colormap';
	import { coincidentGroups, nodeCaption } from './coincident';
	import { playheadGroup, playheads } from './cursor';
	import { advance, movedNodes, NORMAL_SPEED, sampleAt, sampleCount, SPEEDS, writeFrame } from './motion';
	import { alignedShape, nodeTravel } from './shape';
	import { idList, modelPlane, nearestNode, placedPoint, toggledSurface } from './pick';
	import { formatCount, formatFixed, formatNumber } from './format';

	interface Props {
		view: Shape3DView;
		/** Index into `view.modes`. */
		mode?: number;
		/** Index into `view.overlays`, drawn over the mode; none when null. */
		overlay?: number | null;
		/** The result the view belongs to, which its playback `link` is scoped to. */
		scope?: string;
		/** Run the view's `pick` operation with these params added: what a click chose. */
		onpick?: (patch: Record<string, unknown>) => void;
	}

	let { view, mode = $bindable(0), overlay = null, scope, onpick }: Props = $props();

	/** The click the view takes, when it takes one and there is somewhere to send it. */
	const pick = $derived(onpick ? view.pick : undefined);
	/** Nodes picked so far toward one element, as rows of `view.nodes`. */
	let picked = $state<number[]>([]);
	/** The pickable node under the cursor. */
	let hoverNode = $state<number | null>(null);
	/** How far a press may move and still be a click, in CSS pixels. */
	const CLICK_SLOP = 4;

	const paneId = `s3-${Math.random().toString(36).slice(2, 9)}`;
	/** The playhead group playback is shared through, when the view links one. */
	const playhead = $derived(view.motion && view.link ? playheadGroup(view.link, scope) : null);

	let copiedImage = $state<'copied' | 'saved' | null>(null);
	/** The view as it stands, as an image on the clipboard: drawn and copied in one task. */
	async function copyImage() {
		const drawer = renderer;
		if (!drawer) return;
		copiedImage = await copyCanvas(drawer.domElement, plotBackground(drawer.domElement), () => drawer.render(scene, camera), view.title);
		if (copiedImage) setTimeout(() => (copiedImage = null), 1500);
	}

	/** The gray a geometry is drawn in when nothing moves, as Visual Dynamics draws one undeformed. */
	const REST_GRAY = 0.62;

	/** Okabe-Ito orange: apart from every viridis color, and for every color vision. */
	const OVERLAY_COLOR = 0xe69f00;

	/** Seconds per animation cycle. Visual only; not the mode's period. */
	const CYCLE_S = 1.6;
	/** An amplitude of 1, as a fraction of the model's diagonal: the slider's default position, and its scale. */
	const AMP_UNIT = 0.075;
	/** The most nodes the hover caption projects on every pointer move. */
	const HOVER_LIMIT = 60_000;

	/** How long Record records: at least six seconds of whole cycles of a mode, as VD saves an animation, or that long of a motion. */
	const RECORD_S = Math.ceil(6 / CYCLE_S) * CYCLE_S;
	let recording = $state(false);

	/**
	 * The animation recorded from the view as it plays and saved as a video, as Visual Dynamics saves
	 * an animation: MP4 where the web view can write it (Safari's can), WebM otherwise. The
	 * background is painted in for the recording, since the view is drawn on a clear one.
	 */
	async function record() {
		const drawer = renderer;
		if (!drawer || recording || typeof MediaRecorder === 'undefined') return;
		const type = ['video/mp4;codecs=avc1', 'video/mp4', 'video/webm;codecs=vp9', 'video/webm'].find((t) => MediaRecorder.isTypeSupported(t));
		if (!type) return;
		recording = true;
		playing = true;
		const before = drawer.getClearAlpha();
		drawer.setClearColor(plotBackground(drawer.domElement), 1);
		const stream = drawer.domElement.captureStream(30);
		const recorder = new MediaRecorder(stream, { mimeType: type, videoBitsPerSecond: 8_000_000 });
		const pieces: Blob[] = [];
		recorder.ondataavailable = (e) => e.data.size && pieces.push(e.data);
		const done = new Promise<void>((resolve) => (recorder.onstop = () => resolve()));
		recorder.start();
		await new Promise((r) => setTimeout(r, RECORD_S * 1000));
		recorder.stop();
		await done;
		stream.getTracks().forEach((t) => t.stop());
		drawer.setClearAlpha(before);
		recording = false;
		const link = document.createElement('a');
		link.href = URL.createObjectURL(new Blob(pieces, { type }));
		link.download = `${view.title.replace(/[^\w .-]+/g, '_').slice(0, 80) || 'animation'}.${type.startsWith('video/mp4') ? 'mp4' : 'webm'}`;
		link.click();
		setTimeout(() => URL.revokeObjectURL(link.href), 5000);
	}

	let host: HTMLDivElement;
	let canvasHost: HTMLDivElement;
	let triad: SVGSVGElement;
	/** The labeled axes over the canvas: ticks, values and a titled edge per axis, redrawn each frame the view moves. */
	let axesSvg: SVGSVGElement;

	let playing = $state(true);
	/** The deflection amplitude, a multiple of `AMP_UNIT`: the slider runs from 0 to `amplitudeMax`, which the field beside it sets. */
	let amplitude = $state(1);
	let amplitudeMax = $state(4);
	let wireframe = $state(false);
	/** The surface's opacity, 1 for solid: below it the surface is drawn translucent without writing depth, as the ghost is, so a brick model's inner faces show through without draw-order speckle. */
	let opacity = $state(1);
	let ghost = $state(false);
	/** Color by deflection, or one color for the surface (VD's colormap toggle). */
	let colored = $state(true);
	/** The labeled axes about the model (VD's Plot axes): a box at the model's bounds, one edge per axis ticked with values and titled with the unit. */
	let bounds = $state(false);
	/** The node under the pointer, captioned when the view is not picking. */
	let hovered = $state<number | null>(null);
	let hoverPending = false;
	/** Nodes at one place, for the caption; built once per view's nodes and only when first asked. */
	let groupsFor: { nodes: Float32Array; groups: Map<number, number[]> } | null = null;
	function caption(n: number): string {
		if (!groupsFor || groupsFor.nodes !== view.nodes) groupsFor = { nodes: view.nodes, groups: coincidentGroups(view.nodes) };
		return nodeCaption(n, view.node_ids, groupsFor.groups);
	}
	/** The slider's top, typed: 0.1 to 1000; the slider is pulled down to it when it was above. */
	function setAmplitudeMax(text: string) {
		const value = Number(text);
		if (!Number.isFinite(value)) return;
		amplitudeMax = Math.min(1000, Math.max(0.1, value));
		if (amplitude > amplitudeMax) amplitude = amplitudeMax;
	}
	let fps = $state<number | null>(null);
	let failure = $state<string | null>(null);

	const nodeCount = $derived(view.nodes.length / 3);
	const triCount = $derived((view.triangles?.length ?? 0) / 3);
	const current = $derived(view.modes[Math.min(Math.max(0, mode), view.modes.length - 1)]);
	const motion = $derived(view.motion);
	/** Whether anything moves: a geometry on its own has no play, amplitude, ghost or legend to offer. */
	const moves = $derived(view.modes.length > 0 || !!motion);
	const shown = $derived(overlay !== null && view.overlays ? view.overlays[overlay] : undefined);
	const samples = $derived(motion ? sampleCount(motion) : 0);
	let speedIndex = $state(NORMAL_SPEED);
	/** The sample shown, for the scrubber and the time readout. */
	let sample = $state(0);

	/** Model diagonal, from the undeformed nodes. */
	const diagonal = $derived.by(() => {
		const n = view.nodes;
		const lo = [Infinity, Infinity, Infinity];
		const hi = [-Infinity, -Infinity, -Infinity];
		for (let i = 0; i < n.length; i += 3) {
			for (let k = 0; k < 3; k++) {
				if (n[i + k] < lo[k]) lo[k] = n[i + k];
				if (n[i + k] > hi[k]) hi[k] = n[i + k];
			}
		}
		return Math.hypot(hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]) || 1;
	});

	const peak = $derived(amplitude * AMP_UNIT * diagonal);

	/** The model's extents along each axis, for the bounds box and its labels. */
	const extents = $derived.by(() => {
		const n = view.nodes;
		const lo = [Infinity, Infinity, Infinity];
		const hi = [-Infinity, -Infinity, -Infinity];
		for (let i = 0; i < n.length; i += 3) {
			for (let k = 0; k < 3; k++) {
				if (n[i + k] < lo[k]) lo[k] = n[i + k];
				if (n[i + k] > hi[k]) hi[k] = n[i + k];
			}
		}
		return { lo, hi };
	});

	/** Display scaling of the model unit: meters shown as millimeters below one meter. */
	const display = $derived.by(() => {
		if (view.unit === 'm' && peak < 1) return { factor: 1000, unit: 'mm' };
		return { factor: 1, unit: view.unit };
	});

	// three.js objects (not reactive)
	let renderer: WebGLRenderer | null = null;
	let scene: Scene;
	let boundsBox: Box3Helper | null = null;
	let camera: PerspectiveCamera;
	let controls: OrbitControls;
	let geometry: BufferGeometry;
	let lineGeometry: BufferGeometry | null = null;
	let shapeAttr: BufferAttribute;
	let shapeImagAttr: BufferAttribute;
	let colorAttr: BufferAttribute;
	let surface: Mesh;
	let outline: LineSegments | null = null;
	let ghostSurface: Mesh;
	let ghostOutline: LineSegments | null = null;
	/** The view's `marks`: DOF arrows and sensors, drawn still. */
	let markLines: LineSegments | null = null;
	let overlayGeometry: BufferGeometry | null = null;
	let overlayLineGeometry: BufferGeometry | null = null;
	let overlayAttr: BufferAttribute;
	let overlayImagAttr: BufferAttribute;
	let overlaySurface: Mesh | null = null;
	let overlayOutline: LineSegments | null = null;
	/** The surfaces not in use, drawn muted over the model, when clicks leave surfaces out. */
	let mutedSurface: Mesh | null = null;
	/** The nodes picked and the one under the cursor. */
	let marks: Points | null = null;
	/** Every node as a dot, while clicks place points or pick nodes, so a node with no element still shows. */
	let dots: Points | null = null;
	let leftOut: Points | null = null;
	let markGeometry: BufferGeometry | null = null;
	const materials: Material[] = [];
	const uniforms = { uRe: { value: 0 }, uIm: { value: 0 } };
	let maxMag = 1;
	let raf = 0;
	let phase = 0;
	let lastTime = 0;
	let dirty = true;
	let frames = 0;
	let fpsWindowStart = 0;
	let resizeObs: ResizeObserver | null = null;
	let homeDistance = 1;
	/** Playback position in samples, fractional so a slow speed still advances. */
	let position = 0;
	let shownSample = -1;
	let moved = new Uint32Array(0);

	/** Inject the deformation into a built-in material's vertex shader. */
	function displaced<M extends Material>(material: M): M {
		material.onBeforeCompile = (shader) => {
			shader.uniforms.uRe = uniforms.uRe;
			shader.uniforms.uIm = uniforms.uIm;
			shader.vertexShader = shader.vertexShader
				.replace(
					'#include <common>',
					'#include <common>\nattribute vec3 shape;\nattribute vec3 shapeImag;\nuniform float uRe;\nuniform float uIm;'
				)
				.replace('#include <begin_vertex>', '#include <begin_vertex>\ntransformed += shape * uRe + shapeImag * uIm;');
		};
		material.customProgramCacheKey = () => 'sds-displaced';
		materials.push(material);
		return material;
	}

	function plain<M extends Material>(material: M): M {
		materials.push(material);
		return material;
	}

	function srgbToLinear(c: number) {
		return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
	}

	/** The color of a node at `t` of the scale, or the surface's one color when coloring is off. */
	function paint(colors: Float32Array, n: number, t: number, rgb: [number, number, number]) {
		if (colored) colormap(t, rgb);
		else {
			rgb[0] = 0.62;
			rgb[1] = 0.66;
			rgb[2] = 0.72;
		}
		colors[3 * n] = srgbToLinear(rgb[0]);
		colors[3 * n + 1] = srgbToLinear(rgb[1]);
		colors[3 * n + 2] = srgbToLinear(rgb[2]);
	}

	function loadMode() {
		if (!renderer || !current) return;
		const re = current.shape;
		const im = current.shape_imag;
		(shapeAttr.array as Float32Array).set(re);
		if (im) (shapeImagAttr.array as Float32Array).set(im);
		else (shapeImagAttr.array as Float32Array).fill(0);
		const travel = new Float32Array(re.length / 3);
		maxMag = nodeTravel(re, im, travel) || 1;
		const colors = colorAttr.array as Float32Array;
		const rgb: [number, number, number] = [0, 0, 0];
		for (let n = 0; n < travel.length; n++) paint(colors, n, travel[n] / maxMag, rgb);
		shapeAttr.needsUpdate = true;
		shapeImagAttr.needsUpdate = true;
		colorAttr.needsUpdate = true;
		dirty = true;
	}

	/**
	 * Load the chosen overlay, rotated by its pair's phase and scaled so its largest travel is the
	 * mode's, or hide it when none is chosen.
	 */
	function loadOverlay() {
		if (!renderer || !overlaySurface) return;
		const o = shown;
		const visible = !!o && !!current;
		overlaySurface.visible = visible;
		if (overlayOutline) overlayOutline.visible = visible;
		dirty = true;
		if (!o || !current) return;
		const columns = view.overlays!.length;
		const phi = view.overlay_phase ? view.overlay_phase[Math.min(mode, view.modes.length - 1) * columns + overlay!] : 0;
		alignedShape(o.shape, o.shape_imag, phi, maxMag, overlayAttr.array as Float32Array, overlayImagAttr.array as Float32Array);
		overlayAttr.needsUpdate = true;
		overlayImagAttr.needsUpdate = true;
	}

	/** Ready the buffers for measured motion: every node at rest and colored as such. */
	function loadMotion() {
		if (!renderer || !motion) return;
		maxMag = motion.peak || 1;
		moved = movedNodes(motion);
		(shapeAttr.array as Float32Array).fill(0);
		(shapeImagAttr.array as Float32Array).fill(0);
		const colors = colorAttr.array as Float32Array;
		const rgb: [number, number, number] = [0, 0, 0];
		for (let n = 0; n < colors.length / 3; n++) paint(colors, n, 0, rgb);
		shapeImagAttr.needsUpdate = true;
		shownSample = -1;
		showSample(Math.floor(position));
	}

	/** Write sample k's offsets and the moved nodes' colors. */
	function showSample(k: number) {
		if (!motion) return;
		const offsets = shapeAttr.array as Float32Array;
		writeFrame(motion, k, moved, offsets);
		const colors = colorAttr.array as Float32Array;
		const rgb: [number, number, number] = [0, 0, 0];
		for (const n of moved) paint(colors, n, Math.hypot(offsets[3 * n], offsets[3 * n + 1], offsets[3 * n + 2]) / maxMag, rgb);
		shapeAttr.needsUpdate = true;
		colorAttr.needsUpdate = true;
		shownSample = k;
		sample = k;
		dirty = true;
		if (playhead) playheads.set(playhead, motion.start + k * motion.step, paneId);
	}

	/** Move playback to sample k, as the scrubber does. */
	function seek(k: number) {
		position = Math.min(Math.max(0, k), samples - 1);
	}

	function applyTheme() {
		if (!renderer) return;
		const ghostColor = new Color(cssVar('--text-3'));
		const lineColor = new Color(cssVar('--text-2'));
		(ghostSurface.material as MeshBasicMaterial).color = ghostColor;
		if (ghostOutline) (ghostOutline.material as LineBasicMaterial).color = ghostColor;
		if (outline) (outline.material as LineBasicMaterial).color = lineColor;
		if (mutedSurface) (mutedSurface.material as MeshBasicMaterial).color = new Color(cssVar('--bg-4'));
		if (marks) (marks.material as PointsMaterial).color = new Color(cssVar('--blue-2'));
		if (dots) (dots.material as PointsMaterial).color = new Color(cssVar('--text-2'));
		if (leftOut) (leftOut.material as PointsMaterial).color = new Color(cssVar('--text-2'));
		if (markLines) (markLines.material as LineBasicMaterial).color = new Color(cssVar('--text-1'));
		dirty = true;
	}

	function resetView() {
		if (!renderer) return;
		const dir = new Vector3(0.55, -1.2, 0.58).normalize();
		controls.target.set(centre.x, centre.y, centre.z);
		camera.position.copy(centre).addScaledVector(dir, homeDistance);
		camera.up.set(0, 0, 1);
		controls.update();
		dirty = true;
	}

	const centre = new Vector3();

	function init() {
		try {
			renderer = new WebGLRenderer({ antialias: true, alpha: true, powerPreference: 'high-performance' });
		} catch {
			failure = 'The 3-D view needs WebGL, which is not available in this browser.';
			return;
		}
		renderer.setPixelRatio(Math.min(devicePixelRatio, 2));
		renderer.setClearColor(0x000000, 0);
		canvasHost.appendChild(renderer.domElement);
		renderer.domElement.addEventListener('webglcontextlost', (e) => {
			e.preventDefault();
			failure = 'The graphics context was lost. Switch workspaces and back to rebuild the view.';
		});

		scene = new Scene();
		camera = new PerspectiveCamera(32, 1, 0.001, 1000);
		camera.up.set(0, 0, 1);

		// Geometry: one position buffer shared by the surface, outline and ghosts.
		geometry = new BufferGeometry();
		const position = new BufferAttribute(view.nodes, 3);
		shapeAttr = new BufferAttribute(new Float32Array(view.nodes.length), 3);
		shapeImagAttr = new BufferAttribute(new Float32Array(view.nodes.length), 3);
		// a neutral gray until a mode or a motion colors the nodes: a geometry on its own is not
		// at the bottom of the colormap, which reads as black on a dark background
		colorAttr = new BufferAttribute(new Float32Array(view.nodes.length).fill(srgbToLinear(REST_GRAY)), 3);
		geometry.setAttribute('position', position);
		geometry.setAttribute('shape', shapeAttr);
		geometry.setAttribute('shapeImag', shapeImagAttr);
		geometry.setAttribute('color', colorAttr);
		if (view.triangles) geometry.setIndex(new BufferAttribute(view.triangles, 1));
		geometry.computeBoundingSphere();
		const sphere = geometry.boundingSphere!;
		centre.copy(sphere.center);
		const radius = Math.max(sphere.radius, 1e-6);
		homeDistance = (radius / Math.sin((camera.fov * Math.PI) / 360)) * 0.82;
		camera.near = radius / 100;
		camera.far = radius * 100;
		camera.updateProjectionMatrix();

		boundsBox = new Box3Helper(new Box3().setFromBufferAttribute(position), 0x8b949e);
		boundsBox.visible = bounds;
		scene.add(boundsBox);

		surface = new Mesh(
			geometry,
			displaced(new MeshLambertMaterial({ vertexColors: true, side: DoubleSide, flatShading: true }))
		);
		surface.frustumCulled = false;
		scene.add(surface);

		ghostSurface = new Mesh(
			geometry,
			plain(new MeshBasicMaterial({ transparent: true, opacity: 0.1, depthWrite: false, side: DoubleSide }))
		);
		ghostSurface.visible = false;
		ghostSurface.renderOrder = 2;
		scene.add(ghostSurface);

		if (view.overlays?.length) {
			// the same nodes and faces, moved by the overlay's own shape attributes
			overlayAttr = new BufferAttribute(new Float32Array(view.nodes.length), 3);
			overlayImagAttr = new BufferAttribute(new Float32Array(view.nodes.length), 3);
			overlayGeometry = new BufferGeometry();
			overlayGeometry.setAttribute('position', position);
			overlayGeometry.setAttribute('shape', overlayAttr);
			overlayGeometry.setAttribute('shapeImag', overlayImagAttr);
			if (view.triangles) overlayGeometry.setIndex(new BufferAttribute(view.triangles, 1));
			overlaySurface = new Mesh(
				overlayGeometry,
				displaced(new MeshBasicMaterial({ color: OVERLAY_COLOR, transparent: true, opacity: 0.45, depthWrite: false, side: DoubleSide }))
			);
			overlaySurface.frustumCulled = false;
			overlaySurface.renderOrder = 3;
			overlaySurface.visible = false;
			scene.add(overlaySurface);
			if (view.lines && view.lines.length) {
				overlayLineGeometry = new BufferGeometry();
				overlayLineGeometry.setAttribute('position', position);
				overlayLineGeometry.setAttribute('shape', overlayAttr);
				overlayLineGeometry.setAttribute('shapeImag', overlayImagAttr);
				overlayLineGeometry.setIndex(new BufferAttribute(view.lines, 1));
				overlayOutline = new LineSegments(overlayLineGeometry, displaced(new LineBasicMaterial({ color: OVERLAY_COLOR })));
				overlayOutline.frustumCulled = false;
				overlayOutline.visible = false;
				scene.add(overlayOutline);
			}
		}

		if (view.marks && view.marks.lines.length) {
			// DOF arrows and sensor bodies: line work that stays put while the model moves
			const g = new BufferGeometry();
			const { nodes: mn, lines: ml } = view.marks;
			const segs = new Float32Array(ml.length * 3);
			for (let i = 0; i < ml.length; i++) {
				const n = ml[i] * 3;
				segs[i * 3] = mn[n];
				segs[i * 3 + 1] = mn[n + 1];
				segs[i * 3 + 2] = mn[n + 2];
			}
			g.setAttribute('position', new BufferAttribute(segs, 3));
			markLines = new LineSegments(g, new LineBasicMaterial({ color: new Color(cssVar('--text-1')), transparent: true, opacity: 0.95 }));
			markLines.frustumCulled = false;
			scene.add(markLines);
		}

		if (view.lines && view.lines.length) {
			lineGeometry = new BufferGeometry();
			lineGeometry.setAttribute('position', position);
			lineGeometry.setAttribute('shape', shapeAttr);
			lineGeometry.setAttribute('shapeImag', shapeImagAttr);
			lineGeometry.setIndex(new BufferAttribute(view.lines, 1));
			outline = new LineSegments(lineGeometry, displaced(new LineBasicMaterial({ transparent: true, opacity: 0.9 })));
			outline.frustumCulled = false;
			scene.add(outline);
			ghostOutline = new LineSegments(
				lineGeometry,
				plain(new LineBasicMaterial({ transparent: true, opacity: 0.55, depthWrite: false }))
			);
			ghostOutline.visible = false;
			scene.add(ghostOutline);
		}

		if (view.pick?.target === 'faces' && view.pick.faces && view.triangles) {
			const kept = new Set(view.pick.kept ?? []);
			const muted: number[] = [];
			view.pick.faces.forEach((owner, t) => {
				if (!kept.has(owner)) muted.push(view.triangles![3 * t], view.triangles![3 * t + 1], view.triangles![3 * t + 2]);
			});
			if (muted.length) {
				const g = new BufferGeometry();
				g.setAttribute('position', position);
				g.setIndex(muted);
				mutedSurface = new Mesh(
					g,
					plain(new MeshBasicMaterial({ side: DoubleSide, polygonOffset: true, polygonOffsetFactor: -1, polygonOffsetUnits: -1 }))
				);
				mutedSurface.frustumCulled = false;
				scene.add(mutedSurface);
			}
		}
		if (view.pick && view.pick.target !== 'faces') {
			const g = new BufferGeometry();
			g.setAttribute('position', position);
			const off = view.pick.off ?? 0;
			g.setDrawRange(0, (view.pick.ids?.length ?? view.nodes.length / 3) - off);
			// marked places show through the model, since a seam runs inside a solid
			const places = view.pick.off !== undefined;
			dots = new Points(g, plain(new PointsMaterial({ size: places ? 7 : 4, sizeAttenuation: false, depthTest: !places })));
			if (places) dots.renderOrder = 4;
			dots.frustumCulled = false;
			scene.add(dots);
			if (off) {
				// the places left out: still there to click, and dim so they read as out
				const dim = new BufferGeometry();
				dim.setAttribute('position', position);
				dim.setDrawRange((view.pick.ids?.length ?? 0) - off, off);
				leftOut = new Points(dim, plain(new PointsMaterial({ size: 6, sizeAttenuation: false, transparent: true, opacity: 0.35, depthTest: false })));
				leftOut.renderOrder = 4;
				leftOut.frustumCulled = false;
				scene.add(leftOut);
			}
		}
		if (view.pick?.target === 'nodes') {
			markGeometry = new BufferGeometry();
			markGeometry.setAttribute('position', new BufferAttribute(new Float32Array(0), 3));
			marks = new Points(markGeometry, plain(new PointsMaterial({ size: 9, sizeAttenuation: false, depthTest: false })));
			marks.renderOrder = 5;
			marks.frustumCulled = false;
			scene.add(marks);
		}

		// A headlight that follows the camera keeps shading even while orbiting.
		scene.add(new AmbientLight(0xffffff, 1.05));
		const head = new DirectionalLight(0xffffff, 2.1);
		head.position.set(0.45, 0.9, 1);
		camera.add(head);
		scene.add(camera);

		controls = new OrbitControls(camera, renderer.domElement);
		controls.enableDamping = true;
		controls.dampingFactor = 0.12;
		controls.rotateSpeed = 0.8;
		controls.zoomSpeed = 0.9;
		controls.addEventListener('change', () => (dirty = true));

		const pose = view.pick && scope ? poses.get(scope) : undefined;
		if (pose) {
			camera.position.fromArray(pose.position);
			camera.up.fromArray(pose.up);
			controls.target.fromArray(pose.target);
			controls.update();
		} else resetView();
		applyTheme();
		loadMode();
		loadOverlay();
		loadMotion();

		resizeObs = new ResizeObserver(resize);
		resizeObs.observe(canvasHost);
		resize();

		lastTime = performance.now();
		fpsWindowStart = lastTime;
		raf = requestAnimationFrame(loop);
	}

	function resize() {
		if (!renderer) return;
		const w = Math.max(1, canvasHost.clientWidth);
		const h = Math.max(1, canvasHost.clientHeight);
		renderer.setSize(w, h, false);
		renderer.domElement.style.width = '100%';
		renderer.domElement.style.height = '100%';
		camera.aspect = w / h;
		camera.updateProjectionMatrix();
		dirty = true;
	}

	const tmp = new Vector3();
	const inv = new Quaternion();
	function updateTriad() {
		if (!triad) return;
		inv.copy(camera.quaternion).invert();
		const axes: [string, Vector3][] = [
			['x', new Vector3(1, 0, 0)],
			['y', new Vector3(0, 1, 0)],
			['z', new Vector3(0, 0, 1)]
		];
		for (const [name, v] of axes) {
			tmp.copy(v).applyQuaternion(inv);
			const line = triad.querySelector<SVGLineElement>(`line.${name}`);
			const label = triad.querySelector<SVGTextElement>(`text.${name}`);
			const x = 24 + tmp.x * 15;
			const y = 24 - tmp.y * 15;
			line?.setAttribute('x2', x.toFixed(1));
			line?.setAttribute('y2', y.toFixed(1));
			label?.setAttribute('x', (24 + tmp.x * 21).toFixed(1));
			label?.setAttribute('y', (24 - tmp.y * 21 + 3.5).toFixed(1));
		}
	}

	const corner = new Vector3();
	/** Place the box's ticks, values and titles on the picture, on the edge of each axis that lies outermost; nothing while the axes are off. */
	function updateAxes() {
		if (!axesSvg) return;
		if (!bounds || !boundsBox) {
			if (axesSvg.firstChild) axesSvg.replaceChildren();
			return;
		}
		const w = Math.max(1, canvasHost.clientWidth);
		const h = Math.max(1, canvasHost.clientHeight);
		axesSvg.setAttribute('viewBox', `0 0 ${w} ${h}`);
		const { lo, hi } = extents;
		const onPicture = (): Point => ({ x: ((corner.x + 1) / 2) * w, y: ((1 - corner.y) / 2) * h });
		const corners: Point[] = [];
		for (let i = 0; i < 8; i++) {
			corner.set(i & 1 ? hi[0] : lo[0], i & 2 ? hi[1] : lo[1], i & 4 ? hi[2] : lo[2]).project(camera);
			corners.push(onPicture());
		}
		corner.set((lo[0] + hi[0]) / 2, (lo[1] + hi[1]) / 2, (lo[2] + hi[2]) / 2).project(camera);
		const centre = onPicture();
		const unit = view.unit === 'undefined' ? '' : ` ${view.unit}`;
		// a label `by` pixels out from `p`, kept on the canvas by its anchor and its width (about 6.5 px a character)
		const at = (p: Point, out: Point, by: number, anchor: string, text: string) => {
			const half = 3.3 * text.length;
			const least = anchor === 'start' ? 3 : anchor === 'end' ? 3 + 2 * half : 3 + half;
			const most = anchor === 'start' ? w - 3 - 2 * half : anchor === 'end' ? w - 3 : w - 3 - half;
			const x = Math.min(most, Math.max(least, p.x + out.x * by));
			const y = Math.min(h - 8, Math.max(8, p.y + out.y * by));
			return `x="${x.toFixed(1)}" y="${y.toFixed(1)}" text-anchor="${anchor}"`;
		};
		let drawn = '';
		labeledEdges(corners, centre).forEach((edge, k) => {
			const a = corners[edge.from];
			const b = corners[edge.to];
			// a tick every 70 px or so, and only the ends of an edge seen short
			const { values, decimals } = niceTicks(lo[k], hi[k], Math.max(1, Math.min(5, Math.floor(Math.hypot(b.x - a.x, b.y - a.y) / 70))));
			const anchor = Math.abs(edge.out.x) < 0.35 ? 'middle' : edge.out.x > 0 ? 'start' : 'end';
			for (const v of values) {
				const p = along(a, b, lo[k], hi[k], v);
				drawn += `<line x1="${p.x.toFixed(1)}" y1="${p.y.toFixed(1)}" x2="${(p.x + edge.out.x * 5).toFixed(1)}" y2="${(p.y + edge.out.y * 5).toFixed(1)}"/>`;
				const text = v.toFixed(decimals);
				drawn += `<text ${at(p, edge.out, 9, anchor, text)} dominant-baseline="middle">${text}</text>`;
			}
			const title = `${'XYZ'[k]}${unit}`;
			drawn += `<text class="title" ${at(along(a, b, 0, 1, 0.5), edge.out, 30, 'middle', title)} dominant-baseline="middle">${title}</text>`;
		});
		axesSvg.innerHTML = drawn;
	}

	function loop(now: number) {
		raf = requestAnimationFrame(loop);
		if (!renderer) return;
		const dt = Math.min(0.1, (now - lastTime) / 1000);
		lastTime = now;
		const moved = controls.update();
		let re: number;
		let im: number;
		if (motion) {
			if (playing && samples) position = (position + advance(samples, dt, SPEEDS[speedIndex])) % samples;
			const k = Math.floor(position);
			if (k !== shownSample) showSample(k);
			re = peak / maxMag;
			im = 0;
		} else {
			if (playing) {
				phase = (phase + dt / CYCLE_S) % 1;
				dirty = true;
			}
			// Re(shape exp(i (theta - pi/2))): a real shape starts at rest and swings as sin(theta)
			const scale = peak / maxMag;
			const theta = 2 * Math.PI * phase;
			re = scale * Math.sin(theta);
			im = scale * Math.cos(theta);
		}
		if (uniforms.uRe.value !== re || uniforms.uIm.value !== im) {
			uniforms.uRe.value = re;
			uniforms.uIm.value = im;
			dirty = true;
		}
		if (!dirty && !moved) {
			fpsWindowStart = now;
			frames = 0;
			return;
		}
		dirty = false;
		renderer.render(scene, camera);
		updateTriad();
		updateAxes();
		frames++;
		if (now - fpsWindowStart >= 500) {
			fps = playing ? Math.round((frames * 1000) / (now - fpsWindowStart)) : fps;
			frames = 0;
			fpsWindowStart = now;
		}
	}

	onMount(init);

	// a linked plot's line, dragged, moves playback to it
	$effect(() => {
		const group = playhead;
		if (!group) return;
		const unsubscribe = playheads.subscribe(group, (state) => {
			if (motion && state.source !== paneId && state.x !== null) seek(sampleAt(motion, state.x));
		});
		return () => {
			unsubscribe();
			playheads.clear(group, paneId);
		};
	});

	// Mode or view changes: reload the shape buffer, and the overlay scaled to it.
	$effect(() => {
		void current;
		loadMode();
		void shown;
		loadOverlay();
	});

	$effect(() => {
		void theme.version;
		applyTheme();
	});

	$effect(() => {
		if (!renderer) return;
		for (const m of [surface.material as MeshLambertMaterial]) {
			m.wireframe = wireframe;
			m.transparent = opacity < 1;
			m.opacity = opacity;
			m.depthWrite = opacity >= 1;
			m.needsUpdate = true;
		}
		ghostSurface.visible = ghost;
		if (ghostOutline) ghostOutline.visible = ghost;
		dirty = true;
	});

	$effect(() => {
		void peak;
		dirty = true;
	});

	onDestroy(() => {
		if (markLines) {
			markLines.geometry.dispose();
			(markLines.material as Material).dispose();
		}
		cancelAnimationFrame(raf);
		resizeObs?.disconnect();
		if (!renderer) return;
		if (view.pick && scope) poses.set(scope, { position: camera.position.toArray(), target: controls.target.toArray(), up: camera.up.toArray() });
		controls.dispose();
		geometry.dispose();
		lineGeometry?.dispose();
		mutedSurface?.geometry.dispose();
		dots?.geometry.dispose();
		leftOut?.geometry.dispose();
		markGeometry?.dispose();
		overlayGeometry?.dispose();
		overlayLineGeometry?.dispose();
		for (const m of materials) m.dispose();
		renderer.dispose();
		renderer.forceContextLoss();
		renderer.domElement.remove();
		renderer = null;
	});

	// The picked nodes and the one under the cursor, as marks on the model.
	$effect(() => {
		if (!renderer || !markGeometry) return;
		const rows = hoverNode !== null && !picked.includes(hoverNode) ? [...picked, hoverNode] : picked;
		const xyz = new Float32Array(3 * rows.length);
		rows.forEach((r, k) => xyz.set(view.nodes.subarray(3 * r, 3 * r + 3), 3 * k));
		markGeometry.setAttribute('position', new BufferAttribute(xyz, 3));
		dirty = true;
	});

	const raycaster = new Raycaster();
	const pointer = new Vector2();
	let pressedAt: [number, number] | null = null;

	/** The cursor's place in the canvas, in CSS pixels. */
	function at(e: PointerEvent): [number, number] {
		const box = canvasHost.getBoundingClientRect();
		return [e.clientX - box.left, e.clientY - box.top];
	}

	/** The pickable node nearest (x, y) on screen, as a row of `view.nodes`. */
	function nodeAt(x: number, y: number, over?: number): number | null {
		const count = over ?? Math.min(pick?.ids?.length ?? 0, nodeCount);
		const w = canvasHost.clientWidth;
		const h = canvasHost.clientHeight;
		const screen = new Float32Array(2 * count);
		const visible: boolean[] = [];
		for (let n = 0; n < count; n++) {
			tmp.set(view.nodes[3 * n], view.nodes[3 * n + 1], view.nodes[3 * n + 2]).project(camera);
			screen[2 * n] = ((tmp.x + 1) / 2) * w;
			screen[2 * n + 1] = ((1 - tmp.y) / 2) * h;
			visible.push(tmp.z < 1);
		}
		return nearestNode(screen, visible, count, x, y);
	}

	/** Aim the raycaster through (x, y) and return what it meets on the model's faces. */
	function aim(x: number, y: number) {
		pointer.set((x / canvasHost.clientWidth) * 2 - 1, -(y / canvasHost.clientHeight) * 2 + 1);
		raycaster.setFromCamera(pointer, camera);
		return view.triangles ? raycaster.intersectObject(surface, false)[0] : undefined;
	}

	function commitNodes() {
		const p = pick;
		if (!p?.ids || !p.param || picked.length < (p.count === 1 ? 1 : 2)) return;
		onpick?.({ [p.param]: idList(picked.map((r) => p.ids![r])) });
		picked = [];
	}

	function click(x: number, y: number) {
		const p = pick;
		if (!p || !renderer) return;
		if (p.target === 'nodes') {
			const row = nodeAt(x, y);
			if (row === null) return;
			picked = picked.includes(row) ? picked.filter((r) => r !== row) : [...picked, row];
			if (p.count && picked.length === p.count) commitNodes();
			return;
		}
		const hit = aim(x, y);
		if (p.target === 'faces') {
			const owner = hit?.faceIndex != null ? p.faces?.[hit.faceIndex] : undefined;
			if (owner !== undefined && owner >= 0) onpick?.(toggledSurface(p.params, p.kept ?? [], p.out!, p.back!, owner));
			return;
		}
		let point: [number, number, number];
		if (hit) point = [hit.point.x, hit.point.y, hit.point.z];
		else {
			const count = nodeCount;
			const w = canvasHost.clientWidth;
			const h = canvasHost.clientHeight;
			let anchor: [number, number, number] = [centre.x, centre.y, centre.z];
			let nearest = Infinity;
			for (let n = 0; n < count; n++) {
				tmp.set(view.nodes[3 * n], view.nodes[3 * n + 1], view.nodes[3 * n + 2]).project(camera);
				const d = (((tmp.x + 1) / 2) * w - x) ** 2 + (((1 - tmp.y) / 2) * h - y) ** 2;
				if (d < nearest) {
					nearest = d;
					anchor = [view.nodes[3 * n], view.nodes[3 * n + 1], view.nodes[3 * n + 2]];
				}
			}
			const o = raycaster.ray.origin;
			const d = raycaster.ray.direction;
			point = placedPoint([o.x, o.y, o.z], [d.x, d.y, d.z], anchor, modelPlane(view.nodes, count));
		}
		const [scale, offset] = p.to_si ?? [1, 0];
		onpick?.({ x: point[0] * scale + offset, y: point[1] * scale + offset, z: point[2] * scale + offset });
	}

	function onPointerDown(e: PointerEvent) {
		pressedAt = e.button === 0 ? at(e) : null;
	}

	function onPointerUp(e: PointerEvent) {
		const start = pressedAt;
		pressedAt = null;
		if (!start || e.button !== 0) return;
		const [x, y] = at(e);
		if (Math.hypot(x - start[0], y - start[1]) <= CLICK_SLOP) click(x, y);
	}

	function onPointerMove(e: PointerEvent) {
		if (e.buttons) return;
		if (pick?.target === 'nodes') {
			hoverNode = nodeAt(...at(e));
			return;
		}
		// the caption outside picking: once a frame, and only on a model small enough to project
		// every node on each move (a large one is looked at, not read node by node)
		if (!view.node_ids || nodeCount > HOVER_LIMIT || hoverPending) return;
		hoverPending = true;
		const [x, y] = at(e);
		requestAnimationFrame(() => {
			hoverPending = false;
			hovered = nodeAt(x, y, nodeCount);
		});
	}

	$effect(() => {
		void colored;
		if (!renderer) return;
		if (motion) loadMotion();
		else loadMode();
	});
	$effect(() => {
		if (boundsBox) {
			boundsBox.visible = bounds;
			dirty = true;
		}
	});

	/** Move to the next or previous mode, staying within the view's modes. */
	function step(by: number) {
		mode = Math.min(view.modes.length - 1, Math.max(0, mode + by));
	}

	function onKey(e: KeyboardEvent) {
		if (e.target instanceof HTMLInputElement || e.metaKey || e.ctrlKey || e.altKey) return;
		const k = e.key.toLowerCase();
		if (k === ' ') playing = !playing;
		else if (k === 'w') wireframe = !wireframe;
		else if (k === 'g') ghost = !ghost;
		else if (k === 'r') resetView();
		else if (e.key === 'ArrowDown' && view.modes.length > 1) step(1);
		else if (e.key === 'ArrowUp' && view.modes.length > 1) step(-1);
		else if (e.key === 'Enter' && pick?.target === 'nodes') commitNodes();
		else if (e.key === 'Escape' && picked.length) picked = [];
		else return;
		e.preventDefault();
	}

	const legendTicks = $derived(
		[0, 0.5, 1].map((t) => formatNumber(motion ? t * motion.peak : t * peak * display.factor, 3))
	);
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
<div
	class="shape3d"
	data-testid="view-shape3d"
	bind:this={host}
	tabindex="0"
	role="group"
	aria-label="{view.title}. Space plays or pauses, W toggles wireframe, G toggles the undeformed ghost, R resets the view, Up and Down step through the modes."
	onkeydown={onKey}
>
	<!-- svelte-ignore a11y_no_static_element_interactions (clicks are also the keyboard's: Enter commits, Escape clears) -->
	<div
		class="canvas"
		class:picking={!!pick}
		data-testid="shape3d-canvas"
		bind:this={canvasHost}
		onpointerdown={onPointerDown}
		onpointerup={onPointerUp}
		onpointermove={onPointerMove}
		onpointerleave={() => (hoverNode = null)}
	>
		<svg class="axes" bind:this={axesSvg} data-testid="shape3d-axes" aria-hidden="true"></svg>
	</div>

	{#if pick}
		<div class="pickbar" data-testid="shape3d-pick" role="status">
			<span>{pick.hint}</span>
			{#if picked.length}
				<span class="num" data-testid="shape3d-picked">Picked {idList(picked.map((r) => pick.ids?.[r] ?? r))}</span>
				{#if !pick.count}<button class="btn sm" data-testid="shape3d-pick-done" onclick={commitNodes} title="Make the elements (Enter)">Done</button>{/if}
				<button class="btn ghost sm" onclick={() => (picked = [])} title="Clear the picks (Escape)">Clear</button>
			{/if}
		</div>
	{/if}

	{#if failure}
		<div class="failure" role="alert">{failure}</div>
	{/if}

	<div class="top">
		<div class="toolbar" role="toolbar" aria-label="Mode shape display">
			{#if moves}
			<button
				class="btn icon sm"
				data-testid="shape3d-play"
				onclick={() => (playing = !playing)}
				aria-pressed={playing}
				title={playing ? 'Pause (Space)' : 'Play (Space)'}
				aria-label={playing ? 'Pause' : 'Play'}
			>
				{#if playing}
					<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="M5 3.5v9M11 3.5v9" stroke="currentColor" stroke-width="2" stroke-linecap="round" /></svg>
				{:else}
					<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="M5 3.2v9.6L12.6 8z" fill="currentColor" /></svg>
				{/if}
			</button>
			<label class="amp">
				<span class="lbl">Amplitude</span>
				<input type="range" data-testid="shape3d-amplitude" min="0" max={amplitudeMax} step={amplitudeMax / 400} bind:value={amplitude} aria-valuetext="Amplitude {formatFixed(amplitude, 3)}, peak {formatFixed(peak * display.factor, 3)} {display.unit}" />
				<span class="ampval num">{formatFixed(peak * display.factor, 3)} {display.unit}</span>
			</label>
			<label class="scale" title="The slider's top: the amplitude at its right end, 0.1 to 1000">
				<span class="lbl">to</span>
				<input class="field mono num" data-testid="shape3d-amplitude-max" type="number" min="0.1" max="1000" step="any" value={Number(amplitudeMax.toPrecision(3))} onchange={(e) => setAmplitudeMax(e.currentTarget.value)} aria-label="The slider's top amplitude" />
			</label>
			{/if}
			<div class="seg" role="radiogroup" aria-label="Surface style">
				<button role="radio" data-testid="shape3d-surface" aria-checked={!wireframe} class:on={!wireframe} onclick={() => (wireframe = false)} title="Surface (W)">Surface</button>
				<button role="radio" data-testid="shape3d-wireframe" aria-checked={wireframe} class:on={wireframe} onclick={() => (wireframe = true)} title="Wireframe (W)">Wireframe</button>
			</div>
			{#if !wireframe}
				<label class="amp opacity" title="How solid the surface is drawn; lower it to see through to what is inside">
					<span class="lbl">Opacity</span>
					<input type="range" data-testid="shape3d-opacity" min="0.1" max="1" step="0.01" bind:value={opacity} aria-valuetext="{Math.round(opacity * 100)} %" />
				</label>
			{/if}
			{#if moves}
			<button class="btn sm toggle" data-testid="shape3d-ghost" class:on={ghost} aria-pressed={ghost} onclick={() => (ghost = !ghost)} title="Undeformed ghost (G)">
				<svg viewBox="0 0 16 16" width="13" height="13" aria-hidden="true"><rect x="2" y="5" width="12" height="6" rx="1" fill="none" stroke="currentColor" stroke-width="1.3" stroke-dasharray="2 1.6" /></svg>
				Ghost
			</button>
			{/if}
			{#if moves}
			<button class="btn sm toggle" data-testid="shape3d-color" class:on={colored} aria-pressed={colored} onclick={() => (colored = !colored)} title="Color by deflection">Color</button>
			{/if}
			<button class="btn sm toggle" data-testid="shape3d-bounds" class:on={bounds} aria-pressed={bounds} onclick={() => (bounds = !bounds)} title="Plot axes: a box at the model's bounds, one edge per axis ticked and titled">Axes</button>
{#if moves}<button class="btn ghost sm copyimg" data-testid="shape3d-record" onclick={record} disabled={recording}
				title="Record {RECORD_S.toFixed(1)} s of the animation as a video">{recording ? 'Recording' : 'Record'}</button>{/if}
			<button class="btn ghost sm copyimg" data-testid="shape3d-copy-image" onclick={copyImage} title="Copy the 3-D view as an image">{copiedImage === 'copied' ? 'Copied' : copiedImage === 'saved' ? 'Saved as PNG' : 'Copy Image'}</button>
			<button class="btn icon sm" data-testid="shape3d-reset" onclick={resetView} title="Reset view (R)" aria-label="Reset view">
				<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="M3 8a5 5 0 105-5H5.5M5.5 1.2V5H9.3" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" /></svg>
			</button>
			{#if motion}
				<span class="speed">
					<button class="btn icon sm" data-testid="shape3d-slower" disabled={speedIndex <= 0} onclick={() => speedIndex--} title="Slower" aria-label="Slower">
						<svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true"><path d="M3.5 8h9" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" /></svg>
					</button>
					<span class="speedval num" data-testid="shape3d-speed" aria-live="polite">{SPEEDS[speedIndex]}x</span>
					<button class="btn icon sm" data-testid="shape3d-faster" disabled={speedIndex >= SPEEDS.length - 1} onclick={() => speedIndex++} title="Faster" aria-label="Faster">
						<svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true"><path d="M3.5 8h9M8 3.5v9" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" /></svg>
					</button>
				</span>
			{/if}
		</div>

		{#if motion && samples}
			<div class="scrub">
				<input
					type="range"
					data-testid="shape3d-time"
					min="0"
					max={samples - 1}
					step="1"
					value={sample}
					oninput={(e) => seek(Number(e.currentTarget.value))}
					aria-label={motion.label}
					aria-valuetext="{formatFixed(motion.start + sample * motion.step, 4)} {motion.unit}"
				/>
				<span class="num">{motion.label} {formatFixed(motion.start + sample * motion.step, 4)} {motion.unit}</span>
			</div>
			{#if view.note}<div class="note">{view.note}</div>{/if}
		{/if}

		{#if current}
			<div class="modetag">
				{#if view.modes.length > 1 && !view.overlays}
					<!-- a view shown outside a workspace with a mode table steps here; a MAC pair is chosen in its matrix -->
					<span class="stepper">
						<button class="btn icon sm" data-testid="shape3d-previous" disabled={mode <= 0} onclick={() => step(-1)} title="Previous mode (Up)" aria-label="Previous mode">
							<svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true"><path d="M10 3.5L5.5 8l4.5 4.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
						</button>
						<button class="btn icon sm" data-testid="shape3d-next" disabled={mode >= view.modes.length - 1} onclick={() => step(1)} title="Next mode (Down)" aria-label="Next mode">
							<svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true"><path d="M6 3.5L10.5 8 6 12.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
						</button>
					</span>
				{/if}
				<span class="m">{current.label ?? `Mode ${current.index}`}</span>
				<span class="num">{formatFixed(current.frequency_hz, 4)} Hz</span>
				{#if current.damping != null}<span class="num dim">{formatFixed(current.damping * 100, 3)} % damping</span>{/if}
				{#if shown}
					<span class="ov" data-testid="shape3d-overlay"
						><span class="swatch" aria-hidden="true"></span>{shown.label ?? `Mode ${shown.index}`}
						<span class="num">{formatFixed(shown.frequency_hz, 4)} Hz</span></span
					>
				{/if}
			</div>
		{/if}
	</div>

	<div class="bottom">
		{#if moves && colored}
		<div class="legend" aria-label="Color legend">
			<div class="ltitle">
				{motion ? `${motion.quantity}${motion.value_unit ? ` (${motion.value_unit})` : ''}` : `Peak displacement (${display.unit})`}
			</div>
			<div class="bar" style:background={colormapGradient()}></div>
			<div class="ticks num">
				{#each legendTicks as t, i (i)}<span>{t}</span>{/each}
			</div>
		</div>
		{/if}

		<div class="corner">
			<svg class="triad" bind:this={triad} viewBox="0 0 48 48" width="48" height="48" aria-hidden="true">
				<line class="x" x1="24" y1="24" x2="39" y2="24" />
				<line class="y" x1="24" y1="24" x2="24" y2="9" />
				<line class="z" x1="24" y1="24" x2="30" y2="30" />
				<text class="x" x="45" y="27">X</text>
				<text class="y" x="24" y="6">Y</text>
				<text class="z" x="33" y="36">Z</text>
			</svg>

			<div class="stats num" aria-live="off">
				{#if hovered !== null && !pick}
					<span class="hovercap" data-testid="shape3d-hover">{caption(hovered)}</span>
				{/if}
				<span class="fps" data-testid="shape3d-fps" class:paused={!playing}>{playing ? (fps === null ? '-- fps' : `${fps} fps`) : 'Paused'}</span>
				<span>{formatCount(nodeCount)} nodes</span>
				<span>{formatCount(triCount)} triangles</span>
			</div>
		</div>
	</div>
</div>

<style>
	.copyimg {
		flex: none;
		font-size: var(--text-2xs, 11px);
	}

	.shape3d {
		position: relative;
		height: 100%;
		min-height: 280px;
		background:
			radial-gradient(ellipse at 50% 35%, var(--bg-3), transparent 70%),
			var(--bg-plot);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-md);
		overflow: hidden;
	}

	.shape3d:focus-visible {
		outline-offset: -2px;
	}

	.canvas {
		position: absolute;
		inset: 0;
	}

	.canvas :global(canvas) {
		display: block;
		touch-action: none;
	}

	.canvas.picking {
		cursor: crosshair;
	}

	.axes {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		overflow: visible;
		pointer-events: none;
	}

	.axes :global(line) {
		stroke: var(--text-2);
		stroke-width: 1;
	}

	.axes :global(text) {
		font-family: var(--font-mono);
		font-size: 11px;
		fill: var(--text-2);
	}

	.axes :global(text.title) {
		font-size: 12px;
		fill: var(--text-1);
	}

	.pickbar {
		position: absolute;
		left: 50%;
		bottom: 30px;
		translate: -50% 0;
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		max-width: calc(100% - 2 * var(--sp-3));
		padding: 4px var(--sp-2);
		white-space: nowrap;
		font-size: var(--text-xs);
		color: var(--text-2);
		background: color-mix(in srgb, var(--bg-1) 88%, transparent);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-md);
	}

	.top,
	.bottom {
		position: absolute;
		left: var(--sp-2);
		right: var(--sp-3);
		display: flex;
		flex-wrap: wrap;
		justify-content: space-between;
		gap: var(--sp-2) var(--sp-4);
		pointer-events: none;
	}

	.top {
		top: var(--sp-2);
		align-items: flex-start;
	}

	.bottom {
		bottom: var(--sp-2);
		left: var(--sp-3);
		align-items: flex-end;
	}

	.toolbar {
		pointer-events: auto;
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2);
		padding: 4px;
		background: color-mix(in srgb, var(--bg-1) 88%, transparent);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-md);
		backdrop-filter: blur(6px);
	}

	.scale {
		display: inline-flex;
		align-items: center;
		gap: 3px;
	}

	.scale input {
		width: 6ch;
		padding: 1px 4px;
		font-size: var(--text-xs);
	}

	.hovercap {
		color: var(--text-1);
	}

	.amp {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
		padding: 0 var(--sp-1);
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	.amp input {
		width: 110px;
	}

	.opacity input {
		width: 70px;
	}

	.ampval {
		min-width: 7.5ch;
		color: var(--text-1);
		text-align: right;
	}

	.seg {
		display: inline-flex;
		padding: 2px;
		background: var(--bg-2);
		border: 1px solid var(--line-2);
		border-radius: var(--radius-md);
	}

	.seg button {
		height: 20px;
		padding: 0 var(--sp-2);
		font-size: var(--text-xs);
		color: var(--text-2);
		background: transparent;
		border: 0;
		border-radius: 4px;
	}

	.seg button.on {
		color: var(--text-1);
		background: var(--bg-4);
		box-shadow: inset 0 0 0 1px var(--line-3);
	}

	.toggle.on {
		color: var(--text-1);
		background: var(--blue-weak);
		border-color: var(--blue-line);
	}

	.speed {
		display: inline-flex;
		align-items: center;
		gap: 2px;
	}

	.speedval {
		min-width: 4.5ch;
		font-size: var(--text-xs);
		color: var(--text-1);
		text-align: center;
	}

	.scrub {
		pointer-events: auto;
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		margin-top: 6px;
		padding: 4px var(--sp-2);
		font-size: var(--text-xs);
		color: var(--text-1);
		background: color-mix(in srgb, var(--bg-1) 88%, transparent);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-md);
	}

	.scrub input {
		width: 220px;
	}

	.note {
		padding-top: 6px;
		font-size: var(--text-xs);
		color: var(--text-3);
	}

	.modetag {
		padding-top: 6px;
		display: flex;
		align-items: baseline;
		gap: var(--sp-3);
		font-size: var(--text-sm);
		color: var(--text-1);
		pointer-events: none;
	}

	.stepper {
		display: inline-flex;
		gap: 2px;
		pointer-events: auto;
	}

	.ov {
		display: inline-flex;
		align-items: baseline;
		gap: 6px;
		color: var(--text-1);
	}

	.swatch {
		align-self: center;
		width: 10px;
		height: 10px;
		background: #e69f00;
		border-radius: 2px;
	}

	.modetag .m {
		font-weight: var(--weight-strong);
	}

	.dim {
		color: var(--text-3);
	}

	.legend {
		width: 200px;
		padding-bottom: 2px;
	}

	.corner {
		display: flex;
		flex-direction: column;
		align-items: flex-end;
		gap: 2px;
	}

	.ltitle {
		margin-bottom: 4px;
		font-size: var(--text-2xs);
		color: var(--text-2);
	}

	.bar {
		height: 9px;
		border-radius: 2px;
		box-shadow: inset 0 0 0 1px rgba(0, 0, 0, 0.25);
	}

	.ticks {
		display: flex;
		justify-content: space-between;
		margin-top: 3px;
		font-size: var(--text-2xs);
		color: var(--text-2);
	}

	.triad {
		margin-right: -6px;
	}

	.triad line {
		stroke-width: 1.6;
		stroke-linecap: round;
	}

	.triad line.x {
		stroke: var(--blue-2);
	}

	.triad line.y {
		stroke: var(--amber-2);
		stroke-dasharray: 3 2;
	}

	.triad line.z {
		stroke: var(--text-2);
		stroke-dasharray: 1 2;
	}

	.triad text {
		font-family: var(--font-mono);
		font-size: 8.5px;
		fill: var(--text-2);
		text-anchor: middle;
	}

	.stats {
		display: flex;
		gap: var(--sp-3);
		font-size: var(--text-2xs);
		color: var(--text-3);
	}

	.fps {
		min-width: 6.5ch;
		color: var(--text-1);
		text-align: right;
	}

	.fps.paused {
		color: var(--text-3);
	}

	.failure {
		position: absolute;
		inset: 0;
		display: grid;
		place-items: center;
		padding: var(--sp-6);
		color: var(--text-2);
		text-align: center;
	}
</style>
