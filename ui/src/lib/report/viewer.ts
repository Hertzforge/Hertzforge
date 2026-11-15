/**
 * The exported report's page: the report the core wrote into this file as one protocol frame
 * (`report.views`, base64 in `#report-data`), drawn by `ReportViewer` with the shell's own viewers.
 * Built alone into one self-contained HTML file (`vite.report.config.ts`) that opens offline.
 */
import '@fontsource-variable/atkinson-hyperlegible-next';
import '@fontsource-variable/atkinson-hyperlegible-mono';
import '$lib/theme/tokens.css';
import '$lib/theme/base.css';
import { mount } from 'svelte';
import type { ReportPageData } from '$lib/protocol/types';
import { readReport } from './read';
import ReportViewer from './ReportViewer.svelte';
import AnimationStage, { type AnimationSetup } from './AnimationStage.svelte';
import type { Shape3DView } from '$lib/protocol/types';

const target = document.getElementById('report');
if (target) {
	let page: ReportPageData | null = null;
	let error: string | null = null;
	try {
		page = readReport(document.getElementById('report-data')?.textContent ?? '');
	} catch (err) {
		error = (err as Error).message;
	}
	// an animation in place of a report (`hertzforge export-animation`): one 3-D view, playing
	const animation = (page as unknown as { animation?: { view: Shape3DView; setup: AnimationSetup } } | null)?.animation;
	if (animation) mount(AnimationStage, { target, props: { view: animation.view, setup: animation.setup } });
	else mount(ReportViewer, { target, props: { page, error } });
}
