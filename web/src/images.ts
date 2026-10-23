/** Downloads guest images with byte-level progress and gzip decoding. */

import type { ImageFile, Manifest } from './protocol';

export interface LoadedImages {
  firmware: Uint8Array;
  kernel: Uint8Array;
  dtb?: Uint8Array;
  encodedBytes: number;
  decodedBytes: number;
}

export async function fetchManifest(url: string): Promise<Manifest> {
  const res = await fetch(url, { cache: 'no-cache' });
  if (!res.ok) throw new Error(`could not load ${url}: HTTP ${res.status}`);
  const manifest = (await res.json()) as Manifest;
  if (manifest.version !== 1 || !Array.isArray(manifest.files)) {
    throw new Error('unsupported image manifest');
  }
  return manifest;
}

export async function loadImages(
  manifest: Manifest,
  manifestUrl: string,
  onProgress: (loaded: number, total: number) => void,
): Promise<LoadedImages> {
  const total = manifest.files.reduce((n, f) => n + f.encodedSize, 0);
  let loaded = 0;
  const report = (n: number) => {
    loaded += n;
    onProgress(Math.min(loaded, total), total);
  };
  const files = await Promise.all(manifest.files.map((f) => fetchImage(f, manifestUrl, report)));
  const byRole = (role: ImageFile['role']) => files[manifest.files.findIndex((f) => f.role === role)];
  const firmware = byRole('firmware');
  const kernel = byRole('kernel');
  if (!firmware || !kernel) throw new Error('the image manifest needs a firmware and a kernel');
  return {
    firmware,
    kernel,
    dtb: byRole('dtb'),
    encodedBytes: total,
    decodedBytes: files.reduce((n, f) => n + f.byteLength, 0),
  };
}

async function fetchImage(file: ImageFile, base: string, report: (n: number) => void): Promise<Uint8Array> {
  const res = await fetch(new URL(file.url, base));
  if (!res.ok || !res.body) throw new Error(`could not load ${file.name}: HTTP ${res.status}`);
  let stream: ReadableStream<Uint8Array> = res.body.pipeThrough(
    new TransformStream<Uint8Array, Uint8Array>({
      transform(chunk, controller) {
        report(chunk.byteLength);
        controller.enqueue(chunk);
      },
    }),
  );
  if (file.encoding === 'gzip') {
    // A server may already have decoded the gzip layer (Content-Encoding), so check the
    // magic number instead of trusting the file name.
    const [isGzip, rest] = await peekGzipMagic(stream);
    stream = isGzip ? rest.pipeThrough(new DecompressionStream('gzip') as ReadableWritablePair<Uint8Array, Uint8Array>) : rest;
  }
  const data = await collect(stream, file.size);
  if (data.byteLength !== file.size) {
    throw new Error(`${file.name}: expected ${file.size} bytes, got ${data.byteLength}`);
  }
  return data;
}

async function peekGzipMagic(stream: ReadableStream<Uint8Array>): Promise<[boolean, ReadableStream<Uint8Array>]> {
  const reader = stream.getReader();
  const head: Uint8Array[] = [];
  let have = 0;
  while (have < 2) {
    const { done, value } = await reader.read();
    if (done) break;
    head.push(value);
    have += value.byteLength;
  }
  const first = new Uint8Array(have);
  let off = 0;
  for (const c of head) {
    first.set(c, off);
    off += c.byteLength;
  }
  const isGzip = first[0] === 0x1f && first[1] === 0x8b;
  const rest = new ReadableStream<Uint8Array>({
    start(controller) {
      if (first.byteLength) controller.enqueue(first);
    },
    async pull(controller) {
      const { done, value } = await reader.read();
      if (done) controller.close();
      else controller.enqueue(value);
    },
    cancel(reason) {
      return reader.cancel(reason);
    },
  });
  return [isGzip, rest];
}

async function collect(stream: ReadableStream<Uint8Array>, expected: number): Promise<Uint8Array> {
  let buf = new Uint8Array(expected);
  let len = 0;
  const reader = stream.getReader();
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    if (len + value.byteLength > buf.byteLength) {
      const grown = new Uint8Array(Math.max(buf.byteLength * 2, len + value.byteLength));
      grown.set(buf.subarray(0, len));
      buf = grown;
    }
    buf.set(value, len);
    len += value.byteLength;
  }
  return len === buf.byteLength ? buf : buf.slice(0, len);
}
