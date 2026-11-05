# FAQ

## Which Deluges are supported?

The **OLED Deluge** only. The original seven-segment Deluge is not supported, and the app
loader and the SDK have not been tested on one.

## What does "developer preview" mean?

The SDK works, and the examples run on hardware, but the API is not stable yet: until 1.0,
a new release may change it in ways that need your app's code to change. The crates are not on
crates.io yet, so an app depends on this repository by path or over git (`cargo deluge new`
sets that up). Breaking changes are listed in the [changelog](../CHANGELOG.md).

## Can an app damage my Deluge?

An app runs from RAM, and a crash or hang is one power cycle away from the boot menu. Holding
LOAD at power-on always reaches the menu, whatever the auto-boot setting.

An app has full control of the hardware while it runs, so one that drives the HAL directly
can write to the SPI flash. Even then, the stock first-stage bootloader can reinstall the
loader or any firmware from the SD card. For a unit that runs untrusted apps, a loader built
with [bootloader write protection](app-loader.md#bootloader-write-protection) locks both
bootloaders in the flash chip before it launches an app.

## Do I need a debug probe?

No. With the app loader's dev mode on, `cargo deluge run` uploads over USB, and the
`usb-log` feature sends `log` output over the same cable. A J-Link adds RTT logging,
breakpoints, and Cortex-A9 instruction trace; see the
[advanced guide](advanced-guide.md#9-debugging--tracing-with-the-probe-rs-cortex-a9-fork).

## Do I need a Deluge at all?

Not to start. `cargo deluge sim` runs the same app source on your computer in the
[desktop simulator](simulator.md), with the front panel, audio, MIDI, and a host directory
standing in for the SD card.

## How do I go back to the stock or Community firmware?

Install it the way you installed the app loader: put the firmware's `.bin` on the SD card
and hold SHIFT while powering on. The app loader and the SDK's normal workflows never change
the first-stage bootloader, so this always works. See
[Installing the loader](app-loader.md#installing-the-loader).

## Why nightly Rust?

The device target has no prebuilt standard library, so the core library is built from source
(`-Zbuild-std`), and Embassy's task macro needs `impl_trait_in_assoc_type`. The repository
pins one nightly in [`rust-toolchain.toml`](../rust-toolchain.toml), and `rustup` installs it
on first use.

## Which host operating systems work?

The SDK and `cargo deluge` are developed and tested on Linux. The simulator and the build
itself are portable Rust; USB upload and serial logging need a serial port that the
`serialport` crate can open.

## How is the SDK licensed?

The SDK and its core libraries are `MIT OR Apache-2.0`, so your app can use any license. The
optional OLED UI toolkit, its fonts and the pad-grid toolkit are `GPL-3.0-or-later`; an app
that uses them becomes GPL. The facade does not depend on them. See
[Licensing](../README.md#licensing).

## Can I write apps for the Deluge's Linux userland instead?

Yes: the `linux` backend runs the same app on Linux on the Deluge, through `libdeluge`.
It needs a `deluge-linux` release bundle; see [the Linux examples](../examples/linux/README.md).

## Where do I report a bug or ask a question?

Open an issue on [GitHub](https://github.com/FirestormAudio/deluge-sdk/issues). For changes,
see [CONTRIBUTING.md](../CONTRIBUTING.md).
