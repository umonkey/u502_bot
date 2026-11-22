# ADR 0017: Export one local day of a chat to Markdown

- Status: accepted
- Date: 2026-11-22

## Context

The store is a local file with no browsing interface, and the retention eventually deletes old rows. An
operator who wants to read or search a day of a chat has to open the database directly, and the clock
times they care about are local ones rather than the instants the store records. The export must run
beside the bot against the same file, must not need the messaging or model credentials, and must not
depend on monitoring being configured.

## Decision

Add a one-shot command that reads the shared store and writes one Markdown file per chat for one
calendar day. It defaults to yesterday and to every chat with a message that day, and it accepts an
explicit chat and an output directory. Day boundaries and clock times use the host's local time zone,
and an unresolvable zone fails the export rather than silently exporting a day in UTC. Rows without text
are omitted. Files are overwritten, so re-running the command is idempotent.

The command issues only reads and never applies the startup schema. It does not install logging or
reporting: file paths go to standard output, failures to standard error, and the exit status says
whether anything failed.

## Consequences

- the date arithmetic needs a runtime dependency and zone data the runtime image does not otherwise
  guarantee, so the image grows slightly
- export reads while the bot runs: the store's write-ahead logging lets it see the last committed
  snapshot without waiting for the ingestor
- the two positional arguments force a date even when only one chat is wanted, because the chat is the
  second position
- a day whose messages are all untitled media or service events produces no file, so "nothing to
  export" is a normal outcome rather than an error
- the container's working directory is not writable by the bot user, so the output directory must be
  passed explicitly when running in the image
- there is no telemetry: a failed export is visible only in the exit status and the message on standard
  error

## Alternatives considered

- Export a UTC day: simpler and zone-free, but it shifts evening messages into the wrong file for an
  operator east or west of UTC.
- Take a fixed offset from configuration: no zone data needed, but daylight-saving days break and the
  operator has to maintain the value.
- Append to one running log: continuous, but re-running duplicates rows and the file grows without a
  boundary.
- Write one file for all chats: fewer files, but a chat that is shared independently cannot be handed
  over on its own.
