#!/usr/bin/env node
// Prepares the guest images that the site serves from images/: downloads (or copies) the
// firmware and kernel, verifies their checksums, gzips them, and writes manifest.json.
//
//   node scripts/prepare-images.mjs --release guest-v1        # GitHub release assets
//   node scripts/prepare-images.mjs --dir ~/guest             # local files
//   node scripts/prepare-images.mjs --fake                    # synthetic images (stub builds)
//
// Options: --repo theoi/hart  --firmware fw_jump.bin  --kernel Image  --dtb <name>
//          --ram 256  --bootargs "console=ttyS0"  --out public/images
// Release downloads use GH_TOKEN or GITHUB_TOKEN when set (needed for private repos).

import { createHash } from 'node:crypto';
import { mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { parseArgs } from 'node:util';
import { gzipSync } from 'node:zlib';

const { values: opt } = parseArgs({
  options: {
    release: { type: 'string' },
    dir: { type: 'string' },
    fake: { type: 'boolean', default: false },
    repo: { type: 'string', default: 'theoi/hart' },
    firmware: { type: 'string', default: 'fw_jump.bin' },
    kernel: { type: 'string', default: 'Image' },
    dtb: { type: 'string' },
    ram: { type: 'string', default: '256' },
    bootargs: { type: 'string' },
    out: { type: 'string', default: new URL('../public/images', import.meta.url).pathname },
  },
});

const sources = [opt.release, opt.dir, opt.fake || undefined].filter((v) => v !== undefined);
if (sources.length !== 1) {
  console.error('choose exactly one of --release <tag>, --dir <path>, or --fake');
  process.exit(2);
}

const sha256 = (buf) => createHash('sha256').update(buf).digest('hex');

function parseSums(text) {
  const sums = new Map();
  for (const line of text.split('\n')) {
    const m = line.trim().match(/^([0-9a-f]{64})\s+\*?(.+)$/);
    if (m) sums.set(m[2], m[1]);
  }
  return sums;
}

async function fromRelease(tag, names) {
  const token = process.env.GH_TOKEN || process.env.GITHUB_TOKEN;
  const headers = { 'User-Agent': 'hart-web-build', 'X-GitHub-Api-Version': '2022-11-28' };
  if (token) headers.Authorization = `Bearer ${token}`;
  const rel = await fetch(`https://api.github.com/repos/${opt.repo}/releases/tags/${encodeURIComponent(tag)}`, {
    headers: { ...headers, Accept: 'application/vnd.github+json' },
  });
  if (!rel.ok) throw new Error(`release ${opt.repo}@${tag}: HTTP ${rel.status}`);
  const { assets } = await rel.json();
  const download = async (name) => {
    const asset = assets.find((a) => a.name === name);
    if (!asset) throw new Error(`release ${tag} has no asset named ${name}`);
    const res = await fetch(asset.url, { headers: { ...headers, Accept: 'application/octet-stream' } });
    if (!res.ok) throw new Error(`${name}: HTTP ${res.status}`);
    return Buffer.from(await res.arrayBuffer());
  };
  const files = new Map();
  for (const name of names) files.set(name, await download(name));
  const sums = assets.some((a) => a.name === 'SHA256SUMS') ? parseSums((await download('SHA256SUMS')).toString()) : null;
  return { files, sums };
}

async function fromDir(dir, names) {
  const files = new Map();
  for (const name of names) files.set(name, await readFile(join(dir, name)));
  let sums = null;
  try {
    sums = parseSums(await readFile(join(dir, 'SHA256SUMS'), 'utf8'));
  } catch {
    // No checksum file; nothing to verify against.
  }
  return { files, sums };
}

/** Deterministic stand-in images with realistic sizes and compressibility. */
function fakeImages(names) {
  let seed = 0x2545f491;
  const rand = () => {
    seed ^= seed << 13;
    seed ^= seed >>> 17;
    seed ^= seed << 5;
    return seed >>> 0;
  };
  const make = (size) => {
    const buf = Buffer.alloc(size);
    for (let off = 0; off < size; off += 4096) {
      const end = Math.min(off + 4096, size);
      if ((off >> 12) % 9 < 4) for (let i = off; i < end; i++) buf[i] = rand() & 0xff;
      else for (let i = off; i < end; i++) buf[i] = (i * 7) & 0x3f;
    }
    return buf;
  };
  const files = new Map([
    [names[0], make(275_904)],
    [names[1], make(26_313_728)],
  ]);
  return { files, sums: null };
}

const roles = [
  ['firmware', opt.firmware],
  ['kernel', opt.kernel],
  ...(opt.dtb ? [['dtb', opt.dtb]] : []),
];
const names = roles.map(([, name]) => name);

const { files, sums } = opt.release
  ? await fromRelease(opt.release, names)
  : opt.dir
    ? await fromDir(resolve(opt.dir), names)
    : fakeImages(names);

const outDir = resolve(opt.out);
await rm(outDir, { recursive: true, force: true });
await mkdir(outDir, { recursive: true });

const entries = [];
for (const [role, name] of roles) {
  const data = files.get(name);
  const hash = sha256(data);
  if (sums) {
    const want = sums.get(name);
    if (!want) throw new Error(`SHA256SUMS has no entry for ${name}`);
    if (want !== hash) throw new Error(`${name}: sha256 ${hash} does not match SHA256SUMS (${want})`);
  }
  const gz = gzipSync(data, { level: 9 });
  // Content-addressed names: Pages caches files for 10 minutes, and the manifest (fetched
  // with revalidation) must never pair with a stale image.
  const file = `${name}.${hash.slice(0, 12)}.gz`;
  await writeFile(join(outDir, file), gz);
  entries.push({ role, name, url: file, size: data.length, encodedSize: gz.length, encoding: 'gzip', sha256: hash });
  console.log(`${name}: ${data.length} bytes -> ${file} ${gz.length} bytes${sums ? ' (sha256 verified)' : ''}`);
}

const manifest = {
  version: 1,
  guest: opt.release ?? (opt.fake ? 'fake-images' : 'local'),
  ramMiB: Number(opt.ram),
  ...(opt.bootargs ? { bootargs: opt.bootargs } : {}),
  files: entries,
};
await writeFile(join(outDir, 'manifest.json'), JSON.stringify(manifest, null, 2) + '\n');
console.log(`wrote ${join(outDir, 'manifest.json')}`);
