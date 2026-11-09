CREATE TABLE attachments (
    id TEXT PRIMARY KEY, host_id TEXT NOT NULL, storage_name TEXT NOT NULL UNIQUE,
    original_name TEXT NOT NULL, byte_length INTEGER NOT NULL CHECK(byte_length >= 0 AND byte_length <= 209715200),
    content_type TEXT NOT NULL, sha256 TEXT NOT NULL, created_at TEXT NOT NULL
  ); CREATE TABLE message_attachments (
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE, attachment_index INTEGER NOT NULL CHECK(attachment_index >= 0),
    attachment_id TEXT NOT NULL REFERENCES attachments(id), PRIMARY KEY(message_id,attachment_index)
  ); CREATE INDEX message_attachments_by_attachment ON message_attachments(attachment_id);