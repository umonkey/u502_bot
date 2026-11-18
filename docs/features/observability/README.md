# Observability

The bot writes structured logs and, when a Sentry DSN is configured, reports errors, logs, and metrics there.

## Logging

`RUST_LOG` controls console output only and defaults to `info`. Compose sets the default and passes a host or `.env` value through; see [installation](../../Install.md). It does not affect what Sentry receives.

## Sentry

Sentry is optional. A missing, blank, or invalid DSN logs one warning and the bot runs without reporting. The DSN resolves from `SENTRY_DSN_FILE` first and `SENTRY_DSN` second, like every other variable; see [configuration](../configuration/README.md).

## Issues and logs

- every `ERROR`-level event and every panic becomes a Sentry issue
- every `INFO`-or-higher event from this crate becomes a structured log
- events from dependencies become issues but not logs

## Metrics

Application counters and timings are recorded as Sentry metrics. The pairs are meant to be compared: the gap between the first and the second of a pair is the work that did not complete.

- `telegram.messages.received` and `telegram.messages.stored`: the gap is messages that failed to persist
- `telegram.attachments.audio` and `telegram.attachments.video`, counted per received attachment, and `telegram.attachments.stored`, counted per successful statement
- `telegram.attachments.transcribed`: every attachment the transcriber finishes with, whether transcribed, silent, or skipped
- `outbox.messages.added` and `outbox.messages.sent`: the gap is replies not yet delivered
- `openrouter.calls` and `openrouter.duration` for every call, successful or not, plus `openrouter.tokens.input`, `openrouter.tokens.output`, and `openrouter.cost_micro_usd` from the usage OpenRouter reports; cost is in micro-USD
- `ffmpeg.calls` and `ffmpeg.duration` for every extraction, successful or timed out, so the video path is visible even when it never reaches the model

No metric carries attributes.

## Privacy

The reported logs include message text, transcripts, chat ids, sender names, file ids, MIME types, and file names. Point the DSN at a project whose members may see that content. The DSN, the bot token, and the OpenRouter key are never logged, and the Sentry SDK's automatic PII enrichment stays off, so it adds nothing beyond what the bot reports itself.

## Shutdown

Sentry flushes after the services stop, so the last errors of a graceful shutdown are not lost. The sequence is in [communication](../communication/README.md).

## Further reading

- [ADR 0004: report errors and metrics to Sentry](../../adr/0004-sentry-observability.md)
- [ADR 0007: send application logs to Sentry](../../adr/0007-send-application-logs-to-sentry.md)
