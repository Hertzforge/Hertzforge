/**
 * A drawing put on the clipboard as a PNG; saved as a PNG file instead where the clipboard refuses
 * an image.
 * The canvas is flattened onto `background` (a WebGL canvas is drawn with a clear background), so
 * the image reads as it does on screen. A WebGL canvas must be copied in the same task it was
 * rendered in, before the browser clears it: `render` draws it first. Says which happened.
 */
export async function copyCanvas(
	canvas: HTMLCanvasElement,
	background: string,
	render?: () => void,
	name = 'figure'
): Promise<'copied' | 'saved' | null> {
	render?.();
	const flat = document.createElement('canvas');
	flat.width = canvas.width;
	flat.height = canvas.height;
	const ctx = flat.getContext('2d');
	if (!ctx) return null;
	ctx.fillStyle = background;
	ctx.fillRect(0, 0, flat.width, flat.height);
	ctx.drawImage(canvas, 0, 0);
	const blob = await new Promise<Blob | null>((resolve) => flat.toBlob(resolve, 'image/png'));
	if (!blob) return null;
	try {
		await navigator.clipboard.write([new ClipboardItem({ 'image/png': blob })]);
		return 'copied';
	} catch {
		// no image on this clipboard: the file, where it can be put in a slide just the same
		const link = document.createElement('a');
		link.href = URL.createObjectURL(blob);
		link.download = `${name.replace(/[^\w .-]+/g, '_').slice(0, 80) || 'figure'}.png`;
		link.click();
		setTimeout(() => URL.revokeObjectURL(link.href), 1000);
		return 'saved';
	}
}

/** The page's plot background, read from the theme. */
export function plotBackground(from: Element): string {
	return getComputedStyle(from).getPropertyValue('--bg-plot').trim() || getComputedStyle(from).backgroundColor || '#ffffff';
}
