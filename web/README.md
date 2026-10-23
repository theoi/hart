# hart web demo

The static site that boots hart in a browser tab. It uses xterm.js for the console and
runs the emulator (`crates/hart-web`, compiled to WebAssembly) in a Web Worker, so the page
stays responsive while the guest runs.

## Build

Needs Rust with the `wasm32-unknown-unknown` target, [wasm-pack](https://github.com/wasm-bindgen/wasm-pack)
0.15, and Node 24.

```sh
cd web
npm ci
npm run wasm                                   # crates/hart-web -> src/wasm (release, LTO)
npm run images -- --release guest-v1           # guest images -> public/images (gzip + manifest)
npm run build                                  # -> dist/
```

- `npm run images` downloads `fw_jump.bin` and `Image` from a GitHub release, checks them
  against the release's `SHA256SUMS`, gzips them, and writes `images/manifest.json`. Use
  `--dir <path>` for local files. For a private repository, set `GH_TOKEN`.
- The worker downloads the images with byte-level progress and decodes them with
  `DecompressionStream`. Release-asset URLs do not allow cross-origin fetches, so the site
  serves the images itself.
- `npm run wasm:stub` builds a scripted stand-in machine (the `stub` feature) that replays a
  boot log and runs a toy shell, for front-end work without a guest.

`npm run dev` starts a Vite dev server. To serve `dist/` the way GitHub Pages does, under
`/hart/`, run `node ../tests/web/serve.mjs`.

## Page API

The page exposes `window.hart` for the browser tests and for anyone curious: run state,
backend, boot time, instructions retired, speed, `promptVisible()`, `terminalText()`, and
`transcript()` (the raw console output).
