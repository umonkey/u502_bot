# ADR 0011: Extract video audio with ffmpeg

- Status: accepted
- Date: 2026-11-18

## Context

A video message arrives as a container: the speech is wrapped in a stream the transcription model's
audio content part cannot take. Transcription already accepts one audio file, so a video has to be
reduced to that before it can join the same path. The reduction needs a media tool, which is a new
system dependency, another failure mode, and a step that spends CPU on a host that already runs the
bot.

## Decision

Extract the first audio track with the `ffmpeg` command-line program, run as a subprocess rather than
linked into the bot, and install that program in the runtime image. Reduce the track in one pass to
Ogg/Opus, mono, at a speech sample rate, which is the format the model in use already accepts.
Bound the extraction by a timeout and kill the child when it is dropped, so an abandoned extraction
cannot outlive a shutdown.

Transcode rather than stream-copy, so every input becomes one known-good format instead of the
format its source happened to use. Skip a video the platform will not serve, or one that carries no
audio track, and mark it processed: retrying cannot change either outcome. Keep the target format
fixed rather than configurable.

## Consequences

- the runtime image grows by a few hundred megabytes, and a local run needs the program on the host
- a host without the program still transcribes audio; every video is skipped and logged
- the alternative rejected when transcription was introduced, transcoding to a preferred audio
  format, is now accepted, for video only
- extraction cost is bounded by the same age window that bounds retries, and a long video is
  otherwise bounded only by the model call
- a skipped video leaves a transcript-shaped hole without a reply, visible only in the logs
- a model that accepts only mp3 or wav would need both the extraction target and the declared format
  changed, because the two are fixed together

## Alternatives considered

- Pure Rust demux, decode, and encode: no external binary, but partial codec coverage and immature
  encoders for far more code than the problem justifies.
- libav bindings in-process: the same engine without a subprocess, but it adds build-time system
  dependencies and unsafe FFI, and an in-process decode offers no safe cancellation point inside the
  shutdown budget.
- Send the video to the model directly: the chat completions schema has no video content part.
- Stream-copy and remux instead of transcoding: cheaper, but the output format would follow the
  source codec, forcing a probe and a format table, and forwarding formats the model may reject.
