// A static server for the e2e checks: serves the directory `root` at the
// path `base` on 127.0.0.1, `index.html` for the base itself.

import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { extname, join, normalize } from 'node:path';

const types = {
  '.html': 'text/html',
  '.js': 'text/javascript',
  '.wasm': 'application/wasm',
  '.css': 'text/css',
  '.json': 'application/json',
  '.svg': 'image/svg+xml',
};

/** Starts serving, and resolves to the server and the origin with `base`. */
export async function serve(root, base = '/') {
  const server = createServer(async (request, response) => {
    const path = normalize(decodeURIComponent(new URL(request.url, 'http://x').pathname));
    const inside = path.startsWith(base) ? path.slice(base.length) : null;
    const file = join(root, inside === '' ? 'index.html' : (inside ?? 'missing'));
    try {
      const body = await readFile(file);
      response.writeHead(200, { 'content-type': types[extname(file)] ?? 'application/octet-stream' });
      response.end(body);
    } catch {
      response.writeHead(404).end();
    }
  });
  await new Promise((ready) => server.listen(0, '127.0.0.1', ready));
  return { server, origin: `http://127.0.0.1:${server.address().port}${base}` };
}
