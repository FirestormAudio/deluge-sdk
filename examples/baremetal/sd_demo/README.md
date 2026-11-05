# `sd_demo`

SD-card files through the SDK's `Sd`:

1. **Round trip:** writes `SDKTEST.TXT` to the card's root and reads it back.
2. **Stream:** opens `sd_demo_stream.bin` (8 MiB), reads it to the end in 32 KiB chunks into an
   `AlignedBuf` (the full-speed path: whole multi-block transfers straight from the card), checks
   every byte against the test pattern, and logs the throughput.
3. **Seek:** seeks to 4 MiB + 4 in the same file and checks the 4 bytes there.

Each step logs its result; the SYNC LED is solid when all three pass and blinks fast otherwise.

## The stream file

Make it on the host and copy it to the card's root directory:

```sh
python3 tools/make_stream_file.py /path/to/the/card
```

Byte `i` of the file is byte `i % 4` of the little-endian word `i / 4`, where word `k` is
`k * 0x9E3779B9` (mod 2³²).

## Running it

- **On the Deluge**, with RTT logging: `cargo build-fw -p sd_demo --features rtt`, then load it
  through a debug probe and read RTT channel 0. Lines look like:

  ```text
  INFO  [sd_demo] round trip: OK
  INFO  [sd_demo] stream: 8192 KiB in <ms> ms = <rate> KiB/s
  INFO  [sd_demo] seek: OK
  INFO  [sd_demo] sd_demo: all OK
  ```

- **In the simulator**, the card is a host directory: put the stream file in it and point
  `DELUGE_SIM_SD` at it, then `cargo deluge sim` (with this crate's `sim` feature).
