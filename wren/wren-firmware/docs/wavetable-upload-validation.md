# Device validation — inline wavetable upload

`upload_table` (`wren/wren-firmware/src/audio.rs`) builds a mip pyramid
**synchronously** inside the Wren `Wavetable.from` call, blocking the single
cooperative Embassy executor for the build. This is safe only if the build fits
inside the audio TX write-ahead lead (~5.8 ms under `audio-irq`, ~11.6 ms in the
default poll mode). That property can only be confirmed on hardware; the checklist
below is how to confirm it.

The `unsafe` soundness argument (single cooperative executor; `audio_task`'s render
closure and `upload_table` are both synchronous and never interleave; `ENGINE`
initialised once before any task; no ISR touches `ENGINE`) is documented in the
`## Concurrency` section of `audio.rs`.

## Checklist

- [ ] **Plays, not silence.** `var w = Wavetable.from([ ...one single cycle... ])`
      then `Out.patch(Osc.wavetable(w, 220))` → an audible tone (the built table),
      not silence.
- [ ] **No glitch on upload (poll mode, default).** Trigger an upload during
      playback → no audible click/dropout. If possible, timestamp around
      `upload_table` (GPIO toggle / cycle counter) and confirm the build is well
      under the ~11.6 ms poll write-ahead lead.
- [ ] **`audio-irq` mode (tighter, ~5.8 ms lead).** Rebuild with
      `cargo build-wren --features deluge-sdk/audio-irq` and repeat. Note any
      glitch. **Poll mode has ~2× the margin and is the safer default for
      upload-heavy patches.**
- [ ] **Reuse / no leak.** build → `Node.free()` → build again, several times →
      no pool exhaustion, no corruption, tables still correct. (The table's pool
      region is freed with its node, via `Cmd::Free`.)
- [ ] **Boot-script upload.** A `Wavetable.from` in `MAIN.WREN` plays (`ENGINE` is
      initialised in `main` before any task is spawned) and never crashes.

## If the inline build glitches

Switch to a deferred, chunked build: `Wavetable.from` returns a *pending* handle
immediately; spread the pyramid build across `vm_task` loop iterations (yielding to
`audio_task` between mip levels); bind the table when all levels are ready (the node
plays silence until then). This removes the long synchronous stall, at the cost of
an async-readiness state machine.
