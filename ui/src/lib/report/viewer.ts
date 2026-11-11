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

const target = document.getElementById('report');
if (target) {
	let page: ReportPageData | null = null;
	let error: string | null = null;
	try {
		page = readReport(document.getElementById('report-data')?.textContent ?? '');
	} catch (err) {
		error = (err as Error).message;
	}
	mount(ReportViewer, { target, props: { page, error } });
}
