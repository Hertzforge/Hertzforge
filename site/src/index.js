// Static assets answer every request with the whole file, Range header or not.
// Safari, and iOS above all, will not play a video unless a Range request comes
// back as 206 Partial Content, so videos are routed here (run_worker_first in
// wrangler.jsonc) and cut to the range asked for.
export default {
  async fetch(request, env) {
    const asset = await env.ASSETS.fetch(request);
    if (asset.status !== 200 || request.method !== 'GET') return asset;

    const headers = new Headers(asset.headers);
    headers.set('Accept-Ranges', 'bytes');
    const range = /^bytes=(\d*)-(\d*)$/.exec((request.headers.get('Range') || '').trim());
    if (!range || (range[1] === '' && range[2] === '')) {
      // No range, several ranges, or one we cannot read: the whole file is a valid answer.
      return new Response(asset.body, { status: 200, headers });
    }

    const body = await asset.arrayBuffer();
    const size = body.byteLength;
    let start, end;
    if (range[1] !== '') {
      start = Number(range[1]);
      end = range[2] === '' ? size - 1 : Math.min(Number(range[2]), size - 1);
    } else {
      start = Math.max(size - Number(range[2]), 0);
      end = size - 1;
    }
    if (start >= size || start > end) {
      return new Response(null, { status: 416, headers: { 'Content-Range': `bytes */${size}` } });
    }
    headers.set('Content-Range', `bytes ${start}-${end}/${size}`);
    headers.set('Content-Length', String(end - start + 1));
    return new Response(body.slice(start, end + 1), { status: 206, headers });
  },
};
