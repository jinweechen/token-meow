CREATE TABLE usage_plans (
    plan_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    source_id TEXT NOT NULL,
    data TEXT NOT NULL CHECK (json_valid(data)),
    PRIMARY KEY (plan_id, revision)
);
CREATE INDEX usage_plans_source ON usage_plans(source_id, plan_id, revision);
CREATE TABLE plan_period_state (
    owner_kind TEXT NOT NULL CHECK (owner_kind IN ('plan', 'quota_watch')),
    owner_id TEXT NOT NULL,
    period_instance_id TEXT NOT NULL,
    data TEXT NOT NULL CHECK (json_valid(data)),
    PRIMARY KEY (owner_kind, owner_id, period_instance_id)
);
CREATE INDEX subscription_observation_scope_time ON account_snapshots(
    json_extract(data, '$.sourceId'), collected_at
) WHERE json_extract(data, '$.documentKind') = 'subscription_observation';
CREATE UNIQUE INDEX subscription_observation_sequence ON account_snapshots(
    json_extract(data, '$.runtimeId'),
    json_extract(data, '$.sourceId'),
    json_extract(data, '$.bindingGeneration'),
    json_extract(data, '$.clearGeneration'),
    json_extract(data, '$.observationSequence')
) WHERE json_extract(data, '$.documentKind') = 'subscription_observation';
PRAGMA user_version=7;
