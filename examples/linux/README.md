# Linux example apps

Native `libdeluge` apps for the Deluge's Linux userland, built with
`cargo deluge linux`. The build needs an unpacked `deluge-linux` release bundle
(its musl toolchain, `libdeluge` sysroot and `deluge-mkimage` packer): point
`DELUGE_BASE` at it.

    export DELUGE_BASE=/path/to/unpacked/bundle

`cargo deluge linux` packs a bootable appliance image (`target/<NAME>.ELF`, for
the SD card's `/APPS/`); `cargo deluge linux --bare` emits a plain static binary
(`target/bare/<name>`, for `/LINUX/APPS/`). Run `cargo deluge help` for the
other options.

## rust-app (minimal)

`examples/linux/rust-app` is the smallest possible app: it opens the device,
blinks an indicator LED a few times, and exits.

## The launcher

The launcher that lists and runs `/LINUX/APPS/` now lives in
[deluge-linux](https://github.com/FirestormAudio/deluge-linux): it is the app
slot of the `platform` image (`LINUX.ELF` in each release). Drop bare
app binaries (built with `cargo deluge linux --bare` from each app's own
directory) into the card's `/LINUX/APPS/`. SHIFT+TRIPLETS+LEARN, held about a
second, always returns to the list. See deluge-linux's
[docs/launcher.md](https://github.com/FirestormAudio/deluge-linux/blob/main/docs/launcher.md).

## Snake (pad-grid bare app)

`examples/linux/snake` is the reference *non-appliance* app: a static binary
for `/LINUX/APPS/` that the launcher exec's. It reads the pad grid and tempo
encoder and drives the pad LEDs + OLED. Build the bare binary from the
`snake/` directory:

    cargo deluge linux --bare      # -> target/bare/snake  for /LINUX/APPS/

Copy `target/bare/snake` onto the SD card's `/LINUX/APPS/`, boot the launcher,
and select `SNAKE`. Steer with the tempo encoder (turn = relative left/right);
eating food grows the snake and speeds it up; hitting a wall or yourself ends
the game — press the tempo encoder to restart. SHIFT+TRIPLETS+LEARN (held ~1s)
returns to the launcher.

> The bare binary must be copied to the card manually — the dev-mode USB upload
> only streams the appliance `LINUX.ELF` into RAM; it cannot place files on the
> card.

## Terminal (PTY shell bare app)

`examples/linux/terminal` is a bare `/LINUX/APPS/` app that runs an
interactive BusyBox shell (`/bin/sh -i`) on a pseudo-terminal and renders it
as a 25×6 character terminal on the OLED. Input comes from the on-pad QWERTY
keyboard (`deluge-grid-toolkit`'s `TextKeyboardComponent`, driven through the
`deluge-linux-ui::pads` bridge) plus front-panel keys. Build the bare binary
from the `terminal/` directory:

    cargo deluge linux --bare      # -> target/bare/terminal  for /LINUX/APPS/

Copy `target/bare/terminal` onto the card's `/LINUX/APPS/`, boot the launcher,
and select `TERMINAL`. Type on the pad keyboard; `Enter` runs the line. Shift
(on-pad or the `SHIFT` button) gives capitals and symbols. `BACK` sends Ctrl-C,
`SAVE` sends Ctrl-D (EOF), `LOAD` sends Tab; the `SELECT` encoder scrolls shell
history. On `exit`/Ctrl-D the OLED shows `[exit N] ENTER=restart`.
SHIFT+TRIPLETS+LEARN (held ~1s) returns to the launcher.

Notes:
- The app mounts `devpts` at `/dev/pts` itself on startup (the appliance rootfs
  does not) before `forkpty`; the child sets `PR_SET_PDEATHSIG(SIGKILL)` so the
  shell is reaped if the launcher force-kills the app.
- The reduced pad keyboard exposes the shell-critical symbols
  (`/ \ | ; : ? < > _ " ! @ # $ % ^ & * ( )`); `~` and `` ` `` are omitted (no
  free pad — the last slot collides with the SHIFT pad).
- Full-screen TUI programs (`vi`, `top`) are out of scope: only a minimal
  VT100 subset (cursor moves + erase) is emulated; colours are ignored (1bpp).
