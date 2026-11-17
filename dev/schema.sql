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
    message TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS outbox_pending ON outbox (id) WHERE sent_at IS NULL;
