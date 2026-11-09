ALTER TABLE conversations ADD COLUMN name TEXT;
  ALTER TABLE conversations ADD COLUMN created_at TEXT;
  UPDATE conversations SET name = 'Everyone' WHERE id = 'team';
  CREATE TABLE room_members (
    room_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
    PRIMARY KEY (room_id, bot_id));