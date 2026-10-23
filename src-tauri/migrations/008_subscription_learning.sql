-- Derived results are versioned separately from authoritative observations.
CREATE TABLE quota_calibrations (
    calibration_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    source_id TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    data TEXT NOT NULL CHECK (json_valid(data)),
    PRIMARY KEY (calibration_id, revision)
);
CREATE INDEX quota_calibrations_source_time ON quota_calibrations(source_id, updated_at);
PRAGMA user_version=8;
