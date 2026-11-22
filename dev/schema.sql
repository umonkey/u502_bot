-- Initial schema for u502_bot. Applied on every startup, so statements must be idempotent.

CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY,
    chat_id INTEGER NOT NULL,
    message_id INTEGER NOT NULL,
    received_at INTEGER NOT NULL, -- Unix time in seconds, UTC
    sender_id INTEGER,
    sender_name TEXT,
    message TEXT,
    UNIQUE (chat_id, message_id)
);

CREATE TABLE IF NOT EXISTS attachments (
    id INTEGER PRIMARY KEY,
    chat_id INTEGER NOT NULL,
    message_id INTEGER NOT NULL,
    kind TEXT NOT NULL, -- 'audio' or 'video'
    file_id TEXT NOT NULL,
    mime_type TEXT,
    duration_seconds INTEGER,
    file_size INTEGER,
    transcribed INTEGER NOT NULL DEFAULT 0, -- processed: transcribed or deliberately skipped
    UNIQUE (chat_id, message_id)
);

CREATE INDEX IF NOT EXISTS attachments_pending ON attachments (id) WHERE transcribed = 0;

CREATE TABLE IF NOT EXISTS outbox (
    id INTEGER PRIMARY KEY,
    chat_id INTEGER NOT NULL,
    thread_id INTEGER,        -- Telegram message id the reply answers; NULL for a plain message
    added_at INTEGER NOT NULL, -- Unix time in seconds, UTC
    sent_at INTEGER,          -- Unix time in seconds, UTC; NULL while unsent
    message TEXT NOT NULL     -- escaped Telegram HTML, rendered by the producer
);

CREATE INDEX IF NOT EXISTS outbox_pending ON outbox (id) WHERE sent_at IS NULL;

-- Serves the message prune and the attachment prune's join.
CREATE INDEX IF NOT EXISTS messages_received_at ON messages (received_at);

-- Serves the outbox prune.
CREATE INDEX IF NOT EXISTS outbox_sent ON outbox (sent_at) WHERE sent_at IS NOT NULL;

-- One row per chat the bot has observed; the owner's status gates transcription.
CREATE TABLE IF NOT EXISTS chats (
    chat_id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL,                      -- 'private', 'group', 'supergroup', or 'channel'
    title TEXT,                              -- absent for a private chat
    status TEXT NOT NULL DEFAULT 'pending',  -- 'pending', 'approved', or 'blocked'
    added_at INTEGER NOT NULL,               -- Unix time in seconds, UTC
    seen_at INTEGER NOT NULL                 -- Unix time in seconds, UTC
);
