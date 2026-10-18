CREATE INDEX legacy_usage_event_time ON legacy_usage_segments(event_time);
CREATE INDEX usage_aggregate_event_time ON usage_aggregates(event_time);
CREATE INDEX context_snapshot_event_time ON context_snapshots(event_time);
CREATE INDEX timing_observation_event_time ON timing_observations(event_time);
CREATE INDEX fact_observation_event_time ON fact_observations(event_time);
PRAGMA user_version=6;
