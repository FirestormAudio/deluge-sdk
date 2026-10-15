# RZ/A1 Phase 5b: DMA, Audio, SD, Flash and USB Take Their Registers from the PAC Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the remaining register users in `rza1l-hal` onto `rza1-pac` and meet the Phase 5 exit criterion: no hand-written register tables left in the HAL. The remaining users are DMAC, SSI, SCUX, SDHI, SPIBSC and USB.

**Architecture:**
- **New PAC shapes.** `rza1-pac` gains shape transforms for:
  - DMAC: per-channel blocks, plus the group control and resource-selector arrays;
  - SCUX: per-sub-unit blocks;
  - USB: register arrays and FIFO byte views.
- **SDHI is added by hand.** It is a hand-written block for the RZ/A1L, from the A1L manual's Table 46.1 (the vendor SVD omits it). The RZ/A1L manual check then covers SDHI too, and it learns to accept byte views of registers that allow byte access.
- **One module per task, as in 5a.** Pin the PAC's addresses and widths against the old table, swap the table for PAC handles, and keep the existing sequence tests passing.
- **SCUX, SPIBSC and USB come onto the seam.** They currently bypass the MMIO seam with raw volatile pointers, so they become host-testable as they move.
- **The USB register struct goes.** `Rusb1Regs` is pinned field by field against the PAC, then deleted once nothing uses it.

**Tech Stack:** Rust nightly (edition 2024), chiptool `bcf538a2e7b8584ae874ee9ab72efb1576fc6152`, Python 3 with PyYAML (the generator's helpers), the MMIO test seam (`rza1_pac::mmio`), QEMU ARM host tests (`armv7-unknown-linux-gnueabihf`), `armv7a-none-eabihf` firmware builds.

**Spec:** `docs/superpowers/specs/2026-10-14-rza1-hal-architecture-design.md`, Phase 5, §3 Decisions, and its "5a done" paragraph. Phase 5a is merged into local `main` (`556a4534`). The plan it was written from is `docs/superpowers/plans/2026-10-15-rza1-phase5a-pac-and-small-blocks.md`; read its "Decisions settled before execution" for the conventions this plan continues.

**Reference:** the RZ/A1L hardware manual as `vendor/docs/rza1/rza1l-hw-manual/*.md`. Table 46.1 is in `ch46-list-of-registers.md`, and its legend reads "Note 1. RZ/A1L only, Note 2. RZ/A1LU only". The Deluge's chip is the RZ/A1L R7S721020, and `vendor/rza1.svd` is the RZ/A1H's.

## Global Constraints

- **Spec Phase 5 exit:** "no `const OFF_*`/`*_BASE` register tables left in `rza1l-hal`." Task 9 makes `tools/check-register-tables.sh` hold every HAL module to it. Two exceptions are named there: `memmap.rs` and `mmu.rs`, which describe memory, not registers.
- **Spec, Phase 5:**
  - "Hand-write YAML for any block the SVD gets wrong (expect USB and SCUX)."
  - "Before deleting each hand-written offset table, add a test that the PAC's addresses equal the old constants."
  - "Migrate one driver per task."
- **Spec §3:** "Keep the MMIO test seam (`mmio.rs`) through every phase."
- **Settled in 5a and carried forward:**
  - **The vendor SVD is never edited.** Corrections are transforms under `crates/rza1-pac/transforms/`.
  - **Generated files are never edited by hand:** `src/chips/*.rs` and `transforms/rza1l/absent.yaml`. Change a transform and run `tools/gen-pac.sh`.
  - **Fieldset newtypes are used as plain values**, e.g. `X(v)` and `.read().0`.
  - **Drivers keep their bit constants.**
  - **Access calls map one to one:** `write_value`/`read` exactly where the old code wrote and read, and `modify` only where it already read-modified-wrote.
  - **Formatting:** only the lines a task touches. Pre-existing rustfmt drift stays.
- **The A1H PAC:** `rza1h` gets the shared shape transforms (DMAC, SCUX, USB) but not SDHI. There is no RZ/A1H manual in the repo to source SDHI from, and no RZ/A1H user.
- **No memory-layout change:** `tools/link-layout.sh check` passes after every task.
- **Checks:**
  - PAC tests: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1-pac --features rza1l --lib`, and the same with `--features rza1h`.
  - HAL tests: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib`.
  - BSP tests: `cargo test --target armv7-unknown-linux-gnueabihf -p deluge-bsp --features usb-host --lib`.
  - Suite: `tools/test.sh`, exit 0. It runs `tools/check-pac.sh`, the drift and manual check.
- **Firmware build check**, run at the end of every task:
  ```bash
  for p in app-loader deluge-controller demo-firmware msc-firmware wp-probe vbus-probe sd-bench uac-host-firmware wren-firmware; do printf '%s: ' $p; cargo build-fw -p $p 2>&1 | tail -1; done
  tools/build-examples.sh > /tmp/claude-1000/p5b-examples.log 2>&1; echo "examples exit $?"
  ```
  Expected: nine `Finished` lines and `examples exit 0`, with 38 distinct warnings across the logs, as at `main`.
- **DelugeFirmware** builds against this working copy through its gitignored `local-sdk.toml`. Its device check runs at the end of Tasks 2, 5, 8 and 9:
  ```bash
  cd /home/kate/GitHub/DelugeFirmware/src/bsp/rust && cargo check --target armv7a-deluge-eabihf.json -Zbuild-std=core,alloc -Zjson-target-spec --no-default-features --features rtt --config local-sdk.toml; git -C /home/kate/GitHub/DelugeFirmware checkout src/bsp/rust/Cargo.lock
  ```
  Expected: `Finished`, with only its existing `oled_display_off` warning.
- **Branch:** `refactor/rza1-pac-5b`, cut from `main` (`556a4534`). Never push.

## Decisions settled before execution

1. **The new shapes were prototyped during planning.** `chiptool generate` ran on the RZ/A1L chip with all of Task 1's transforms. The manual check reported 3 183 PAC registers against 3 481 RZ/A1L registers, with 335 not covered (the debugger interface only) and 0 problems. The arithmetic reconciles exactly: 3 146 covered manual registers, plus 7 GPIO port-0 holes, plus 18 unaligned FIFO byte lanes, gives 3 171 distinct addresses. RSPI's `SPDR` views and USB's aligned FIFO byte views share an address with the register they view, which brings the count to 3 183.
2. **The shapes:**
   - **DMAC:**
     - `DMAC.ch(n)` (n = 0–15) is a block of `n0sa, n0da, n0tb, n1sa, n1da, n1tb, crsa, crda, crtb, chstat, chctrl, chcfg, chitvl, chext, nxla, crla`. The array is cursed, because channels 8–15 sit at `0x400 + (n − 8) × 0x40`.
     - `DMAC.dctrl(g)` and `dstat_{en,er,end,tc,sus}(g)` take g = 0 for channels 0–7 and 1 for channels 8–15.
     - `DMAC.dmars(i)` (i = 0–7) is the resource selector for channels 2i and 2i+1, at `0xFCFE_1000 + 4i`. That is far from the DMAC block, but chiptool handles the offset.
   - **SCUX:**
     - `SCUX.ipc(n)`, `opc(n)`, `ffd(n)`, `ffu(n)` and `dvu(n)` each take n = 0–3, at a stride of 0x100.
     - `SCUX.src(i)` is the 2SRC converter `i = 2·unit + pair`, at offsets `[0x1000, 0x1034, 0x1100, 0x1134]`, so the array is cursed.
     - `SCUX.srcirr(unit)` and `SCUX.mix()` (a block) cover the rest.
     - The CIM's `swrsr_cim`, `dmacr_cim`, `ssirsel_cim`, `ssipmd_cim`, `ssictrl_cim` and `mixrsel_cim` stay flat. Its selector arrays are `dmatd(n)`, `dmatu(n)`, `fdtsel(n)`, `futsel(n)` and `srcrsel(n)`.
     - `dvu(n).volr(k)` (k = 0–7) gives the eight volume registers.
   - **USB:**
     - `USB200.intenb(n)` and `intsts(n)` take n = 0–1.
     - `pipectr(n)` is pipe n+1's control register; pipe 0's is `dcpctr`.
     - `devadd(n)` takes n = 0–10.
     - `cfifo_b(i)`, `d0fifo_b(i)` and `d1fifo_b(i)` are the byte at offset i of each 32-bit FIFO port.
     - The per-pipe TRE/TRN pairs stay flat (`pipe1tre` …): the hardware orders them 1–5, B–F, 9, A, and nothing uses them.
     - Every other register keeps the SVD's name in Rust casing, digits included (`SYSCFG0` → `syscfg0`, `DVSTCTR0` → `dvstctr0`); only the arrayed ones drop theirs (`INTENB0` → `intenb(0)`).
   - **SDHI (RZ/A1L only):** `SDHI0` and `SDHI1` are `sdhi::Sdhi` handles. Their registers carry Table 46.1's names, without the channel suffix and in Rust casing: `sd_cmd`, `sd_arg0`, `sd_arg1`, … `sd_buf0` (32-bit), …, `ext_swap`.
3. **Byte and halfword views pass the manual check when the viewed register allows that width.** For a PAC register at an address the manual doesn't list, the check looks at the 32-bit-aligned register containing it, and accepts the view if that register's access sizes include the view's width. This covers RSPI's `SPDR` views from 5a and USB's FIFO byte views. A view of a register that doesn't allow the width still fails.
4. **SCUX, SPIBSC and USB lose their raw volatile helpers.**
   - SCUX: `scux/regs.rs`'s `ipc()`/`opc()`/… returned `*mut u32`, and `scux.rs` called `write_volatile` on them.
   - SPIBSC: its private `wr32`/`rd32`.
   - USB: `usb::regs::{rd, wr, rmw, rd32, wr32}` on `Rusb1Regs` fields.

   On the device the PAC compiles to the same volatile access. On the host it goes through the seam, which is why each of those tasks starts with a sequence test that crashes before the migration.
5. **USB migrates in two tasks.**
   - **Task 7** gives the HAL a `fn usb(port: u8) -> pac::usb::Usb`, moves `regs.rs`'s helpers, `fifo.rs`, `pipe.rs` and `mod.rs` onto it, and pins every `Rusb1Regs` field's offset against the PAC.
   - **Task 8** moves `driver.rs`, `host.rs` and `hw-tests/vbus-probe`, then deletes `Rusb1Regs`, `rd`/`wr`/`rmw`/`rd32`/`wr32`, `pipectr_ptr` and `devadd_ptr`. Its pin test switches to literal offsets.
   - `usb::regs` keeps its bit-mask constants (`SYSCFG_DPRPU`, `INTSTS0_VBSTS`, …), because drivers and `vbus-probe` name them.
6. **CPG registers in these modules go through `CPG`:** SSI's `SWRSTCR1`, SDHI's `STBCR12` and USB's `STBCR7`. Dummy reads stay where they are.
7. **Exit criterion.** Task 9 inverts `tools/check-register-tables.sh`. Instead of a `MIGRATED` list it checks every `.rs` under `crates/rza1l-hal/src`, except `memmap.rs` (the memory map's address ranges) and `mmu.rs` (the translation table in RAM).

## Review Focus

1. **USB FIFO width and lane order.** Bulk data moves 32 bits at a time through `cfifo`/`d0fifo`/`d1fifo`. A packet's tail moves a byte at a time to lane `3 − (i & 3)`, as the existing `sw_to_hw_tail_uses_bigend0_lanes` test pins. A wrong width or lane would corrupt every USB packet whose length isn't a multiple of 4. Task 7 rewrites the FIFO tests onto the seam, and they assert each access's address and width.
2. **Sequences that used to bypass the seam.** SCUX's software reset (write, dummy read, write, dummy read), USB's read-modify-writes (`rmw`) and SPIBSC's manual-mode transfer have never been host-tested. Each of Tasks 4, 6 and 7 opens with a sequence test that crashes before the migration and pins the order after.
3. **DMAC channels 8–15.** They sit 0x200 past a uniform stride. A wrong array would program channel 8's registers into channel 7's neighbours, which is the SD card's and the PIC's DMA on the Deluge. Task 2's pin covers all 16 channels.
4. **ISR-path cost.** USB's `dcd_int_handler`/`hcd_int_handler`, the ISO hooks and SCUX's position reads run in interrupt context. As in 5a, every PAC access must inline. Task 8 checks a release `deluge-controller` for any `rza1_pac::common` or `rza1_pac::mmio` symbol.
5. **Outside users of the USB internals.** `hw-tests/vbus-probe` reads `Rusb1Regs` directly, and DelugeFirmware builds against this tree. Task 8 moves vbus-probe and runs the DelugeFirmware device check.

---

## File Structure

| Path | Change |
|------|--------|
| `crates/rza1-pac/transforms/common/{dmac,scux,usb}.yaml` | New shape transforms (Task 1) |
| `crates/rza1-pac/transforms/rza1l/sdhi.yaml` | Hand-written SDHI block, generated once from Table 46.1 (Task 1) |
| `crates/rza1-pac/transforms/{rza1l,rza1h}/chip.yaml` | Include them |
| `crates/rza1-pac/src/chips/*.rs`, `transforms/rza1l/absent.yaml` | Regenerated |
| `crates/rza1-pac/src/lib.rs` | Shape tests |
| `tools/pac-check-a1l.py` | Views rule; SDHI no longer excepted |
| `crates/rza1l-hal/src/{dmac.rs,dmac/raw.rs}` | Task 2 |
| `crates/rza1l-hal/src/ssi.rs` | Task 3 |
| `crates/rza1l-hal/src/{scux.rs,scux/regs.rs,scux/*_path.rs}` | Task 4 |
| `crates/rza1l-hal/src/sdhi.rs` | Task 5 |
| `crates/rza1l-hal/src/spibsc.rs` | Task 6 |
| `crates/rza1l-hal/src/usb/{regs,fifo,pipe,mod}.rs` | Task 7 |
| `crates/rza1l-hal/src/usb/{driver,host,regs}.rs`, `hw-tests/vbus-probe/src/tasks/probe.rs` | Task 8 |
| `tools/check-register-tables.sh`, `CHANGELOG.md`, READMEs, the spec | Task 9 |

---

### Task 1: The PAC's shapes for DMAC, SCUX, USB and SDHI

**Files:** listed in the File Structure table under Task 1.

**Interfaces:**
- Produces, under `rza1l`:
  - `DMAC.ch(0..16)`, a `dmac::Ch` block with the registers listed in Decision 2;
  - `DMAC.dctrl(0..2)`, `DMAC.dstat_{en,er,end,tc,sus}(0..2)` and `DMAC.dmars(0..8)`;
  - `SCUX.{ipc,opc,ffd,ffu,dvu}(0..4)`, `SCUX.src(0..4)`, `SCUX.srcirr(0..2)` and `SCUX.mix()`;
  - `SCUX.{dmatd,dmatu,fdtsel,futsel,srcrsel}(0..4)`;
  - `scux::Dvu::volr(0..8)`;
  - `USB200/USB201.{intenb,intsts}(0..2)`, `.pipectr(0..15)`, `.devadd(0..11)` and `.{cfifo_b,d0fifo_b,d1fifo_b}(0..4)`;
  - `SDHI0`/`SDHI1`: `sdhi::Sdhi`.
- `rza1h` produces the same apart from SDHI.

- [ ] **Step 1: Branch, and write the shape tests first**

```bash
cd /home/kate/GitHub/deluge-sdk && git checkout -b refactor/rza1-pac-5b main
```

Append to `crates/rza1-pac/src/lib.rs`'s `mod tests`:

```rust
    #[test]
    fn dmac_channels_8_to_15_sit_past_the_group_gap() {
        assert_eq!((DMAC.ch(0).n0sa().addr(), DMAC.ch(0).n0sa().width()), (0xE820_0000, 4));
        assert_eq!(DMAC.ch(7).chctrl().addr(), 0xE820_0000 + 7 * 64 + 0x28);
        assert_eq!(DMAC.ch(8).chctrl().addr(), 0xE820_0000 + 8 * 64 + 0x200 + 0x28);
        assert_eq!(DMAC.ch(15).nxla().addr(), 0xE820_0000 + 15 * 64 + 0x200 + 0x38);
        assert_eq!(DMAC.dctrl(0).addr(), 0xE820_0300);
        assert_eq!(DMAC.dctrl(1).addr(), 0xE820_0700);
        assert_eq!(DMAC.dmars(3).addr(), 0xFCFE_100C);
    }

    #[test]
    fn scux_sub_units_are_indexed_by_unit() {
        assert_eq!(SCUX.ipc(2).ipslr().addr(), 0xE820_8000 + 0x200 + 0x04);
        assert_eq!(SCUX.ffd(1).drqsr().addr(), 0xE820_8800 + 0x100 + 0x08);
        assert_eq!(SCUX.ffu(3).urqsr().addr(), 0xE820_8C00 + 0x300 + 0x08);
        // 2SRC converter 2·unit + pair: unit stride 0x100, pair stride 0x34.
        assert_eq!(SCUX.src(0).srcir().addr(), 0xE820_9000);
        assert_eq!(SCUX.src(1).srcir().addr(), 0xE820_9034);
        assert_eq!(SCUX.src(3).ifsvr().addr(), 0xE820_9000 + 0x100 + 0x34 + 0x10);
        assert_eq!(SCUX.dvu(1).volr(7).addr(), 0xE820_9200 + 0x100 + 0x24 + 7 * 4);
        assert_eq!(SCUX.swrsr_cim().addr(), 0xE820_9700);
    }

    #[test]
    fn usb_fifo_ports_have_byte_views_over_their_lanes() {
        assert_eq!((USB200.cfifo().addr(), USB200.cfifo().width()), (0xE801_0014, 4));
        assert_eq!((USB200.cfifo_b(3).addr(), USB200.cfifo_b(3).width()), (0xE801_0017, 1));
        assert_eq!(USB201.d1fifo_b(0).addr(), 0xE820_701C);
        assert_eq!(USB200.pipectr(0).addr(), 0xE801_0070); // PIPE1CTR
        assert_eq!(USB200.pipectr(14).addr(), 0xE801_008C); // PIPEFCTR
        assert_eq!(USB200.devadd(10).addr(), 0xE801_00D0 + 20); // DEVADDA
        assert_eq!(USB200.intsts(1).addr(), 0xE801_0042);
    }

    #[cfg(feature = "rza1l")]
    #[test]
    fn sdhi_comes_from_the_rza1l_manual() {
        assert_eq!((SDHI0.sd_cmd().addr(), SDHI0.sd_cmd().width()), (0xE804_E000, 2));
        assert_eq!((SDHI1.sd_buf0().addr(), SDHI1.sd_buf0().width()), (0xE804_E830, 4));
        assert_eq!(SDHI1.ext_swap().addr(), 0xE804_E8F0);
    }
```

The SCUX addresses come from `scux/regs.rs`'s constants:
- `SCUX_BASE` is 0xE820_8000, with IPC at +0x000, OPC at +0x400, FFD at +0x800 and FFU at +0xC00, each at a stride of 0x100.
- `SRC_BASE` is 0xE820_9000 (unit stride 0x100, pair stride 0x34).
- `DVU_BASE` is 0xE820_9200.
- `CIM_BASE` is 0xE820_9700.

Before running the test, confirm each one against `scux/regs.rs`, and fix the test, not the transform, if the plan copied a value wrong.

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1-pac --features rza1l --lib 2>&1 | grep -E "^error" | sort | uniq -c`
Expected: compile errors (`no method named ch`, `no function or associated item named sdhi`, …).

- [ ] **Step 2: The transforms**

`crates/rza1-pac/transforms/common/dmac.yaml`:

```yaml
transforms:
  # The SVD names DMAC's per-channel registers by channel (`N0SA_3`): gather each channel's into
  # a block, then index the blocks by channel. Channels 8–15 sit 0x200 above where a uniform stride
  # would put them, so the array is cursed (explicit offsets).
  - !MakeBlock
    blocks: '^DMAC::DMAC$'
    from: '^(N0SA|N0DA|N0TB|N1SA|N1DA|N1TB|CRSA|CRDA|CRTB|CHSTAT|CHCTRL|CHCFG|CHITVL|CHEXT|NXLA|CRLA)_(\d+)$'
    to_outer: 'CH$2'
    to_block: 'DMAC::CH'
    to_inner: '$1'
  - !MakeRegisterArray
    blocks: '^DMAC::DMAC$'
    from: '^CH\d+$'
    to: CH
    mode: Cursed
  # The two channel groups' control and status registers (channels 0–7, 8–15), and the eight
  # resource selectors (one per channel pair).
  - !MakeRegisterArray
    blocks: '^DMAC::DMAC$'
    from: '^(DCTRL|DSTAT_EN|DSTAT_ER|DSTAT_END|DSTAT_TC|DSTAT_SUS)_\d+_\d+$'
    to: '$1'
  - !MakeRegisterArray
    blocks: '^DMAC::DMAC$'
    from: '^DMARS\d+$'
    to: DMARS
```

`crates/rza1-pac/transforms/common/scux.yaml`:

```yaml
transforms:
  # SCUX's sub-units are named by unit (`IPCIR_IPC0_2` is IPC unit 2's IPCIR): gather each unit's
  # registers into a block and index the blocks by unit, one array per sub-unit type.
  - !MakeBlock
    blocks: '^SCUX::SCUX$'
    from: '^(IPCIR|IPSLR)_IPC0_(\d)$'
    to_outer: 'IPC$2'
    to_block: 'SCUX::IPC'
    to_inner: '$1'
  - !MakeBlock
    blocks: '^SCUX::SCUX$'
    from: '^(OPCIR|OPSLR)_OPC0_(\d)$'
    to_outer: 'OPC$2'
    to_block: 'SCUX::OPC'
    to_inner: '$1'
  - !MakeBlock
    blocks: '^SCUX::SCUX$'
    from: '^(FFDIR|FDAIR|DRQSR|FFDPR|FFDBR|DEVMR|DEVCR)_FFD0_(\d)$'
    to_outer: 'FFD$2'
    to_block: 'SCUX::FFD'
    to_inner: '$1'
  - !MakeBlock
    blocks: '^SCUX::SCUX$'
    from: '^(FFUIR|FUAIR|URQSR|FFUPR|UEVMR|UEVCR)_FFU0_(\d)$'
    to_outer: 'FFU$2'
    to_block: 'SCUX::FFU'
    to_inner: '$1'
  # 2SRC: two units (m) of two converters each (p): `SRCIRp_2SRC0_m`. One block per converter,
  # indexed 2*m + p (unit stride 0x100, converter stride 0x34, so the array is cursed).
  - !MakeBlock
    blocks: '^SCUX::SCUX$'
    from: '^(SRCIR|SADIR|SRCBR|IFSCR|IFSVR|SRCCR|MNFSR|BFSSR|SC2SR|WATSR|SEVMR|SEVCR)([01])_2SRC0_([01])$'
    to_outer: 'SRC$3$2'
    to_block: 'SCUX::SRC'
    to_inner: '$1'
  - !MakeBlock
    blocks: '^SCUX::SCUX$'
    from: '^(DVUIR|VADIR|DVUBR|DVUCR|ZCMCR|VRCTR|VRPDR|VRDBR|VRWTR|VOL\dR|DVUER|DVUSR|VEVMR|VEVCR)_DVU0_(\d)$'
    to_outer: 'DVU$2'
    to_block: 'SCUX::DVU'
    to_inner: '$1'
  - !MakeRegisterArray
    blocks: '^SCUX::DVU$'
    from: '^VOL\dR$'
    to: VOLR
  - !MakeRegisterArray
    blocks: '^SCUX::SCUX$'
    from: '^(IPC|OPC|FFD|FFU|DVU)\d$'
    to: '$1'
  - !MakeRegisterArray
    blocks: '^SCUX::SCUX$'
    from: '^SRC\d\d$'
    to: SRC
    mode: Cursed
  - !MakeRegisterArray
    blocks: '^SCUX::SCUX$'
    from: '^SRCIRR_2SRC0_\d$'
    to: SRCIRR
  # The CIM's per-FIFO and per-converter selectors.
  - !MakeRegisterArray
    blocks: '^SCUX::SCUX$'
    from: '^(DMATD|DMATU|FDTSEL|FUTSEL|SRCRSEL)\d_CIM$'
    to: '$1'
  # MIX is one unit: its registers in a block of their own.
  - !MakeBlock
    blocks: '^SCUX::SCUX$'
    from: '^(MIXIR|MADIR|MIXBR|MIXMR|MVPDR|MDBAR|MDBBR|MDBCR|MDBDR|MDBER|MIXSR)_MIX0_0$'
    to_outer: 'MIX'
    to_block: 'SCUX::MIX'
    to_inner: '$1'
```

`crates/rza1-pac/transforms/common/usb.yaml`:

```yaml
transforms:
  # Numbered registers the driver indexes: the two interrupt enable/status words, PIPE1CTR..
  # PIPEFCTR (element n is pipe n + 1; pipe 0's control is DCPCTR) and DEVADD0..DEVADDA.
  - !MakeRegisterArray
    blocks: '^USB::USB$'
    from: '^(INTENB|INTSTS)[01]$'
    to: '$1'
  - !MakeRegisterArray
    blocks: '^USB::USB$'
    from: '^PIPE[1-9A-F]CTR$'
    to: PIPECTR
  - !MakeRegisterArray
    blocks: '^USB::USB$'
    from: '^DEVADD[0-9A]$'
    to: DEVADD
  # A packet's tail goes into a FIFO port a byte at a time (see usb::fifo), so each 32-bit port
  # gets a byte view over its four lanes. The ports allow 8-, 16- and 32-bit access (Table 46.1).
  - !AddRegisters
    block: '^USB::USB$'
    registers:
      - {name: CFIFO_B, description: "CFIFO, byte access: element n is the byte at offset n.", byte_offset: 0x14, bit_size: 8, array: {len: 4, stride: 1}}
      - {name: D0FIFO_B, description: "D0FIFO, byte access: element n is the byte at offset n.", byte_offset: 0x18, bit_size: 8, array: {len: 4, stride: 1}}
      - {name: D1FIFO_B, description: "D1FIFO, byte access: element n is the byte at offset n.", byte_offset: 0x1C, bit_size: 8, array: {len: 4, stride: 1}}
```

`crates/rza1-pac/transforms/rza1l/sdhi.yaml` is generated once from Table 46.1 and then checked in as a hand-maintained transform. The manual check pins it from then on. Generate it with:

```bash
python3 - > crates/rza1-pac/transforms/rza1l/sdhi.yaml <<'EOF'
import re
rows = []
for line in open('vendor/docs/rza1/rza1l-hw-manual/ch46-list-of-registers.md'):
    if line.startswith('## 46.2'):
        break
    c = [x.strip() for x in line.strip()[1:-1].split('|')]
    if len(c) >= 6 and re.fullmatch(r"H'E804E0[0-9A-F]{2}", c[4]) and c[2].endswith('_0'):
        rows.append((c[2][:-2], int(c[3]), int(c[4][2:], 16) - 0xE804E000, re.sub(r'_0$', '', c[1])))
print('transforms:')
print("  # SDHI0/1, which the vendor SVD omits: a hand-written block from the RZ/A1L manual's Table 46.1")
print('  # (R01UH0437EJ0700, SD host interface). The RZ/A1L manual check covers it.')
print('  - !Add')
print('    ir:')
print('      "block/SDHI::SDHI":')
print('        description: "SD host interface (RZ/A1L hardware manual §38)."')
print('        items:')
for n, b, o, d in rows:
    print('          - {name: %s, description: "%s.", byte_offset: 0x%03X, bit_size: %d}' % (n, d.replace('"', ''), o, b))
print('  - !AddPeripherals')
print('    devices: .*')
print('    peripherals:')
print('      - {name: SDHI0, base_address: 0xE804E000, block: "SDHI::SDHI"}')
print('      - {name: SDHI1, base_address: 0xE804E800, block: "SDHI::SDHI"}')
EOF
grep -c byte_offset crates/rza1-pac/transforms/rza1l/sdhi.yaml
```

Expected: `30`.

**Chip files.**
- In `transforms/rza1l/chip.yaml`, after `  - ../common/adc.yaml`, add:
  ```yaml
    - ../common/dmac.yaml
    - ../common/scux.yaml
    - ../common/usb.yaml
    - sdhi.yaml
  ```
- In `transforms/rza1h/chip.yaml`, add the first three lines only.
- In both, they go before `../common/sanitize.yaml`.

- [ ] **Step 3: The manual check: views, and SDHI covered**

In `tools/pac-check-a1l.py`:
- Delete the `'SD host interface': …` entry from `NOT_YET`.
- Replace the line `    m = manual.get(addr)` with:

```python
    m = manual.get(addr)
    if m is None and addr & 3:
        # A byte or halfword view of a register the manual lists (RSPI's SPDR, USB's FIFO ports):
        # fine if that register allows the view's width.
        whole = manual.get(addr & ~3)
        if whole is not None and bits in whole[2] and bits < 32:
            continue
```

- [ ] **Step 4: Regenerate and pass**

Run:
- `tools/gen-pac.sh 2>&1 | tail -2`
- `cargo test --target armv7-unknown-linux-gnueabihf -p rza1-pac --features rza1l --lib 2>&1 | grep "test result"`
- `cargo test --target armv7-unknown-linux-gnueabihf -p rza1-pac --features rza1h --lib 2>&1 | grep "test result"`

Expected:
- `==> 3183 PAC registers, 3481 RZ/A1L registers (335 in modules not yet covered), 0 problems`.
- `ok` twice. The SDHI test is compiled for `rza1l` only.

The HAL still builds unchanged: no HAL module uses the reshaped DMAC, SCUX or USB accessors yet. Run `cargo check -p rza1l-hal` to confirm.

- [ ] **Step 5: Suite, build check, commit**

Run `tools/test.sh`, the **Firmware build check** and `tools/link-layout.sh check`.

```bash
git add crates/rza1-pac tools/pac-check-a1l.py
git commit -m "feat(rza1-pac): DMAC channel blocks, SCUX sub-unit blocks, USB arrays and FIFO byte views, and the RZ/A1L's SDHI from its manual"
```

---

### Task 2: DMAC takes its registers from the PAC

**Files:** `crates/rza1l-hal/src/{dmac.rs,dmac/raw.rs}`, `tools/check-register-tables.sh`

**Interfaces:**
- Consumes: `crate::pac::{DMAC, dmac::Ch}`.
- Produces: no API change. `Dma`'s methods, `BlockDma` and `dmac::stop_all` issue the same sequences.

- [ ] **Step 1: Pin**

Append to `dmac.rs`'s `mod tests`:

```rust
    #[test]
    fn the_pac_names_the_same_channel_registers() {
        use crate::pac::DMAC;
        for ch in 0..16usize {
            let base = 0xE820_0000 + ch * 64 + if ch >= 8 { 0x200 } else { 0 };
            let c = DMAC.ch(ch);
            let got = [
                c.n0sa().addr(),
                c.n0da().addr(),
                c.n0tb().addr(),
                c.crsa().addr(),
                c.crda().addr(),
                c.chstat().addr(),
                c.chctrl().addr(),
                c.chcfg().addr(),
                c.chitvl().addr(),
                c.chext().addr(),
                c.nxla().addr(),
            ];
            assert_eq!(
                got,
                [
                    base,
                    base + 0x04,
                    base + 0x08,
                    base + 0x18,
                    base + 0x1C,
                    base + 0x24,
                    base + 0x28,
                    base + 0x2C,
                    base + 0x30,
                    base + 0x34,
                    base + 0x38,
                ],
                "channel {ch}"
            );
            assert_eq!(c.chctrl().width(), 4);
            assert_eq!(DMAC.dctrl(ch / 8).addr(), 0xE820_0300 + (ch / 8) * 0x400);
            assert_eq!(DMAC.dmars(ch / 2).addr(), 0xFCFE_1000 + (ch / 2) * 4);
        }
    }
```

Run it by name. Expected: `ok` (characterization).

- [ ] **Step 2: Migrate**

- **`dmac.rs`:**
  - Delete `DMAC_BASE`, `DMARS_BASE`, the `OFF_*` constants and `DCTRL_0_7`.
  - `ch_reg(ch, off)` and `dctrl_reg(ch)` go. Callers take the PAC handle:
    - `ch_reg(ch, OFF_CHCTRL)` → `DMAC.ch(ch as usize).chctrl()`, and so on for each register;
    - `dctrl_reg(ch)` → `DMAC.dctrl((ch / 8) as usize)`.
  - The old handles were `mmio::Reg32`: their `.write(v)` becomes `.write_value(v)` and `.read()` stays `.read()`. Every DMAC register is a plain `u32` (the SVD gives DMAC no fields).
  - Keep the bit constants (`CHCTRL_*`, `CHSTAT_TC`, …).
- **`dmac/raw.rs`:** the same mapping. The DMARS read-modify-write (even channel in bits 15:0, odd in 31:16) takes `DMAC.dmars((ch / 2) as usize)`, with its read and write unchanged.
- **Tests:** the existing tests that compute `0xE820_0000 + ch * 64 + …` in a local `chreg` helper keep it. That's test-local arithmetic, which the check ignores. Replace any use of a deleted constant with its literal, and delete the old address tests the pin supersedes.
- **`tools/check-register-tables.sh`:** add `dmac.rs dmac/raw.rs` to `MIGRATED`.

- [ ] **Step 3: Verify and commit**

Run:
- the HAL tests: `ok`; DMAC's sequence tests (register mode, link-descriptor mode, `stop`, DMARS sharing) pass unchanged;
- `tools/check-register-tables.sh`;
- `tools/test.sh`;
- the **Firmware build check**;
- `tools/link-layout.sh check`;
- the DelugeFirmware device check.

```bash
git add crates/rza1l-hal/src/dmac.rs crates/rza1l-hal/src/dmac/raw.rs tools/check-register-tables.sh
git commit -m "refactor(rza1l-hal): DMAC takes its channel, group and selector registers from rza1-pac"
```

---

### Task 3: SSI takes its registers from the PAC

**Files:** `crates/rza1l-hal/src/ssi.rs`, `tools/check-register-tables.sh`

**Interfaces:**
- Consumes: `crate::pac::{SSIF0, CPG}`, with `ssif::Ssif` methods `ssicr, ssisr, ssifcr, ssifsr, ssiftdr, ssifrdr, ssitdmr, ssifccr, ssifcmr, ssifcsr` and `CPG.swrstcr1()`.
- Produces: no API change.

- [ ] **Step 1: Pin**

```rust
    #[test]
    fn the_pac_names_the_same_ssi_registers() {
        use crate::pac::{CPG, SSIF0};
        let b = 0xE820_B000;
        assert_eq!((SSIF0.ssicr().addr(), SSIF0.ssicr().width()), (b, 4));
        assert_eq!(SSIF0.ssisr().addr(), b + 0x04);
        assert_eq!(SSIF0.ssifcr().addr(), b + 0x10);
        assert_eq!(SSIF0.ssifsr().addr(), b + 0x14);
        assert_eq!(SSIF0.ssiftdr().addr(), b + 0x18);
        assert_eq!(SSIF0.ssifrdr().addr(), b + 0x1C);
        assert_eq!(SSIF0.ssitdmr().addr(), b + 0x20);
        assert_eq!((CPG.swrstcr1().addr(), CPG.swrstcr1().width()), (0xFCFE_0460, 1));
    }
```

Before running it, compare each offset with `ssi.rs`'s own constants. They are the source of truth for the pin; fix the test if the plan's list differs.

Run it by name. Expected: `ok`.

- [ ] **Step 2: Migrate**

- Delete `SSI0_BASE`, the register-offset constants and `SWRSTCR1`.
- `ssi_reg(off)` (an `mmio::Reg32`) goes: each `ssi_reg(OFF_X).write(v)` becomes `SSIF0.x().write_value(X(v))`, using the fieldset when the SVD gives one (check the generated `ssif::regs`). The SVD gives SSIF no fields, so the registers are plain `u32` and `write_value(v)` takes the value directly.
- The DMA descriptors' peripheral addresses (`SSIFTDR`/`SSIFRDR` as the transfer's fixed end) become `SSIF0.ssiftdr().addr() as u32` and `SSIF0.ssifrdr().addr() as u32`.
- The software-reset sequence on `SWRSTCR1` becomes `CPG.swrstcr1()` with its fieldset (`cpg::regs::Swrstcr1(v)`), with its writes and dummy reads unchanged.
- In the tests, replace deleted constants with literals.
- **`tools/check-register-tables.sh`:** add `ssi.rs`.

- [ ] **Step 3: Verify and commit**

Run the HAL tests (`ok`; `ssi_rings_keep_their_completion_interrupts_masked` and the block-ring test pass), `tools/check-register-tables.sh`, `tools/test.sh`, the **Firmware build check** and `tools/link-layout.sh check`.

```bash
git add crates/rza1l-hal/src/ssi.rs tools/check-register-tables.sh
git commit -m "refactor(rza1l-hal): SSI takes its registers from rza1-pac"
```

---

### Task 4: SCUX takes its registers from the PAC, onto the seam

**Files:** `crates/rza1l-hal/src/{scux.rs,scux/regs.rs}` (and `scux/*_path.rs` if they name a deleted item), `tools/check-register-tables.sh`

**Interfaces:**
- Consumes: `crate::pac::{SCUX, scux}`, with the arrays of Task 1.
- Produces: no API change. `scux::reset`, `configure_*`, `start`, `init_ffd_dma`/`init_ffu_dma` and the paths issue the same sequences, now through the seam.

- [ ] **Step 1: Write the failing tests**

Append to `scux.rs`'s `mod tests` (or a new `#[cfg(all(test, not(target_os = "none")))] mod seam_tests { use super::*; use crate::mmio::test; … }`):

```rust
    #[test]
    fn the_pac_names_the_same_scux_registers() {
        use crate::pac::SCUX;
        for n in 0..4usize {
            assert_eq!(SCUX.ipc(n).ipcir().addr(), 0xE820_8000 + n * 0x100);
            assert_eq!(SCUX.opc(n).opcir().addr(), 0xE820_8400 + n * 0x100);
            assert_eq!(SCUX.ffd(n).ffdir().addr(), 0xE820_8800 + n * 0x100);
            assert_eq!(SCUX.ffu(n).ffuir().addr(), 0xE820_8C00 + n * 0x100);
            assert_eq!(SCUX.dvu(n).dvuir().addr(), 0xE820_9200 + n * 0x100);
            let (unit, pair) = (n / 2, n % 2);
            assert_eq!(SCUX.src(n).srcir().addr(), 0xE820_9000 + unit * 0x100 + pair * 0x34);
        }
        assert_eq!(SCUX.mix().mixir().addr(), 0xE820_9600);
        assert_eq!(SCUX.swrsr_cim().addr(), 0xE820_9700);
    }

    #[test]
    fn a_reset_writes_swrsr_with_dummy_reads() {
        crate::mmio::test::reset();
        unsafe { reset() };
        let log: Vec<_> = crate::mmio::test::log().iter().map(|a| (a.write, a.addr, a.width)).collect();
        let swrsr = 0xE820_9700;
        assert_eq!(
            &log[..4],
            [(true, swrsr, 4), (false, swrsr, 4), (true, swrsr, 4), (false, swrsr, 4)]
        );
    }
```

The MIX address comes from `scux/regs.rs`'s `MIX_BASE`; confirm it before running. If `reset()` touches registers before SWRSR, match its first four accesses as `scux.rs:415–418` shows them: write RESET, dummy read, write RUN, dummy read.

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib scux:: 2>&1 | tail -5`.
Expected:
- The pin passes when run alone by name.
- `a_reset_writes_swrsr_with_dummy_reads` crashes the test process with a segmentation fault: `reset` writes `0xE820_9700` through a raw pointer, outside the seam.

- [ ] **Step 3: Migrate**

- **`scux/regs.rs`:**
  - Delete `SCUX_BASE`, `SRC_BASE`, `DVU_BASE`, `MIX_BASE`, `CIM_BASE`, every `*_STRIDE` and `*_OFF` constant, and the pointer functions `ipc`, `opc`, `ffd`, `ffu`, `src`, `dvu`, `mix` and `cim`.
  - Keep the value constants: `FDTSEL_*`, `SSICTRL_*`, `INTIFS_*`, `ffd_chcfg`/`ffu_chcfg` and the rest. Its address tests are superseded by the pin.
- **`scux.rs`:** each raw access maps onto the PAC:

  | Old | New |
  |---|---|
  | `ipc(ch, IPSLR_OFF).write_volatile(v)` | `SCUX.ipc(ch as usize).ipslr().write_value(v)` |
  | `ffd(ch, DRQSR_OFF).write_volatile(v)` | `SCUX.ffd(ch as usize).drqsr().write_value(v)` |
  | `src(unit, pair, IFSVR_OFF)` | `SCUX.src(2 * unit as usize + pair as usize).ifsvr()` |
  | `dvu(ch, VOLnR_OFF)` | `SCUX.dvu(ch as usize).volr(n)` |
  | `mix(MIXIR_OFF)` | `SCUX.mix().mixir()` |
  | `cim(SWRSR_CIM_OFF)` | `SCUX.swrsr_cim()` |
  | `cim(SSICTRL_OFF)` | `SCUX.ssictrl_cim()` |
  | `cim(FDTSEL0_OFF + 4*n)` | `SCUX.fdtsel(n)` |

  Use the selector arrays (`fdtsel(n)`, `futsel(n)`, `dmatd(n)`, `dmatu(n)`, `srcrsel(n)`) wherever the old code added `4*n` to a CIM offset. Each `.read_volatile()` becomes `.read()`.
- **Register types.** The SCUX registers' types are plain `u32` unless `scux::regs` defines a fieldset for one. Where it does, wrap the value (`X(v)`) and take `.read().0`. The compiler names each case.
- **Calls that may no longer need `unsafe`.** The path modules' `unsafe fn`s keep their signatures. Remove `unsafe` blocks only where nothing unsafe remains.
- **`tools/check-register-tables.sh`:** add `scux.rs scux/regs.rs`.

Run the SCUX tests: all `ok`. The five `*_before_init_panics` tests keep passing.

- [ ] **Step 4: Verify and commit**

Run the HAL tests, `tools/check-register-tables.sh`, `tools/test.sh`, the **Firmware build check** (controller and demo drive SCUX) and `tools/link-layout.sh check`.

```bash
git add crates/rza1l-hal/src/scux.rs crates/rza1l-hal/src/scux tools/check-register-tables.sh
git commit -m "refactor(rza1l-hal): SCUX takes its sub-unit registers from rza1-pac, and its accesses go through the seam"
```

---

### Task 5: SDHI takes its registers from the PAC

**Files:** `crates/rza1l-hal/src/sdhi.rs`, `tools/check-register-tables.sh`

**Interfaces:**
- Consumes: `crate::pac::{SDHI0, SDHI1, sdhi::Sdhi, CPG}`.
- Produces: no API change. `Sdhi::{new, reset, …}` issue the same sequences.

- [ ] **Step 1: Pin**

```rust
    #[test]
    fn the_pac_names_the_same_sdhi_registers() {
        for (port, base) in [(0u8, 0xE804_E000usize), (1, 0xE804_E800)] {
            let s = sdhi(port);
            let got = [
                (s.sd_cmd().addr(), s.sd_cmd().width()),
                (s.sd_arg0().addr(), s.sd_arg0().width()),
                (s.sd_arg1().addr(), s.sd_arg1().width()),
                (s.sd_stop().addr(), s.sd_stop().width()),
                (s.sd_seccnt().addr(), s.sd_seccnt().width()),
                (s.sd_rsp00().addr(), s.sd_rsp00().width()),
                (s.sd_rsp07().addr(), s.sd_rsp07().width()),
                (s.sd_info1().addr(), s.sd_info1().width()),
                (s.sd_info2().addr(), s.sd_info2().width()),
                (s.sd_info1_mask().addr(), s.sd_info1_mask().width()),
                (s.sd_info2_mask().addr(), s.sd_info2_mask().width()),
                (s.sd_clk_ctrl().addr(), s.sd_clk_ctrl().width()),
                (s.sd_size().addr(), s.sd_size().width()),
                (s.sd_option().addr(), s.sd_option().width()),
                (s.sd_err_sts1().addr(), s.sd_err_sts1().width()),
                (s.sd_err_sts2().addr(), s.sd_err_sts2().width()),
                (s.sd_buf0().addr(), s.sd_buf0().width()),
                (s.sdio_mode().addr(), s.sdio_mode().width()),
                (s.sdio_info1().addr(), s.sdio_info1().width()),
                (s.sdio_info1_mask().addr(), s.sdio_info1_mask().width()),
                (s.cc_ext_mode().addr(), s.cc_ext_mode().width()),
                (s.soft_rst().addr(), s.soft_rst().width()),
                (s.ext_swap().addr(), s.ext_swap().width()),
            ];
            let want = [
                (base, 2),
                (base + 0x04, 2),
                (base + 0x06, 2),
                (base + 0x08, 2),
                (base + 0x0A, 2),
                (base + 0x0C, 2),
                (base + 0x1A, 2),
                (base + 0x1C, 2),
                (base + 0x1E, 2),
                (base + 0x20, 2),
                (base + 0x22, 2),
                (base + 0x24, 2),
                (base + 0x26, 2),
                (base + 0x28, 2),
                (base + 0x2C, 2),
                (base + 0x2E, 2),
                (base + 0x30, 4),
                (base + 0x34, 2),
                (base + 0x36, 2),
                (base + 0x38, 2),
                (base + 0xD8, 2),
                (base + 0xE0, 2),
                (base + 0xF0, 2),
            ];
            assert_eq!(got, want, "SDHI{port}");
        }
        assert_eq!(crate::pac::CPG.stbcr12().addr(), 0xFCFE_0444);
    }
```

Add the helper, and keep `port_base` until Step 2 deletes it:

```rust
/// SDHI port `port`'s registers.
fn sdhi(port: u8) -> crate::pac::sdhi::Sdhi {
    if port == 0 {
        crate::pac::SDHI0
    } else {
        crate::pac::SDHI1
    }
}
```

Run it by name. Expected: `ok`.

- [ ] **Step 2: Migrate**

- Delete `SDHI0_BASE`, `SDHI1_BASE`, every `OFF_*`, `STBCR12`, `port_base`, `reg16` and `reg32`.
- `reg16(base, OFF_X).write(v)` becomes `sdhi(port).x().write_value(v)`, and `.read()` stays `.read()`. Every SDHI register in the hand-written block is a plain `u16`, or `u32` for `sd_buf0`.
- Where a function held `let base = port_base(port);`, hold `let s = sdhi(port);`.
- The DMA transfers' fixed end, `SD_BUF0`'s address, becomes `sdhi(port).sd_buf0().addr() as u32`.
- The module-clock read-modify-write on `STBCR12` becomes `CPG.stbcr12()` with its fieldset and its dummy read.
- The response readers (`read_r1`, `read_r2`) read `sd_rsp00` … `sd_rsp07` in the same order as before.
- In the tests, replace deleted constants with literals. `SDHI1_BASE + OFF_OPTION` becomes `0xE804_E828`, and so on.
- **`tools/check-register-tables.sh`:** add `sdhi.rs`.

- [ ] **Step 3: Verify and commit**

Run:
- the HAL tests: `ok`; `a_controller_comes_up_clocked_reset_and_bound_with_sdio_masked`, `card_detect_reads_info1_of_its_own_port` and `a_reset_returns_the_controller_to_the_identification_clock` pass unchanged;
- `tools/check-register-tables.sh`;
- `tools/test.sh`;
- the **Firmware build check**;
- `tools/link-layout.sh check`;
- the DelugeFirmware device check.

```bash
git add crates/rza1l-hal/src/sdhi.rs tools/check-register-tables.sh
git commit -m "refactor(rza1l-hal): SDHI takes its registers from rza1-pac's block from the RZ/A1L manual"
```

---

### Task 6: SPIBSC takes its registers from the PAC, onto the seam

**Files:** `crates/rza1l-hal/src/spibsc.rs`, `tools/check-register-tables.sh`

**Interfaces:**
- Consumes: `crate::pac::SPIBSC0`, with `spibsc::Spibsc` methods `cmncr, drcr, smcr, smcmr, smadr, smenr, smrdr0, smrdr1, smwdr0, smwdr1, cmnsr` (all plain `u32`).
- Produces: no API change.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn the_pac_names_the_same_spibsc_registers() {
        use crate::pac::SPIBSC0;
        let b = 0x3FEF_A000;
        assert_eq!((SPIBSC0.cmncr().addr(), SPIBSC0.cmncr().width()), (b, 4));
        assert_eq!(SPIBSC0.drcr().addr(), b + 0x00C);
        assert_eq!(SPIBSC0.smcr().addr(), b + 0x020);
        assert_eq!(SPIBSC0.smcmr().addr(), b + 0x024);
        assert_eq!(SPIBSC0.smadr().addr(), b + 0x028);
        assert_eq!(SPIBSC0.smenr().addr(), b + 0x030);
        assert_eq!(SPIBSC0.cmnsr().addr(), b + 0x048);
    }

    #[test]
    fn reading_the_status_register_is_one_manual_transfer() {
        crate::mmio::test::reset();
        // CMNSR.TEND set: every transfer completes at once.
        crate::mmio::test::poke32(0x3FEF_A048, 1);
        let _ = read_status_reg();
        let w = crate::mmio::test::writes();
        // Manual mode on (CMNCR.MD), then the command and a start of transfer with read enabled.
        assert!(w.iter().any(|&(a, v)| a == 0x3FEF_A000 && v & (1 << 31) != 0));
        assert!(w.iter().any(|&(a, v)| a == 0x3FEF_A024 && (v >> 16) & 0xFF == 0x05));
        assert!(w.iter().any(|&(a, v)| a == 0x3FEF_A020 && v & 0b101 == 0b101));
    }
```

`read_status_reg` is `pub fn read_status_reg() -> u8` (the one `app-loader` calls). The SMCMR command field is bits 23:16, as the module's own `CMD_RDSR << 16` writes it.

Run it. Expected: the pin passes alone, and the second test crashes with a segmentation fault: `wr32`/`rd32` dereference `0x3FEF_A000` directly.

- [ ] **Step 2: Migrate**

- Delete `SPIBSC0`, `CMNCR`, `DRCR`, `SMCR`, `SMCMR`, `SMADR`, `SMENR`, `SMRDR0`, `SMWDR0`, `CMNSR` and the private `wr32`/`rd32`. Keep the bit and opcode constants.
- Each `wr32(SMCR, v)` becomes `SPIBSC0.smcr().write_value(v)`, using the fieldset where the SVD gives one. The SVD gives SPIBSC no fields, so these are plain `u32`.
- Each `rd32(CMNSR)` becomes `SPIBSC0.cmnsr().read()`.
- **`tools/check-register-tables.sh`:** add `spibsc.rs`.

- [ ] **Step 3: Verify and commit**

Run the HAL tests (`ok`, including the window and anti-alias tests), `tools/check-register-tables.sh`, `tools/test.sh`, the **Firmware build check** (app-loader uses SPIBSC) and `tools/link-layout.sh check`.

```bash
git add crates/rza1l-hal/src/spibsc.rs tools/check-register-tables.sh
git commit -m "refactor(rza1l-hal): SPIBSC takes its registers from rza1-pac, and its manual transfers go through the seam"
```

---

### Task 7: USB's register layer, FIFO, pipes and port control take the PAC

**Files:** `crates/rza1l-hal/src/usb/{regs.rs,fifo.rs,pipe.rs,mod.rs}`

**Interfaces:**
- Consumes: `crate::pac::{USB200, USB201, usb::Usb, CPG}`.
- Produces:
  - `pub fn usb::regs::usb(port: u8) -> crate::pac::usb::Usb`, returning `USB200` for port 0 and `USB201` for port 1;
  - `pub fn usb::regs::pipectr(u: Usb, pipe: usize) -> Reg<u16, RW>`, which is `u.dcpctr()` for pipe 0 and `u.pipectr(pipe - 1)` otherwise;
  - `FifoPort { data: Reg<u32, RW>, lanes: [Reg<u8, RW>; 4], sel: Reg<u16, RW>, ctr: Reg<u16, RW> }`.

  `Rusb1Regs`, `rd`/`wr`/`rmw`/`rd32`/`wr32`, `pipectr_ptr` and `devadd_ptr` remain until Task 8 for `driver.rs`/`host.rs`.

- [ ] **Step 1: Pin, and write the failing FIFO test**

In `usb/regs.rs`'s `mod tests`, next to `reg_offsets`:

```rust
    /// Every `Rusb1Regs` field sits where the PAC puts the register of the same name: the struct
    /// can go once nothing reads through it.
    #[test]
    fn the_pac_names_every_rusb1regs_field() {
        use crate::pac::USB200 as U;
        let off = |a: usize| a - 0xE801_0000;
        let pairs = [
            (offset_of!(Rusb1Regs, syscfg0), off(U.syscfg0().addr())),
            (offset_of!(Rusb1Regs, buswait), off(U.buswait().addr())),
            (offset_of!(Rusb1Regs, syssts0), off(U.syssts0().addr())),
            (offset_of!(Rusb1Regs, dvstctr0), off(U.dvstctr0().addr())),
            (offset_of!(Rusb1Regs, cfifo), off(U.cfifo().addr())),
            (offset_of!(Rusb1Regs, d0fifo), off(U.d0fifo().addr())),
            (offset_of!(Rusb1Regs, d1fifo), off(U.d1fifo().addr())),
            (offset_of!(Rusb1Regs, cfifosel), off(U.cfifosel().addr())),
            (offset_of!(Rusb1Regs, cfifoctr), off(U.cfifoctr().addr())),
            (offset_of!(Rusb1Regs, d0fifosel), off(U.d0fifosel().addr())),
            (offset_of!(Rusb1Regs, d0fifoctr), off(U.d0fifoctr().addr())),
            (offset_of!(Rusb1Regs, d1fifosel), off(U.d1fifosel().addr())),
            (offset_of!(Rusb1Regs, d1fifoctr), off(U.d1fifoctr().addr())),
            (offset_of!(Rusb1Regs, intenb0), off(U.intenb(0).addr())),
            (offset_of!(Rusb1Regs, intenb1), off(U.intenb(1).addr())),
            (offset_of!(Rusb1Regs, brdyenb), off(U.brdyenb().addr())),
            (offset_of!(Rusb1Regs, nrdyenb), off(U.nrdyenb().addr())),
            (offset_of!(Rusb1Regs, bempenb), off(U.bempenb().addr())),
            (offset_of!(Rusb1Regs, intsts0), off(U.intsts(0).addr())),
            (offset_of!(Rusb1Regs, intsts1), off(U.intsts(1).addr())),
            (offset_of!(Rusb1Regs, brdysts), off(U.brdysts().addr())),
            (offset_of!(Rusb1Regs, nrdysts), off(U.nrdysts().addr())),
            (offset_of!(Rusb1Regs, bempsts), off(U.bempsts().addr())),
            (offset_of!(Rusb1Regs, usbaddr), off(U.usbaddr().addr())),
            (offset_of!(Rusb1Regs, usbreq), off(U.usbreq().addr())),
            (offset_of!(Rusb1Regs, usbval), off(U.usbval().addr())),
            (offset_of!(Rusb1Regs, usbindx), off(U.usbindx().addr())),
            (offset_of!(Rusb1Regs, usbleng), off(U.usbleng().addr())),
            (offset_of!(Rusb1Regs, dcpcfg), off(U.dcpcfg().addr())),
            (offset_of!(Rusb1Regs, dcpmaxp), off(U.dcpmaxp().addr())),
            (offset_of!(Rusb1Regs, dcpctr), off(U.dcpctr().addr())),
            (offset_of!(Rusb1Regs, pipesel), off(U.pipesel().addr())),
            (offset_of!(Rusb1Regs, pipecfg), off(U.pipecfg().addr())),
            (offset_of!(Rusb1Regs, pipebuf), off(U.pipebuf().addr())),
            (offset_of!(Rusb1Regs, pipemaxp), off(U.pipemaxp().addr())),
            (offset_of!(Rusb1Regs, pipeperi), off(U.pipeperi().addr())),
            (offset_of!(Rusb1Regs, pipe1ctr), off(U.pipectr(0).addr())),
            (offset_of!(Rusb1Regs, pipefctr), off(U.pipectr(14).addr())),
            (offset_of!(Rusb1Regs, devadd0), off(U.devadd(0).addr())),
            (offset_of!(Rusb1Regs, suspmode), off(U.suspmode().addr())),
        ];
        for (i, (field, pac)) in pairs.iter().enumerate() {
            assert_eq!(field, pac, "pair {i}");
        }
        assert_eq!(crate::pac::USB201.syscfg0().addr(), 0xE820_7000);
    }
```

The PAC's names come from the SVD in Rust casing, digits included (`SYSCFG0` → `syscfg0`); only arrays drop theirs (`INTENB0` → `intenb(0)`). If one doesn't compile, look it up in `crates/rza1-pac/src/chips/rza1l.rs` (`grep -n "pub const fn syscfg" …`) and use the generated name.

In `usb/fifo.rs`'s tests, add a seam test for the tail lanes alongside the existing stack-buffer `sw_to_hw_tail_uses_bigend0_lanes`:

```rust
    #[test]
    fn a_packet_tail_goes_to_cfifo_byte_lanes_through_the_seam() {
        crate::mmio::test::reset();
        let port = unsafe { FifoPort::cfifo(crate::usb::regs::Rusb1Regs::ptr(0)) };
        unsafe { sw_to_hw_fifo(&port, [1u8, 2, 3, 4, 5, 6].as_ptr(), 6) };
        let w: Vec<_> = crate::mmio::test::log()
            .iter()
            .filter(|a| a.write && (0xE801_0014..0xE801_0018).contains(&a.addr))
            .map(|a| (a.addr, a.width))
            .collect();
        // One 32-bit word, then bytes 5 and 6 into lanes 3 and 2 (BIGEND clear).
        assert_eq!(w, [(0xE801_0014, 4), (0xE801_0017, 1), (0xE801_0016, 1)]);
    }
```

`FifoPort::cfifo` takes `*mut Rusb1Regs` today and `usb::regs::usb(0)` after Step 2. Write the test against the old constructor now; Step 2 changes that one line along with the API.

Run: `cargo test --target armv7-unknown-linux-gnueabihf -p rza1l-hal --lib usb:: 2>&1 | tail -5`.
Expected:
- The pin passes when run alone by name.
- The FIFO seam test crashes with a segmentation fault: `sw_to_hw_fifo` writes `0xE801_0014` with `write_volatile`.

- [ ] **Step 2: Migrate the register layer, FIFO, pipes and port control**

- **`usb/regs.rs`:** add `pub fn usb(port: u8) -> crate::pac::usb::Usb` and `pub fn pipectr(u: crate::pac::usb::Usb, pipe: usize) -> Reg<u16, RW>`, as the Interfaces block describes. Keep `Rusb1Regs`, `rd`/`wr`/… for now.
- **`usb/fifo.rs`:**
  - `FifoPort` becomes `{ data: Reg<u32, RW>, lanes: [Reg<u8, RW>; 4], sel: Reg<u16, RW>, ctr: Reg<u16, RW> }`.
  - `cfifo`/`d0fifo`/`d1fifo` take `u: Usb`, filling `data: u.cfifo()`, `lanes: [u.cfifo_b(0), u.cfifo_b(1), u.cfifo_b(2), u.cfifo_b(3)]`, `sel: u.cfifosel()` and `ctr: u.cfifoctr()` (and the same for D0/D1).
  - `fifo_for_pipe(u, pipe)` routes as before.
  - `wr32(fifo.data, w)` → `fifo.data.write_value(w)`; `rd32(fifo.data)` → `fifo.data.read()`.
  - The tail's `write_volatile(base.add(3 - (i & 3)), b)` → `fifo.lanes[3 - (i & 3)].write_value(b)`. The read side's sub-word loop takes the same lanes.
  - `rd(fifo.sel)` / `wr(fifo.sel, v)` / `rmw(…)` become `fifo.sel.read()` / `.write_value(v)` / `.modify(|r| *r = (*r & !mask) | (val & mask))`. That matches `rmw`'s read-then-write; check `rmw`'s exact definition in `regs.rs` and mirror it.
  - The existing stack-buffer tests (`sw_to_hw_roundtrip`, `sw_to_hw_tail_uses_bigend0_lanes`, `hw_to_sw_sub_word`) built `FifoPort` from stack pointers. Rewrite each on the seam: preload with `poke32`/`poke8` at `USB200`'s FIFO addresses, and assert the logged writes. They keep their names and what they assert.
- **`usb/pipe.rs`:** the same mapping. Each `Rusb1Regs::ptr(port)` becomes `usb(port)`, `pipectr_ptr(regs, n)` becomes `pipectr(u, n)`, and `rd`/`wr`/`rmw` on a field become the register's `read`/`write_value`/`modify`. The ISO hook API (`register_iso_out_hook`/`register_iso_in_hook`) is unchanged.
- **`usb/mod.rs`:**
  - Delete `USB0_BASE`/`USB1_BASE` and `STBCR7`.
  - `base(port)` goes, or becomes `usb(port).syscfg0().addr()` if a caller needs an address. Check with `rg -n "usb::base\(|USB[01]_BASE"`; Phase 5a's survey found none outside the HAL.
  - `module_clock_enable`/`module_clock_disable` take `CPG.stbcr7()` with its fieldset and their dummy read.
  - `disconnect` takes `usb(T::PORT)`.
  - Its tests `base_addresses`/`base_fn` are superseded by Step 1's pin: delete them.
- Unwrap `unsafe` blocks that no longer contain an unsafe operation. Remove `// SAFETY:` comments left above code with no `unsafe`.

Run the USB tests: all `ok`, including the new FIFO seam test and the rewritten stack-buffer tests.

- [ ] **Step 3: Verify and commit**

Run the HAL tests, the BSP tests, `tools/test.sh`, the **Firmware build check** (every USB firmware) and `tools/link-layout.sh check`. `tools/check-register-tables.sh` doesn't list the USB files until Task 8.

```bash
git add crates/rza1l-hal/src/usb
git commit -m "refactor(rza1l-hal): USB's FIFO, pipe and port-control code takes its registers from rza1-pac, through the seam"
```

---

### Task 8: USB's device and host drivers take the PAC, and `Rusb1Regs` goes

**Files:** `crates/rza1l-hal/src/usb/{driver.rs,host.rs,regs.rs}`, `hw-tests/vbus-probe/src/tasks/probe.rs`, `tools/check-register-tables.sh`

**Interfaces:**
- Consumes: Task 7's `usb::regs::{usb, pipectr}` and `FifoPort`.
- Produces: `usb::regs` keeps its bit constants, `usb()` and `pipectr()`. `Rusb1Regs`, `rd`, `wr`, `rmw`, `rd32`, `wr32`, `pipectr_ptr` and `devadd_ptr` are gone.

- [ ] **Step 1: Migrate the drivers**

- **`driver.rs` and `host.rs`:** each `let regs = Rusb1Regs::ptr(port);` becomes `let u = usb(port);`. Each access maps one to one:
  - `rd(addr_of!((*regs).f))` → `u.f().read()`;
  - `wr(addr_of_mut!((*regs).f), v)` → `u.f().write_value(v)`;
  - `rmw(addr_of_mut!((*regs).f), mask, val)` → `u.f().modify(|r| *r = (*r & !mask) | (val & mask))`. Mirror `rmw`'s definition exactly; if it differs, follow it;
  - `pipectr_ptr(regs, n)` → `pipectr(u, n)`;
  - `devadd_ptr(regs, a)` → `u.devadd(a as usize)`.

  Field names map as in Task 7's pin. Where the PAC's array form differs from the field (`intenb0` → `intenb(0)`, `intsts1` → `intsts(1)`, `pipe1ctr` → `pipectr(0)`, `devadd0` → `devadd(0)`), use the PAC's; the rest keep their names (`syscfg0`, `syssts0`, `dvstctr0`, …). There are about 145 sites across the two files; grep `\(\*regs\)\.` afterwards and expect none.
- **`regs.rs`:** delete `Rusb1Regs` (and `Rusb1Regs::ptr`), `rd`, `wr`, `rmw`, `rd32`, `wr32`, `pipectr_ptr` and `devadd_ptr`.
  - Task 7's pin `the_pac_names_every_rusb1regs_field` used `offset_of!`. Rewrite it as `the_pac_names_the_usb_registers`, asserting each PAC address against its literal offset from `0xE801_0000` (the offsets in the struct's `// +0x…` comments).
  - The old `reg_offsets` test goes.
- **`hw-tests/vbus-probe/src/tasks/probe.rs`:**

  ```rust
  use rza1l_hal::usb::regs::INTSTS0_VBSTS;
  ```

  `let regs = Rusb1Regs::ptr(0);` … `rd(addr_of!((*regs).intsts0))` becomes `rza1l_hal::pac::USB200.intsts(0).read()`.
- Unwrap `unsafe` blocks that no longer contain an unsafe operation. Remove stale `// SAFETY:` comments.
- **`tools/check-register-tables.sh`:** add `usb/regs.rs usb/fifo.rs usb/pipe.rs usb/mod.rs usb/driver.rs usb/host.rs`.

Run: `rg -n "Rusb1Regs|addr_of(_mut)?!\(\(\*regs\)" -g '*.rs' crates firmwares hw-tests examples`
Expected: no output.

- [ ] **Step 2: The ISR paths still inline**

```bash
cargo build-fw -p deluge-controller --release 2>&1 | tail -1
llvm-nm -C target/armv7a-none-eabihf/release/deluge-controller | grep -c "rza1_pac::common\|rza1_pac::mmio"
```

Expected: `0`. If `llvm-nm` isn't on PATH, `rust-nm` takes the same arguments.

- [ ] **Step 3: Verify and commit**

Run:
- the HAL tests, the BSP tests (`usb-host`), `tools/check-register-tables.sh` and `tools/test.sh`;
- the **Firmware build check**, which covers every USB firmware including `vbus-probe` and `uac-host-firmware`;
- `tools/link-layout.sh check`;
- the DelugeFirmware device check.

```bash
git add crates/rza1l-hal/src/usb hw-tests/vbus-probe tools/check-register-tables.sh
git commit -m "refactor(rza1l-hal)!: USB's device and host drivers take their registers from rza1-pac, and the Rusb1Regs struct is gone"
```

---

### Task 9: The exit criterion holds for the whole HAL; docs

**Files:** `tools/check-register-tables.sh`, `CHANGELOG.md`, `crates/rza1l-hal/README.md`, `crates/rza1-pac/README.md`, the spec

- [ ] **Step 1: Check every module, not a list**

Replace `tools/check-register-tables.sh`'s `MIGRATED=(…)` list and loop with a scan of every module, minus two named exceptions:

```bash
# Every module under crates/rza1l-hal/src takes its registers from rza1_pac, except these, which
# describe memory rather than registers.
EXEMPT=(memmap.rs mmu.rs)

status=0
while IFS= read -r f; do
  m="${f#crates/rza1l-hal/src/}"
  for e in "${EXEMPT[@]}"; do [ "$m" = "$e" ] && continue 2; done
  hits="$(awk '/^#\[cfg\(all\(test/{exit} {print FILENAME":"NR": "$0}' "$f" \
    | grep -vE '^[^:]+:[0-9]+: *(//|\*)' \
    | grep -E 'const +(OFF_[A-Z0-9_]+|[A-Z0-9_]*_BASE) *: *usize|0[xX]([Ee]8|[Ff][Cc]|3[Ff])[0-9A-Fa-f]{2}_?[0-9A-Fa-f]{4}' || true)"
  if [ -n "$hits" ]; then
    printf '%s\n' "$hits"
    status=1
  fi
done < <(find crates/rza1l-hal/src -name '*.rs' | sort)
```

Keep the header comment, updated to say the check now covers the whole crate. Keep the error message.

Run: `tools/check-register-tables.sh; echo "exit $?"`
Expected: `exit 0`.

If a module the plan didn't migrate turns up, it is a register user Phase 5 missed. Migrate it in this task, in the same style as Tasks 2–8, and ledger it. A planning-time survey found `interrupt.rs`, `lib.rs`, `clocks.rs`, `time_driver.rs` and `time_math.rs` already clean outside their tests.

- [ ] **Step 2: Docs**

- **`CHANGELOG.md`, top of `### Changed`:**

```markdown
- Phase 5 is complete: every `rza1l-hal` module takes its registers from `rza1-pac`
  (`tools/check-register-tables.sh` checks the whole crate). DMAC, SSI, SCUX, SDHI, SPIBSC and USB
  moved in 5b; SCUX, SPIBSC and USB used raw volatile pointers and now go through the MMIO seam, so
  their register sequences are host-testable. The PAC gains DMAC channel blocks, SCUX sub-unit
  blocks, USB arrays and FIFO byte views, and the RZ/A1L's SDHI (a block from its manual's register
  list; the vendor SVD lacks it). **Breaking:** `usb::regs::{Rusb1Regs, rd, wr, rmw, rd32, wr32,
  pipectr_ptr, devadd_ptr}` and `usb::{USB0_BASE, USB1_BASE}` are gone: use `rza1l_hal::pac::USB200`
  / `USB201` (or `usb::regs::usb(port)`).
```

- **`crates/rza1l-hal/README.md`:** the "Registers" paragraph from 5a ends "DMAC, SSI, SCUX, SDHI, SPIBSC and USB still hand-write their addresses until Phase 5b". Make it say every module takes its registers from the PAC, and that `tools/check-register-tables.sh` checks the whole crate.
- **`crates/rza1-pac/README.md`:** add one sentence. SDHI is a hand-written block for the RZ/A1L (`transforms/rza1l/sdhi.yaml`) from Table 46.1, covered by the manual check; the RZ/A1H PAC has no SDHI block, for want of an RZ/A1H manual.
- **The spec, after the "5a done" paragraph:**

```markdown
**5b done (2026-10-15 plan):** DMAC (per-channel blocks), SSI, SCUX (per-sub-unit blocks), SDHI (a
block from the RZ/A1L manual's Table 46.1, which the manual check now covers), SPIBSC and USB (its
`Rusb1Regs` struct replaced by the PAC, the FIFO's byte lanes as PAC views) take their registers
from `rza1-pac`; SCUX, SPIBSC and USB now go through the MMIO seam. `tools/check-register-tables.sh`
checks every HAL module apart from `memmap.rs` and `mmu.rs`, which describe memory: Phase 5's exit
holds.
```

- [ ] **Step 3: Final verification and commit**

- `tools/test.sh > /tmp/claude-1000/p5b-final.log 2>&1; echo "exit $?"`: `exit 0`, with `failed 0` in the summed results and `0 problems` from the manual check.
- `cargo check --target armv7a-none-eabihf -Zbuild-std=core -p rza1-pac --features rza1h`: `Finished`.
- `tools/link-layout.sh check && tools/check-manifests.sh`: pass.
- The **Firmware build check** and the DelugeFirmware device check.
- The simulator check: `oled_hello` headless gives `cv 0 0 0 0` / `gate 0 0 0 0`, and the frame shows "hello deluge".

```bash
git add tools/check-register-tables.sh CHANGELOG.md crates/rza1l-hal/README.md crates/rza1-pac/README.md docs/superpowers/specs/2026-10-14-rza1-hal-architecture-design.md
git commit -m "docs: Phase 5 is complete — every rza1l-hal module takes its registers from rza1-pac"
```

---

## Hardware smoke tests (the human partner's merge gate)

Every register access in DMAC, SSI, SCUX, SDHI, SPIBSC and USB now compiles through a new path:

1. **DMA and audio (DMAC, SSI, SCUX):** `audio_passthru` and `audio_passthru_irq` pass audio with no glitches. `deluge-controller` plays audio through SCUX, and its volume control works. The OLED and the PIC link keep working (both use DMA).
2. **SD (SDHI):** `sd_demo` reads the card. `sd-bench` runs at both dividers. A card swap is re-identified.
3. **Flash (SPIBSC):** app-loader reads the flash ID and status from its menu, and stores an app to the flash slot. A chain-load from flash works.
4. **USB device:** `usb_log` enumerates and logs. `msc-firmware` mounts. app-loader's DATA TRANSFER and dev-upload sessions each enumerate, including after one another. A transfer whose length isn't a multiple of 4 arrives intact, which exercises the FIFO tail lanes (e.g. a 13-byte MSC CSW).
5. **USB host:** `uac-host-firmware` enumerates a USB audio device. `vbus-probe` reports VBUS. `deluge-controller` in host mode works with a MIDI keyboard.
6. **DelugeFirmware:** it boots, plays audio, enumerates USB MIDI and loads samples from SD.
