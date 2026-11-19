/** The report an exported page holds: `report.views`'s answer as one protocol frame, base64. */
import { decodeFrame } from '$lib/protocol/codec';
import type { ReportPageData } from '$lib/protocol/types';

/** The report written into the page, or an Error saying it holds none. */
export function readReport(text: string): ReportPageData {
	const trimmed = text.trim();
	if (!trimmed || trimmed.startsWith('<!--')) throw new Error('This page holds no report.');
	const binary = atob(trimmed);
	const bytes = new Uint8Array(binary.length);
	for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
	// a frame with no arrays is JSON text; one with arrays is the binary frame
	const message = decodeFrame(bytes[0] === 0x7b ? new TextDecoder().decode(bytes) : bytes);
	if (message.type !== 'response' || !('result' in message)) throw new Error('This page holds no report.');
	return message.result as unknown as ReportPageData;
}
