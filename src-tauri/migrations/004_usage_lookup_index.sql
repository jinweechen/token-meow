CREATE INDEX usage_source_turn_thread ON usage_responses(source_id,turn_id,thread_id);
PRAGMA user_version=4;
