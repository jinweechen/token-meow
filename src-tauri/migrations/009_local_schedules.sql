ALTER TABLE source_instances ADD COLUMN configuration_revision INTEGER NOT NULL DEFAULT 0;

CREATE TABLE local_schedule_templates (
    id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    source_id TEXT NOT NULL,
    data TEXT NOT NULL CHECK (json_valid(data)),
    PRIMARY KEY (id, revision)
);
CREATE INDEX local_schedule_templates_source ON local_schedule_templates(source_id, id, revision);

CREATE TABLE local_schedule_instances (
    id TEXT PRIMARY KEY,
    template_id TEXT NOT NULL,
    template_revision INTEGER NOT NULL,
    source_id TEXT NOT NULL,
    schedule_epoch TEXT NOT NULL,
    period_start TEXT NOT NULL,
    period_end TEXT NOT NULL,
    token_plan_id TEXT,
    data TEXT NOT NULL CHECK (json_valid(data)),
    UNIQUE (template_id, schedule_epoch, period_start),
    FOREIGN KEY (template_id, template_revision) REFERENCES local_schedule_templates(id, revision)
);
CREATE INDEX local_schedule_instances_source_time ON local_schedule_instances(source_id, period_start, id);

CREATE TABLE local_manual_consumption (
    id TEXT PRIMARY KEY,
    instance_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    event_time TEXT NOT NULL,
    amount_minor INTEGER NOT NULL CHECK (amount_minor >= 0),
    kind TEXT NOT NULL CHECK (kind IN ('consume', 'void')),
    target_entry_id TEXT,
    data TEXT NOT NULL CHECK (json_valid(data)),
    FOREIGN KEY (instance_id) REFERENCES local_schedule_instances(id),
    FOREIGN KEY (target_entry_id) REFERENCES local_manual_consumption(id),
    CHECK ((kind='consume' AND amount_minor>0 AND target_entry_id IS NULL)
        OR (kind='void' AND amount_minor=0 AND target_entry_id IS NOT NULL))
);
CREATE UNIQUE INDEX local_manual_consumption_void_once ON local_manual_consumption(target_entry_id) WHERE kind='void';
CREATE INDEX local_manual_consumption_instance_time ON local_manual_consumption(instance_id, event_time, id);
PRAGMA user_version=9;
