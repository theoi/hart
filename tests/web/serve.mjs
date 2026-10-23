#!/usr/bin/env node
// Minimal static file server for the built site, used by the Playwright tests so boot
// timing excludes internet bandwidth. Serves the site under a path prefix like Pages does.
//
//   node serve.mjs [--root ../../web/dist] [--port 4173] [--host 127.0.0.1] [--prefix /hart/]

import { createReadStream } from 'node:fs';
import { stat } from 'node:fs/promises';
import { createServer } from 'node:http';
import { extname, join, normalize, resolve, sep } from 'node:path';
import { parseArgs } from 'node:util';

const { values: opt } = parseArgs({
  options: {
    root: { type: 'string', default: new URL('../../web/dist', import.meta.url).pathname },
    port: { type: 'string', default: '4173' },
    host: { type: 'string', default: '127.0.0.1' },
    prefix: { type: 'string', default: '/hart/' },
  },
});

const root = resolve(opt.root);
const prefix = opt.prefix.endsWith('/') ? opt.prefix : `${opt.prefix}/`;

const types = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.wasm': 'application/wasm',
  '.gz': 'application/gzip',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.ico': 'image/x-icon',
  '.woff2': 'font/woff2',
  '.txt': 'text/plain; charset=utf-8',
};

const server = createServer(async (req, res) => {
  const url = new URL(req.url ?? '/', 'http://localhost');
  if (url.pathname === prefix.slice(0, -1) || url.pathname === '/') {
    res.writeHead(302, { Location: prefix });
    res.end();
    return;
  }
  if (!url.pathname.startsWith(prefix)) {
    res.writeHead(404).end('not found');
    return;
  }
  let rel = decodeURIComponent(url.pathname.slice(prefix.length));
  if (rel === '' || rel.endsWith('/')) rel += 'index.html';
  const file = normalize(join(root, rel));
  if (file !== root && !file.startsWith(root + sep)) {
    res.writeHead(403).end('forbidden');
    return;
  }
  try {
    const st = await stat(file);
    if (!st.isFile()) throw new Error('not a file');
    res.writeHead(200, {
      'Content-Type': types[extname(file)] ?? 'application/octet-stream',
      'Content-Length': st.size,
      'Cache-Control': 'no-cache',
    });
    if (req.method === 'HEAD') res.end();
    else createReadStream(file).pipe(res);
  } catch {
    res.writeHead(404).end('not found');
  }
});

server.listen(Number(opt.port), opt.host, () => {
  console.log(`serving ${root} at http://${opt.host}:${opt.port}${prefix}`);
});
