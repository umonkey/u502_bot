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
