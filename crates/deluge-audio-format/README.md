# deluge-audio-format

WAV/RIFF and AIFF/FORM header decoding for `no_std`. Give it the file's size; it asks for one
small window of bytes at a time (at most `MAX_WINDOW_BYTES`, at an offset it chooses) and returns
an `AudioHeader`:

- container, channel count (1 or 2), byte depth (8/16/24/32-bit integer, 32-bit float), sample rate
- where the audio data starts and how long it is
- loop points (`smpl`; AIFF `MARK` + `INST` sustain loop) and root note (`smpl`/`inst`/`INST`)
- wavetable hints (the Serum `clm ` tag), with the acceptance policy in `wavetable`

It reads no files, allocates nothing, decodes no audio and does not panic on any input. The
caller owns every read, so an async task can drive the parse straight off an SD card, and the
walk skips over the audio payload without reading it.

```rust,ignore
use deluge_audio_format::{FeedResult, HeaderParser};

let mut parser = HeaderParser::new(file_size);
let header = loop {
    let (start, len) = parser.pending().expect("a walk in progress always wants bytes");
    let window = read_at(start, len).await?; // your I/O
    match parser.feed(&window, start)? {
        FeedResult::Done(header) => break header,
        FeedResult::Need { .. } => continue,
    }
};
```

`parse(&bytes)` does the same over a slice for callers that hold the whole file.

## Features

- `c-abi`: the C ABI over the parser (DelugeFirmware's `libdeluge/audio_format.h`).

## Fuzzing

```sh
cd crates/deluge-audio-format/fuzz && cargo +nightly fuzz run parse
```

## Provenance

Migrated from DelugeFirmware's `crates/deluge_audio_format`, where it replaced the C++ header
decoder.
