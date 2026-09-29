# wren-web

The wasm core for the Deluge Wren web editor/simulator.

It implements [`deluge_wren_core::Host`](../deluge-wren-core) against
in-memory buffers — the browser counterpart of the firmware's `FwHost` — and
exposes a small C-ABI surface to JS: boot the VM, run a script, inject input
(pads / buttons / encoders / MIDI), advance the clock, and drain output
(`System.print` / errors, OLED pixels, CV / gate / LED state, MIDI TX).

The wren C VM is compiled and linked against a **wasi-sdk sysroot**, so the
module has a few `wasi_snapshot_preview1` imports a tiny JS shim satisfies.
Audio-graph commands are applied to a main-thread engine (used for the on-screen
scope) and queued for a second engine instance running in an AudioWorklet, which
renders the audible output.

## Build

You need a wasi-sysroot once. Fetch + extract it anywhere, then point
`WASI_SYSROOT` at it:

```sh
# one-time: grab a wasi-sysroot (no system install needed)
curl -sL -o /tmp/wasi-sysroot.tar.gz \
  https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-25/wasi-sysroot-25.0.tar.gz
tar -xzf /tmp/wasi-sysroot.tar.gz -C /tmp

# build the wasm core
export WASI_SYSROOT=/tmp/wasi-sysroot-25.0
cargo build --target wasm32-unknown-unknown --release
# → target/wasm32-unknown-unknown/release/wren_web.wasm  (~230 KB)
```

The web editor in [`app/`](app/) loads the core from
`app/public/wren_web.wasm`; copy a fresh build there after changing this crate.

## Run the web editor

```sh
cd app
npm install
npm run dev        # Vite dev server; `npm run build` for a static bundle in dist/
```

## Test

A Node smoke test drives the module through Node's WASI shim:

```sh
node --experimental-wasi-unstable-preview1 \
  web/test.mjs target/wasm32-unknown-unknown/release/wren_web.wasm
```

It loads scripts and asserts OLED text rasterises, a MIDI note-on drives the
handler → CV jack, a metro fires a gate on tick, and compile errors are reported
with a line number.

## JS API

[`web/loader.mjs`](web/loader.mjs) wraps the C-ABI exports as a `Sim` object:

```js
import { loadSim } from "./loader.mjs";
const sim = await loadSim(wasmBytes, wasiImport, initWasi);

const r = sim.load(source);   // { ok, code, output, error, errorLine }
sim.midiIn(0x90, 60, 100);    // note-on
sim.tick(nowMs, dtSeconds);   // advance CV slew + metros
sim.oled();                   // { width, height, px: Uint8Array }  (one byte/pixel)
sim.cv(0); sim.gateBits(); sim.leds();
```

In the browser, supply a WASI shim (e.g. `@bjorn3/browser_wasi_shim`) in place of
`node:wasi`.
