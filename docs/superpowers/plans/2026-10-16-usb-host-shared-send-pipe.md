# USB host shared send pipe Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every bulk OUT endpoint on a host port shares one hardware pipe, which the RZ/A1L HAL
re-points per transfer. A hub can then carry 4 USB-MIDI devices that send and receive. Today
it carries 2, because each device takes two of the five bulk-capable pipes.

**Architecture:** Transparent to class drivers (`MidiHost` still calls `request_out`).
- **One shared pipe per port.** The first bulk OUT endpoint opened takes the highest free
  bulk pipe, searching the bulk-only pipes 5–3 before 2–1, so 1–2 stay free for isochronous
  audio where possible. Each later bulk OUT endpoint joins it as a user, and the pipe is
  released with its last user.
- **Per-user state.** Each user keeps its device address, endpoint number, MaxPacket and data
  toggle.
- **`xfer_out` on the shared pipe.** It runs under a per-port async lock. When the pipe points
  at another user, it re-points it:
  1. PID=NAK and wait for PBUSY=0;
  2. PIPECFG (EPNUM);
  3. PIPEMAXP (DEVSEL | MXPS);
  4. the toggle via SQSET or SQCLR from the user's saved state.

  After the transfer, the toggle is saved from SQMON. That is exact, unlike the original
  firmware's one-flip-per-send.
- **Unchanged:** interrupt-on-bulk OUT (Overbridge), isochronous OUT (UAC) and every IN pipe
  keep dedicated pipes.

**Tech Stack:** `rza1l-hal` (QEMU poked-MMIO tests), embassy-sync `Mutex`; `deluge-bsp` docs.

**Spec:** this plan's Architecture and Design notes, from the conversation of 2026-10-16. The
original firmware's mechanism is `change_destination_of_send_pipe`
(`~/GitHub/DelugeFirmware-clean/src/RZA1/usb/r_usb_basic/src/hw/r_usb_creg_abs.c:207`) with
`flushUSBMIDIToHostedDevice` (`src/deluge/io/midi/midi_engine.cpp:268`). The memory note
`usb-host-pipe-budget` explains why receive pipes must stay dedicated.

## Design notes

- **Why not pipe 1, as in the original firmware.** Pipes 1–2 are the only isochronous pipes. A
  send pipe there would take one from a USB audio interface. It isn't fixed at pipe 5 either:
  a device's IN endpoint opened first may already hold 5 (bulk allocation searches 5 down), so
  the shared pipe takes whichever bulk pipe is free when it is first needed.
- **Why save the toggle from SQMON.** The original flips its saved toggle once per send, which
  is wrong for any send longer than one packet. SQMON after the transfer is the controller's
  own state.
- **Why a lock.** The interrupt handlers track one transfer per pipe (`HCD_XFER[pipe]`), so two
  devices' sends must not overlap.
- **Toggle on (re)open.** A newly opened endpoint starts at DATA0, since configuration resets
  it. `reset_data_toggle` on a shared user zeroes its saved toggle, and also SQCLRs the pipe if
  that user currently owns it.
- **Cancellation.** If a send is dropped mid-transfer (a detach, or a re-bind's stop), a guard
  saves SQMON and forgets the owner, so the next send to any user re-points. The lock guard
  releases on drop.

## Global Constraints

- Only `EndpointType::Bulk` OUT endpoints share. Interrupt-on-bulk, isochronous, control and
  all IN endpoints are unchanged.
- The shared pipe's packet buffer is sized for 512-byte packets, double-buffered (16 blocks),
  so every user fits. MXPS is set per user on re-pointing.
- Bulk IN allocation is unchanged (highest free in 1–5). With one pipe shared for sending, 4
  bulk-capable pipes remain for receiving.

## Review Focus

1. **Two devices at the same address over time** (unplug A, plug B, which gets address 1).
   B's endpoint must start at DATA0 and not inherit A's toggle. Pinned by
   `a_reopened_endpoint_starts_at_data0`.
2. **The last user closing while another device's send is queued** must not free a pipe a send
   is using. The lock plus the user count cover it; review the release path.
3. **Full-speed devices behind a high-speed hub** (MaxPacket 64) and high-speed devices (512)
   sharing the pipe: MXPS must follow the user. Pinned by
   `repointing_sets_each_user_s_device_endpoint_and_size`.
4. **A send cancelled mid-transfer** must not leave a wrong saved toggle used silently. The
   guard re-reads SQMON; pinned by `a_cancelled_send_forgets_the_owner`.
5. **Pipe budget:** 4 MIDI devices on a hub (4 IN + 1 shared OUT + hub interrupt) must open.
   Pinned by `four_midi_devices_fit`.

---

### Task 1: Shared-pipe allocation

**Files:**
- Modify: `rza1/rza1l-hal/src/usb/host.rs` (`HcdAlloc`, `edpt_open`, `release_pipe`, tests)

**Interfaces:**
- Produces (module-private):
  - `struct SharedOut { pipe: u8, users: [Option<OutUser>; SHARED_OUT_USERS], owner: Option<u8> }`;
  - `struct OutUser { dev_addr: u8, epn: u8, mps: u16, toggle: bool }`;
  - `HcdAlloc { shared_out: Option<SharedOut>, .. }`;
  - `fn shared_user(&self, dev_addr, epn) -> Option<usize>`;
  - `const SHARED_OUT_USERS: usize = 8`;
  - `fn shared_pipe(port: usize) -> Option<usize>` (test helper reading `shared_out`).

- [ ] **Step 1: Write the failing tests**

```rust
    fn shared_pipe_of(dev: u8, ep: u8, h: &Rusb1HostDriver<'_>) -> usize {
        h.ep_to_pipe(dev, ep).unwrap()
    }

    fn the_shared_pipe() -> usize {
        shared_pipe(0).expect("a shared pipe is open")
    }

    #[test]
    fn bulk_out_endpoints_share_one_pipe() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = host_with_bus(0b00, DVSTCTR0_RHST_HS | DVSTCTR0_UACT);
        critical_section::with(|cs| unsafe { *HCD_ALLOC[0].borrow(cs).get() = HcdAlloc::new() });
        h.edpt_open(1, 0x01, EndpointType::Bulk, 512, 0).unwrap();
        h.edpt_open(2, 0x02, EndpointType::Bulk, 64, 0).unwrap();
        assert_eq!(shared_pipe_of(1, 0x01, &h), 5, "the highest free bulk pipe");
        assert_eq!(shared_pipe_of(2, 0x02, &h), 5);
        let used = critical_section::with(|cs| unsafe { (*HCD_ALLOC[0].borrow(cs).get()).pipes_used });
        assert_eq!(used, 0x0001 | (1 << 5), "one pipe for both");
    }

    #[test]
    fn four_midi_devices_fit() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = host_with_bus(0b00, DVSTCTR0_RHST_HS | DVSTCTR0_UACT);
        critical_section::with(|cs| unsafe { *HCD_ALLOC[0].borrow(cs).get() = HcdAlloc::new() });
        h.edpt_open(5, 0x81, EndpointType::Interrupt, 1, 12).unwrap(); // the hub's status pipe
        for dev in 1..=4u8 {
            h.edpt_open(dev, 0x81, EndpointType::Bulk, 64, 0).unwrap();
            h.edpt_open(dev, 0x01, EndpointType::Bulk, 64, 0).unwrap();
        }
        let mut pipes: std::vec::Vec<usize> = (1..=4).map(|d| h.ep_to_pipe(d, 0x81).unwrap()).collect();
        pipes.push(the_shared_pipe());
        pipes.sort();
        assert_eq!(pipes, [1, 2, 3, 4, 5], "4 receive pipes and the shared send pipe");
    }

    #[test]
    fn interrupt_on_bulk_and_iso_out_are_not_shared() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = host_with_bus(0b00, DVSTCTR0_RHST_HS | DVSTCTR0_UACT);
        critical_section::with(|cs| unsafe { *HCD_ALLOC[0].borrow(cs).get() = HcdAlloc::new() });
        crate::mmio::test::poke16(usb(0).devadd(1).addr(), DEVADD_USBSPD_HS);
        h.edpt_open(1, 0x01, EndpointType::Bulk, 512, 0).unwrap();
        h.edpt_open(1, 0x03, EndpointType::Interrupt, 88, 1).unwrap();
        h.edpt_open(1, 0x04, EndpointType::Isochronous, 56, 1).unwrap();
        assert_ne!(h.ep_to_pipe(1, 0x03).unwrap(), the_shared_pipe());
        assert_ne!(h.ep_to_pipe(1, 0x04).unwrap(), the_shared_pipe());
    }

    #[test]
    fn the_shared_pipe_is_freed_with_its_last_user() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = host_with_bus(0b00, DVSTCTR0_RHST_HS | DVSTCTR0_UACT);
        critical_section::with(|cs| unsafe { *HCD_ALLOC[0].borrow(cs).get() = HcdAlloc::new() });
        h.edpt_open(1, 0x01, EndpointType::Bulk, 512, 0).unwrap();
        h.edpt_open(2, 0x01, EndpointType::Bulk, 512, 0).unwrap();
        let shared = the_shared_pipe();
        h.edpt_close(1, 0x01);
        let used = || critical_section::with(|cs| unsafe { (*HCD_ALLOC[0].borrow(cs).get()).pipes_used });
        assert_ne!(used() & (1 << shared), 0, "device 2 still sends");
        assert_eq!(h.ep_to_pipe(1, 0x01), None);
        h.edpt_close(2, 0x01);
        assert_eq!(used() & (1 << shared), 0);
        assert_eq!(shared_pipe(0), None);
    }

    #[test]
    fn a_reopened_endpoint_starts_at_data0() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = host_with_bus(0b00, DVSTCTR0_RHST_HS | DVSTCTR0_UACT);
        critical_section::with(|cs| unsafe { *HCD_ALLOC[0].borrow(cs).get() = HcdAlloc::new() });
        h.edpt_open(1, 0x01, EndpointType::Bulk, 512, 0).unwrap();
        h.edpt_open(2, 0x01, EndpointType::Bulk, 512, 0).unwrap();
        critical_section::with(|cs| unsafe {
            let a = &mut *HCD_ALLOC[0].borrow(cs).get();
            let i = a.shared_user(1, 1).unwrap();
            a.shared_out.as_mut().unwrap().users[i].as_mut().unwrap().toggle = true;
        });
        h.edpt_close(1, 0x01);
        h.edpt_open(1, 0x01, EndpointType::Bulk, 512, 0).unwrap();
        let toggle = critical_section::with(|cs| unsafe {
            let a = &*HCD_ALLOC[0].borrow(cs).get();
            a.shared_out.as_ref().unwrap().users[a.shared_user(1, 1).unwrap()].unwrap().toggle
        });
        assert!(!toggle);
    }
```

The existing `a_digitakt_leaves_one_isochronous_pipe` test keeps passing unchanged. The
Overbridge IN takes 5, its interrupt-on-bulk OUT takes 4 and MIDI IN takes 3. The MIDI bulk OUT
opens the shared pipe on 2, the only bulk-capable pipe left, which leaves pipe 1 for one
isochronous stream.

- [ ] **Step 2: Run them to verify they fail**

```bash
cd rza1 && CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_RUNNER="qemu-arm -cpu cortex-a9 -L /usr/arm-linux-gnueabihf" \
  cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib usb::host 2>&1 | grep -E "^error" | sort | uniq -c
```
Expected: compile errors (`5`, `shared_user`, `shared_out` not found).

- [ ] **Step 3: Implement**

- **Types.** Add `SharedOut`/`OutUser` (both `Clone, Copy`) and the constants. Add
  `shared_out: None` to `HcdAlloc::new`. Add `HcdAlloc::shared_user(dev_addr, epn)`, which
  finds the matching user.
- **`edpt_open`.** At the top, inside its critical section and before `alloc_pipe`: if
  `ep_type == Bulk && !dir_in` and `shared_out` is `Some`, add a user:
  - record `{ dev_addr, epn, mps: layout.mxps, toggle: false }`;
  - map `ep_to_pipe` to the shared pipe;
  - return `Ok(())`;
  - if the user table is full, return `OutOfPipes`.

  If `shared_out` is `None`, allocate the highest free bulk pipe, trying 3–5 before 1–2
  (`alloc_pipe(3, 5)`, then `alloc_pipe(1, 2)`), with `OutOfPipes` if none is free. Reserve the packet buffer for **512** bytes double-buffered, configure the pipe for
  this endpoint as today, then set `shared_out = Some(SharedOut { pipe, users: [this, None, ..],
  owner: Some(0) })`.
- **`release_pipe`.** If `pipe == shared_out.pipe` and the endpoint is OUT:
  - remove that user (and if it was the owner, set `owner = None`);
  - clear the mapping only if it still equals `pipe` (already guarded);
  - free the hardware pipe (today's code) only when no users remain, then set
    `shared_out = None`.

  Otherwise return before touching the pipe's registers.

- [ ] **Step 4: Run, then commit**

Run the Step 2 command with `| tail -3`. Expected: all `usb::host` tests pass.

```bash
git add rza1/rza1l-hal/src/usb/host.rs
git commit -m "rza1l-hal: usb host bulk OUT endpoints share one pipe (allocation)"
```

---

### Task 2: Re-pointing the shared pipe per send

**Files:**
- Modify: `rza1/rza1l-hal/src/usb/host.rs` (`xfer_out`, `reset_data_toggle`, statics, tests)

**Interfaces:**
- Consumes: Task 1's `SharedOut`, `OutUser`, `shared_user`.
- Produces: `fn repoint_shared(u: Usb, pipe: usize, user: &OutUser)` and
  `fn save_shared_toggle(u: Usb, pipe: usize) -> bool`, plus a per-port
  `static SHARED_OUT_LOCK: [embassy_sync::mutex::Mutex<CriticalSectionRawMutex, ()>; 2]`.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn repointing_sets_each_user_s_device_endpoint_and_size() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let _h = host_with_bus(0b00, DVSTCTR0_RHST_HS | DVSTCTR0_UACT);
        let u = usb(0);
        repoint_shared(u, 5, &OutUser { dev_addr: 3, epn: 2, mps: 64, toggle: true });
        u.pipesel().write_value(5 as u16);
        assert_eq!(u.pipemaxp().read(), (3 << PIPEMAXP_DEVSEL_SHIFT) | 64);
        assert_eq!(u.pipecfg().read() & PIPECFG_EPNUM_MASK, 2);
        assert_ne!(u.pipecfg().read() & PIPECFG_DIR, 0, "OUT");
        let writes = crate::mmio::test::writes();
        let ctr = pipectr(u, 5).addr();
        assert!(writes.iter().any(|&(a, v)| a == ctr && v as u16 & PIPECTR_SQSET != 0), "DATA1 restored");
        repoint_shared(u, 5, &OutUser { dev_addr: 4, epn: 1, mps: 512, toggle: false });
        let writes = crate::mmio::test::writes();
        assert!(writes.iter().rev().any(|&(a, v)| a == ctr && v as u16 & PIPECTR_SQCLR != 0), "DATA0 restored");
    }

    #[test]
    fn the_toggle_is_saved_from_sqmon() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let _h = host_with_bus(0b00, DVSTCTR0_RHST_HS | DVSTCTR0_UACT);
        let u = usb(0);
        crate::mmio::test::poke16(pipectr(u, 5).addr(), PIPECTR_SQMON);
        assert!(save_shared_toggle(u, 5));
        crate::mmio::test::poke16(pipectr(u, 5).addr(), 0);
        assert!(!save_shared_toggle(u, 5));
    }

    /// A send cancelled mid-transfer leaves no owner, so the next send re-points.
    #[test]
    fn a_cancelled_send_forgets_the_owner() {
        use core::future::Future;
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = host_with_bus(0b00, DVSTCTR0_RHST_HS | DVSTCTR0_UACT);
        critical_section::with(|cs| unsafe { *HCD_ALLOC[0].borrow(cs).get() = HcdAlloc::new() });
        h.edpt_open(1, 0x01, EndpointType::Bulk, 512, 0).unwrap();
        h.edpt_open(2, 0x01, EndpointType::Bulk, 512, 0).unwrap();
        crate::mmio::test::poke16(usb(0).d0fifoctr().addr(), FIFOCTR_FRDY);
        {
            let data = [1u8; 4];
            let mut send = core::pin::pin!(h.xfer_out(2, 0x01, &data));
            let mut cx = core::task::Context::from_waker(core::task::Waker::noop());
            assert!(send.as_mut().poll(&mut cx).is_pending());
        }
        let owner = critical_section::with(|cs| unsafe {
            (*HCD_ALLOC[0].borrow(cs).get()).shared_out.unwrap().owner
        });
        assert_eq!(owner, None);
        assert!(SHARED_OUT_LOCK[0].try_lock().is_ok(), "the lock is released");
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run Task 1's Step 2 command. Expected: compile errors (`repoint_shared`, `save_shared_toggle`,
`SHARED_OUT_LOCK` not found).

- [ ] **Step 3: Implement**

```rust
/// Per port: serialises sends on the shared bulk OUT pipe (the interrupt handlers track one
/// transfer per pipe).
static SHARED_OUT_LOCK: [embassy_sync::mutex::Mutex<embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex, ()>; 2] =
    [const { embassy_sync::mutex::Mutex::new(()) }; 2];

/// Point the shared OUT pipe at `user`: halt it, then set its endpoint, device and size, and
/// restore the user's data toggle (TRM §28.3: PIPECFG/PIPEMAXP only change at PID=NAK with
/// PBUSY=0; SQSET/SQCLR likewise).
fn repoint_shared(u: Usb, pipe: usize, user: &OutUser) {
    let ctr = pipectr(u, pipe);
    ctr.write_value((ctr.read() & !PIPECTR_PID_MASK) | PIPECTR_PID_NAK);
    while ctr.read() & PIPECTR_PBUSY != 0 {}
    u.pipesel().write_value(pipe as u16);
    let cfg = u.pipecfg().read();
    u.pipecfg().write_value((cfg & !PIPECFG_EPNUM_MASK) | (user.epn as u16 & PIPECFG_EPNUM_MASK) | PIPECFG_DIR);
    u.pipemaxp().write_value(((user.dev_addr as u16) << PIPEMAXP_DEVSEL_SHIFT) | (user.mps & PIPEMAXP_MXPS_MASK));
    u.pipesel().write_value(0);
    ctr.write_value(PIPECTR_ACLRM);
    ctr.write_value(0);
    ctr.write_value(if user.toggle { PIPECTR_SQSET } else { PIPECTR_SQCLR });
}

/// The shared pipe's data toggle now (SQMON: true = DATA1).
fn save_shared_toggle(u: Usb, pipe: usize) -> bool {
    pipectr(u, pipe).read() & PIPECTR_SQMON != 0
}
```

In `xfer_out`, after looking up `pipe`, if `pipe` is the shared pipe:
1. `let _lock = SHARED_OUT_LOCK[p].lock().await;`.
2. In a critical section, find the user for `(dev_addr, epn)`. Missing → `Err(Canceled)`. If
   `owner != Some(i)`: `repoint_shared(u, pipe, &user)`, then `owner = Some(i)`.
3. Arm a guard (`OnDrop`) that, if dropped, re-reads SQMON into the user (if it still exists)
   and sets `owner = None`.
4. Run the existing transfer body, moved into a private `xfer_out_on(pipe, ..)` so both paths
   share it.
5. On completion, defuse the guard, save `save_shared_toggle` into the user, and return the
   result.

`reset_data_toggle` on the shared pipe: zero the user's saved toggle. If that user is the
owner, also do today's NAK + SQCLR (OUT re-arm). For that it needs `dev_addr` and `ep_addr`,
which `Rusb1Pipe` has.

Add `PIPECFG_DIR`, `PIPECTR_SQMON` and `PIPECFG_EPNUM_MASK` to the regs import if they're
missing.

- [ ] **Step 4: Run, rza1 suite, commit**

Run Task 1's Step 2 command with `| tail -3`, then `rza1/tools/test.sh`. Expected: all pass.

```bash
git add rza1/rza1l-hal/src/usb/host.rs
git commit -m "rza1l-hal: usb host re-points the shared send pipe per transfer, keeping each endpoint's data toggle"
```

---

### Task 3: BSP docs, hardware task, changelog

**Files:**
- Modify: `crates/deluge-bsp/src/usb/host.rs`: the `MAX_MIDI_DEVICES` doc. It should say 4
  bidirectional devices: four receive pipes plus one shared send pipe among bulk pipes 1–5;
  interrupt-endpoint MIDI
  (6 in the C firmware) is still a TODO.
- Modify: `docs/hardware-tasks.md` (new section)
- Modify: `CHANGELOG.md`
- Modify: memory `usb-host-pipe-budget.md` (record what the SDK now does)

- [ ] **Step 1: Docs**

`docs/hardware-tasks.md`:

```markdown
## Four MIDI devices on a hub (shared send pipe)

- **Setup:** any host build. A high-speed hub with four USB-MIDI devices, mixing full-speed
  (MaxPacket 64) and high-speed (512) if you have both. A sequence on the Deluge sends to all
  four while they send notes back.
- **Pass:** all four enumerate. Every device receives every note sent to it, with no stuck or
  repeated notes (a wrong data toggle shows up as silently dropped or duplicated packets). Notes
  from all four arrive. Unplugging one leaves the others working.
- **Record:** the devices used, and any device that drops notes.
```

CHANGELOG `### Changed`:

```markdown
- `rza1l-hal` USB host: every bulk OUT endpoint on a port shares one pipe, re-pointed
  per transfer with each endpoint's data toggle kept. A hub now carries 4 USB-MIDI devices that
  send and receive (it was 2). The shared pipe prefers the bulk-only pipes 3–5, keeping 1–2
  free for isochronous audio where it can.
```

- [ ] **Step 2: Suites and commit**

Run `rza1/tools/test.sh` and the BSP tests with `usb-host-uac,usb-host-overbridge`. Build
`demo-firmware`, `deluge-controller`, `uac-host-firmware` and `overbridge-host-firmware`.

```bash
git add crates/deluge-bsp/src/usb/host.rs docs/hardware-tasks.md CHANGELOG.md
git commit -m "docs: four MIDI devices on a hub with the shared send pipe"
```
