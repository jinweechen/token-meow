//! Lossless storage encoding and bounded evidence retention. Storage hashes
//! establish reconstruction integrity, never adapter/identity qualification.
//! Original observations and derived calibration patches remain separate.
use super::super::{err, hash, Result};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const STORAGE: &str = "subscriptionStorage";
const EVIDENCE: &str = "storageEvidence";
const DERIVED: &str = "derivedCalibrationPatches";
const SUMMARY: &str = "subscriptionEvidenceRetention:v1";
const MAX_ROW_BYTES: usize = 512 * 1024;
const MAX_PATCHES: usize = 2048;
const MAX_ROWS: i64 = 100_000;
const SOURCE_BYTES: i64 = 32 * 1024 * 1024;
const TOTAL_BYTES: i64 = 128 * 1024 * 1024;
const HOLDOUT_PREFIX: &str = "subscriptionHoldout:";
const HOLDOUT_COUNT: usize = 100;
const HOLDOUT_BYTES: usize = 256 * 1024;
const HEARTBEAT_SECONDS: i64 = 600;
const MAX_PROJECTED_ROWS: usize = 1024;
const MAX_PROJECTED_BYTES: usize = 32 * 1024 * 1024;
const HEADER_PROJECTION: &[&str] = &[
    "runtimeId",
    "sourceId",
    "bindingGeneration",
    "identityEpoch",
    "clearGeneration",
    "observationSequence",
    "readStartedAt",
    "readFinishedAt",
    "accountRead",
    "quotaRead",
    "usageRead",
    "identityConsistency",
    "identityConsistencyReason",
];

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Change {
    path: Vec<String>,
    remove: bool,
    value: Value,
}
fn timestamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}
fn instant(v: &Value) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(v.as_str()?)
        .ok()
        .map(|v| v.with_timezone(&Utc))
}
fn digest(v: &Value) -> String {
    let mut value = v.clone();
    value.sort_all_objects();
    hash(value.to_string().as_bytes())
}
fn is_receipt(v: &Value) -> bool {
    v["documentKind"] == "subscription_observation" && v["schemaVersion"] == 1 && v.is_object()
}
fn original(v: &Value) -> Value {
    let mut raw = v.clone();
    if let Some(object) = raw.as_object_mut() {
        for key in [STORAGE, EVIDENCE, DERIVED] {
            object.remove(key);
        }
    }
    raw
}
fn subread_static(v: &mut Value) {
    if let Some(object) = v.as_object_mut() {
        for key in [
            "requestStartedAt",
            "responseReceivedAt",
            "freshnessAnchor",
            "factTimeLower",
            "factTimeUpper",
        ] {
            object.remove(key);
        }
    }
}
/// The original v1 projection remains valid for already persisted digests.
fn legacy_static_value(receipt: &Value) -> Value {
    let mut value = original(receipt);
    for target in ["outer", "observation"] {
        let current = if target == "outer" {
            &mut value
        } else {
            &mut value["observation"]
        };
        if let Some(object) = current.as_object_mut() {
            for key in [
                "observationSequence",
                "lastSettledSequence",
                "lastPersistedSequence",
                "readStartedAt",
                "readFinishedAt",
                "collectedAt",
                "lastAttemptAt",
                "persisted",
                "asOfRevision",
                "calibrationAssembly",
            ] {
                object.remove(key);
            }
            for key in ["accountRead", "quotaRead", "usageRead"] {
                if let Some(read) = object.get_mut(key) {
                    subread_static(read);
                }
            }
        }
    }
    value
}
fn exact_keys(value: &Value, keys: &[&str]) -> bool {
    value.as_object().is_some_and(|object| {
        object.len() == keys.len() && keys.iter().all(|key| object.contains_key(*key))
    })
}
fn known_observation_proof(proof: &Value) -> bool {
    if proof["schemaVersion"] != 1
        || proof["algorithmVersion"] != "adapter-observation-proof-v1"
        || !exact_keys(
            proof,
            &[
                "schemaVersion",
                "algorithmVersion",
                "nativeDigest",
                "scope",
                "snapshotFields",
                "windowFields",
                "metadataDigest",
            ],
        )
        || !exact_keys(
            &proof["scope"],
            &[
                "runtimeId",
                "sourceId",
                "bindingGeneration",
                "identityEpoch",
                "clearGeneration",
                "observationSequence",
            ],
        )
        || !exact_keys(
            &proof["snapshotFields"],
            &[
                "identityEvidence",
                "entitlementSegment",
                "entitlementEvidence",
                "clockEpoch",
            ],
        )
        || !exact_keys(
            &proof["snapshotFields"]["identityEvidence"],
            &["adapterId", "reference"],
        )
        || !exact_keys(
            &proof["snapshotFields"]["entitlementEvidence"],
            &[
                "verified",
                "source",
                "segmentId",
                "ruleVersion",
                "reference",
            ],
        )
    {
        return false;
    }
    proof["windowFields"].as_object().is_some_and(|windows| {
        !windows.is_empty()
            && windows.len() <= 200
            && windows.values().all(|window| {
                let Some(fields) = window.as_object() else {
                    return false;
                };
                let required = [
                    "roundingStep",
                    "saturated",
                    "saturationHeadroomBounded",
                    "ruleVersion",
                    "windowInstanceId",
                    "recovery",
                    "windowInstanceEvidence",
                ];
                if !required.iter().all(|key| fields.contains_key(*key))
                    || fields
                        .keys()
                        .any(|key| !required.contains(&key.as_str()) && key != "resetEvidence")
                    || !exact_keys(
                        &window["windowInstanceEvidence"],
                        &[
                            "verified",
                            "source",
                            "stableAcrossRuntimes",
                            "windowInstanceId",
                            "reference",
                        ],
                    )
                {
                    return false;
                }
                fields.get("resetEvidence").is_none_or(|reset| {
                    exact_keys(
                        reset,
                        &[
                            "confirmedNewWindow",
                            "confirmedSameWindow",
                            "windowInstanceId",
                            "previousWindowInstanceId",
                            "nextReset",
                            "actualBoundary",
                            "finalUsedPercent",
                            "adapterId",
                            "reference",
                            "previousSequence",
                            "previousNativeDigest",
                        ],
                    ) && ["nextReset", "actualBoundary"].iter().all(|key| {
                        reset[*key].is_null() || exact_keys(&reset[*key], &["lower", "upper"])
                    })
                })
            })
    })
}
/// The proof's native digest binds every original read time and sequence. Those
/// bindings are dynamic storage receipts, not additional training observations.
/// Keep all stable evidence and every reset transition; unknown shapes stay full.
fn static_value(receipt: &Value) -> Value {
    let mut value = legacy_static_value(receipt);
    if let Some(proof) = value
        .get_mut("derivedObservationProof")
        .filter(|p| known_observation_proof(p))
    {
        let proof = proof.as_object_mut().unwrap();
        proof.remove("nativeDigest");
        proof.remove("metadataDigest");
        proof
            .get_mut("scope")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("observationSequence");
    }
    value
}
fn static_digest_matches(value: &Value, expected: &Value) -> bool {
    expected.as_str().is_some_and(|expected| {
        expected == digest(&static_value(value)) || expected == digest(&legacy_static_value(value))
    })
}
fn diff(base: &Value, next: &Value, path: &mut Vec<String>, changes: &mut Vec<Change>) {
    if base == next {
        return;
    }
    if let (Some(a), Some(b)) = (base.as_object(), next.as_object()) {
        let keys = a.keys().chain(b.keys()).collect::<BTreeSet<_>>();
        for key in keys {
            path.push(key.clone());
            match (a.get(key), b.get(key)) {
                (Some(a), Some(b)) => diff(a, b, path, changes),
                (_, value) => changes.push(Change {
                    path: path.clone(),
                    remove: value.is_none(),
                    value: value.cloned().unwrap_or(Value::Null),
                }),
            }
            path.pop();
        }
    } else {
        changes.push(Change {
            path: path.clone(),
            remove: false,
            value: next.clone(),
        });
    }
}
fn patch(mut body: Value, changes: Vec<Change>) -> Option<Value> {
    if changes.len() > MAX_PATCHES {
        return None;
    }
    let mut seen = BTreeSet::new();
    for change in changes {
        if change.path.len() > 32
            || change.path.iter().any(|p| p.len() > 2048)
            || !seen.insert(change.path.clone())
        {
            return None;
        }
        if change.path.is_empty() {
            if change.remove {
                return None;
            }
            body = change.value;
            continue;
        }
        let (last, parents) = change.path.split_last()?;
        let mut current = &mut body;
        for key in parents {
            current = current.as_object_mut()?.get_mut(key)?;
        }
        let object = current.as_object_mut()?;
        if change.remove {
            object.remove(last)?;
        } else {
            object.insert(last.clone(), change.value);
        }
    }
    Some(body)
}
fn parse_raw(db: &Connection, id: i64) -> Result<Option<Value>> {
    let raw: Option<String> = db
        .query_row(
            "SELECT data FROM account_snapshots WHERE id=?1 AND length(CAST(data AS BLOB))<=?2",
            params![id, MAX_ROW_BYTES as i64],
            |r| r.get(0),
        )
        .optional()
        .map_err(err)?;
    Ok(raw.and_then(|v| serde_json::from_str(&v).ok()))
}
fn unavailable(stored: &Value, reason: &str) -> Value {
    let mut value = original(stored);
    value["observation"] = Value::Null;
    value[EVIDENCE] = json!({"schemaVersion":1,"available":false,"reason":reason,"independentTrainingSample":false,"originalDigest":stored[STORAGE]["originalReceiptDigest"]});
    value
}
fn verify_full(stored: &Value) -> Option<Value> {
    if !is_receipt(stored)
        || stored[STORAGE]["schemaVersion"] != 1
        || stored[STORAGE]["encoding"] != "full"
    {
        return None;
    }
    let value = original(stored);
    (stored[STORAGE]["originalReceiptDigest"].as_str() == Some(digest(&value).as_str())
        && static_digest_matches(&value, &stored[STORAGE]["staticDigest"]))
    .then_some(value)
}

/// Returns complete legacy or verified original data. Missing/corrupt bases
/// become auditable tombstones, not a new observation or a renewed timestamp.
pub(crate) fn resolve(db: &Connection, stored: &Value) -> Result<Value> {
    if stored.get(STORAGE).is_none() {
        return Ok(stored.clone());
    }
    if !is_receipt(stored) || stored[STORAGE]["schemaVersion"] != 1 {
        return Ok(unavailable(stored, "storage_encoding_unsupported"));
    }
    let mut value = match stored[STORAGE]["encoding"].as_str() {
        Some("full") => match verify_full(stored) {
            Some(v) => v,
            None => return Ok(unavailable(stored, "storage_digest_mismatch")),
        },
        Some("reference") => {
            let Some(id) = stored[STORAGE]["baseReceiptId"]
                .as_i64()
                .filter(|id| *id > 0)
            else {
                return Ok(unavailable(stored, "evidence_removed"));
            };
            let Some(base) = parse_raw(db, id)? else {
                return Ok(unavailable(stored, "evidence_removed"));
            };
            let Some(base_original) = verify_full(&base) else {
                return Ok(unavailable(stored, "evidence_removed"));
            };
            if base[STORAGE]["originalReceiptDigest"] != stored[STORAGE]["baseOriginalDigest"]
                || !static_digest_matches(&base_original, &stored[STORAGE]["staticDigest"])
            {
                return Ok(unavailable(stored, "evidence_removed"));
            }
            let Some(changes) =
                serde_json::from_value::<Vec<Change>>(stored[STORAGE]["observationDelta"].clone())
                    .ok()
            else {
                return Ok(unavailable(stored, "storage_delta_invalid"));
            };
            let keys: Vec<String> = match stored[STORAGE].get("headerProjection") {
                Some(value) => match serde_json::from_value(value.clone()) {
                    Ok(keys) => keys,
                    Err(_) => return Ok(unavailable(stored, "storage_delta_invalid")),
                },
                None => vec![],
            };
            let mut unique = BTreeSet::new();
            if keys.len() > HEADER_PROJECTION.len()
                || keys.iter().any(|key| {
                    !HEADER_PROJECTION.contains(&key.as_str())
                        || stored.get(key).is_none()
                        || !unique.insert(key.clone())
                })
            {
                return Ok(unavailable(stored, "storage_delta_invalid"));
            }
            let mut base_body = base_original["observation"].clone();
            for key in &keys {
                base_body
                    .as_object_mut()
                    .ok_or("压缩基准缺少原始body")?
                    .remove(key);
            }
            let Some(mut body) = patch(base_body, changes) else {
                return Ok(unavailable(stored, "storage_delta_invalid"));
            };
            for key in keys {
                body.as_object_mut()
                    .ok_or("压缩差异没有恢复对象")?
                    .insert(key.clone(), stored[&key].clone());
            }
            let mut value = original(stored);
            value["observation"] = body;
            if stored[STORAGE]["originalReceiptDigest"].as_str() != Some(digest(&value).as_str())
                || !static_digest_matches(&value, &stored[STORAGE]["staticDigest"])
            {
                return Ok(unavailable(stored, "storage_digest_mismatch"));
            }
            value
        }
        _ => return Ok(unavailable(stored, "storage_encoding_unsupported")),
    };
    if let Some(derived) = stored.get(DERIVED) {
        value[DERIVED] = derived.clone();
    }
    value[EVIDENCE] = json!({"schemaVersion":1,"available":true,"independentTrainingSample":stored[STORAGE]["independentTrainingSample"]==true,"originalDigest":stored[STORAGE]["originalReceiptDigest"],"encoding":stored[STORAGE]["encoding"]});
    Ok(value)
}

fn deferred_header(stored: &Value, limit: &str) -> Value {
    let mut value = Value::Object(
        stored
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(key, _)| {
                !matches!(key.as_str(), "observation" | STORAGE | EVIDENCE | DERIVED)
            })
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    );
    value["observation"] = Value::Null;
    let deferred_pair_windows = stored[DERIVED]
        .as_object()
        .into_iter()
        .flat_map(|v| v.iter())
        .filter(|(_, patch)| patch["windowFields"]["calibrationEvidence"].is_object())
        .map(|(window, _)| window.clone())
        .collect::<Vec<_>>();
    value[EVIDENCE] = json!({"schemaVersion":1,"available":false,"reason":"projection_budget_exceeded","evidenceRemoved":false,"limit":limit,"independentTrainingSample":false,"originalDigest":stored[STORAGE]["originalReceiptDigest"],"deferredCalibrationWindows":deferred_pair_windows});
    value
}

fn receipt_lookup_key(raw: &Value, sequence: &Value) -> String {
    json!([
        raw["runtimeId"],
        raw["sourceId"],
        raw["bindingGeneration"],
        raw["identityEpoch"],
        raw["clearGeneration"].to_string(),
        sequence
    ])
    .to_string()
}
fn projection_priorities(db: &Connection, stored: &[Value]) -> Result<Vec<usize>> {
    let first = stored.len().saturating_sub(MAX_PROJECTED_ROWS);
    let index = stored
        .iter()
        .enumerate()
        .map(|(i, r)| (receipt_lookup_key(r, &r["observationSequence"]), i))
        .collect::<BTreeMap<_, _>>();
    let mut pair_rows = BTreeSet::new();
    let mut metadata = BTreeSet::new();
    let mut add_interval = |start: usize, end: usize| {
        if start <= end {
            pair_rows.extend(start..=end);
        }
    };
    for (end, raw) in stored.iter().enumerate() {
        for patch in raw[DERIVED]
            .as_object()
            .into_iter()
            .flat_map(|v| v.values())
        {
            metadata.insert(end);
            if let Some(sequence) =
                patch["windowFields"]["calibrationEvidence"]["fromSequence"].as_str()
            {
                if let Some(start) = index.get(&receipt_lookup_key(raw, &json!(sequence))) {
                    add_interval(*start, end);
                }
            }
        }
    }
    if table_exists(db, "settings") {
        let mut query=db.prepare("SELECT value FROM settings WHERE key LIKE 'subscriptionHoldout:%' ORDER BY key LIMIT 101").map_err(err)?;
        let rows = query
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(err)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(err)?;
        for raw in rows {
            if raw.len() > HOLDOUT_BYTES {
                continue;
            }
            let Ok(policy) = serde_json::from_str::<Value>(&raw) else {
                continue;
            };
            if !stored.iter().any(|r| r["sourceId"] == policy["sourceId"]) {
                continue;
            }
            for assignment in policy["assignments"]
                .as_array()
                .into_iter()
                .flatten()
                .take(64)
            {
                let mut endpoints = vec![];
                for field in ["startReceipt", "endReceipt"] {
                    let Some(id) = assignment[field].as_i64().filter(|id| *id > 0) else {
                        continue;
                    };
                    let header:Option<String>=db.query_row("SELECT json_remove(data,'$.observation','$.subscriptionStorage','$.derivedCalibrationPatches') FROM account_snapshots WHERE id=?1",[id],|r|r.get(0)).optional().map_err(err)?;
                    if let Some(header) =
                        header.and_then(|s| serde_json::from_str::<Value>(&s).ok())
                    {
                        if let Some(index) =
                            index.get(&receipt_lookup_key(&header, &header["observationSequence"]))
                        {
                            endpoints.push(*index);
                        }
                    }
                }
                if endpoints.len() == 2 {
                    add_interval(endpoints[0], endpoints[1]);
                }
            }
        }
    }
    let mut seen = BTreeSet::new();
    Ok((first..stored.len())
        .rev()
        .chain(pair_rows.into_iter().rev())
        .chain(metadata.into_iter().rev())
        .filter(|index| seen.insert(*index))
        .collect())
}

/// A bounded *temporary projection view*, never the archive dependency index.
/// Input is committed DB order (oldest first). Old/over-budget reads remain
/// ordered headers and break native continuity without claiming fact deletion.
/// Archives must use the encoded originals and lazily `resolve` dependencies.
pub(crate) fn resolve_many(db: &Connection, stored: &[Value]) -> Result<Vec<Value>> {
    let first_candidate = stored.len().saturating_sub(MAX_PROJECTED_ROWS);
    let mut selected = Vec::new();
    let mut bytes = 2usize; // JSON array brackets, plus one separator per row below.
    for (index, raw) in stored.iter().enumerate().rev() {
        let header = deferred_header(
            raw,
            if index < first_candidate {
                "recent-row-count"
            } else {
                "expanded-payload"
            },
        );
        let size = header.to_string().len() + 1;
        if bytes
            .checked_add(size)
            .is_some_and(|next| next <= MAX_PROJECTED_BYTES)
        {
            bytes += size;
            selected.push((index, header, size));
        }
    }
    // Newest evidence receives expansion budget first. At most one resolved
    // body is temporary, so compact receipts cannot amplify RAM without bound.
    let positions = selected
        .iter()
        .enumerate()
        .map(|(i, (index, _, _))| (*index, i))
        .collect::<BTreeMap<_, _>>();
    for index in projection_priorities(db, stored)? {
        let Some(position) = positions.get(&index) else {
            continue;
        };
        let (_, value, previous_size) = &mut selected[*position];
        let raw = &stored[index];
        let hint = raw[STORAGE]["originalBytes"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or_else(|| raw.to_string().len());
        if hint
            > MAX_PROJECTED_BYTES
                .saturating_sub(bytes)
                .saturating_add(*previous_size)
        {
            continue;
        }
        let resolved = resolve(db, raw)?;
        let size = resolved.to_string().len() + 1;
        if bytes
            .checked_sub(*previous_size)
            .and_then(|n| n.checked_add(size))
            .is_some_and(|next| next <= MAX_PROJECTED_BYTES)
        {
            bytes = bytes - *previous_size + size;
            *value = resolved;
            *previous_size = size;
        }
    }
    selected.reverse();
    Ok(selected.into_iter().map(|(_, value, _)| value).collect())
}

/// Encode only the next already-admitted receipt in its writer transaction.
/// Every genuine round still has its own row, header, time bounds and sequence.
pub(crate) fn encode(db: &Connection, receipt: &Value, at: DateTime<Utc>) -> Result<Value> {
    if db.is_autocommit() {
        return Err("观测压缩必须在写入事务内执行".into());
    }
    if !is_receipt(receipt)
        || receipt.get(STORAGE).is_some()
        || receipt.get(EVIDENCE).is_some()
        || receipt.to_string().len() > MAX_ROW_BYTES
    {
        return Err("原始观测格式或存储预算无效".into());
    }
    let raw = original(receipt);
    let static_digest = digest(&static_value(&raw));
    let source = receipt["sourceId"].as_str().ok_or("观测来源缺失")?;
    let previous:Option<(i64,String)>=db.query_row("SELECT id,data FROM account_snapshots WHERE json_extract(data,'$.documentKind')='subscription_observation' AND json_extract(data,'$.sourceId')=?1 ORDER BY id DESC LIMIT 1",[source],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(err)?;
    let mut base = None;
    let mut same = false;
    if let Some((id, stored)) =
        previous.and_then(|(id, v)| serde_json::from_str::<Value>(&v).ok().map(|v| (id, v)))
    {
        let resolved = resolve(db, &stored)?;
        let newer = receipt["observationSequence"]
            .as_str()
            .and_then(|s| s.parse::<u64>().ok())
            .zip(
                resolved["observationSequence"]
                    .as_str()
                    .and_then(|s| s.parse::<u64>().ok()),
            )
            .is_some_and(|(a, b)| a > b);
        same = newer
            && raw["outcome"] == "success"
            && raw["observation"].is_object()
            && resolved["observation"].is_object()
            && static_value(&raw) == static_value(&resolved);
        if same {
            let candidate = if stored[STORAGE]["encoding"] == "full" {
                Some((id, stored))
            } else if stored[STORAGE]["encoding"] == "reference" {
                stored[STORAGE]["baseReceiptId"]
                    .as_i64()
                    .and_then(|id| parse_raw(db, id).ok().flatten().map(|v| (id, v)))
            } else {
                None
            };
            if let Some((id, candidate)) = candidate {
                if let Some(anchor) = instant(&candidate[STORAGE]["anchoredAt"]) {
                    let age = (at - anchor).num_milliseconds();
                    if (0..HEARTBEAT_SECONDS * 1000).contains(&age)
                        && verify_full(&candidate).is_some()
                    {
                        base = Some((id, candidate));
                    }
                }
            }
        }
    }
    let mut full = receipt.clone();
    full[STORAGE] = json!({"schemaVersion":1,"encoding":"full","digestVersion":"sha256-canonical-original-receipt-v1","originalReceiptDigest":digest(&raw),"staticDigest":static_digest,"anchoredAt":timestamp(at),"originalBytes":raw.to_string().len(),"independentTrainingSample":!same});
    if let Some((id, base)) = base {
        let mut changes = vec![];
        let keys = HEADER_PROJECTION
            .iter()
            .filter(|key| {
                raw.get(**key).is_some() && raw["observation"].get(**key) == raw.get(**key)
            })
            .map(|key| key.to_string())
            .collect::<Vec<_>>();
        let mut base_body = base["observation"].clone();
        let mut next_body = raw["observation"].clone();
        for key in &keys {
            base_body
                .as_object_mut()
                .ok_or("压缩基准body无效")?
                .remove(key);
            next_body.as_object_mut().ok_or("原始body无效")?.remove(key);
        }
        diff(&base_body, &next_body, &mut vec![], &mut changes);
        if changes.len() <= MAX_PATCHES {
            let mut reference = receipt.clone();
            reference.as_object_mut().unwrap().remove("observation");
            reference[STORAGE] = json!({"schemaVersion":1,"encoding":"reference","digestVersion":"sha256-canonical-original-receipt-v1","originalReceiptDigest":digest(&raw),"staticDigest":static_digest,"baseReceiptId":id,"baseOriginalDigest":base[STORAGE]["originalReceiptDigest"],"headerProjection":keys,"observationDelta":changes,"originalBytes":raw.to_string().len(),"independentTrainingSample":false});
            if reference.to_string().len() <= MAX_ROW_BYTES
                && reference.to_string().len() < full.to_string().len()
            {
                return Ok(reference);
            }
        }
    }
    if full.to_string().len() > MAX_ROW_BYTES {
        return Err("完整观测超过有界存储上限".into());
    }
    Ok(full)
}

/// The assembler may append only its validated derived namespace. It must not
/// accidentally expand references or change any original field while doing so.
pub(crate) fn with_derived(db: &Connection, stored: &Value, updated: &Value) -> Result<Value> {
    let resolved = resolve(db, stored)?;
    if resolved[EVIDENCE]["available"] == false
        || original(&resolved) != original(updated)
        || resolved.get(EVIDENCE) != updated.get(EVIDENCE)
    {
        return Err("派生补丁改变了原始观测或引用已失效".into());
    }
    let mut value = stored.clone();
    if let Some(namespace) = updated.get(DERIVED) {
        if !namespace.is_object() {
            return Err("派生观测命名空间无效".into());
        }
        value[DERIVED] = namespace.clone();
    } else {
        value.as_object_mut().ok_or("观测格式无效")?.remove(DERIVED);
    }
    if value.to_string().len() > MAX_ROW_BYTES {
        return Err("派生观测超过存储上限".into());
    }
    Ok(value)
}

fn setting(db: &Connection, key: &str) -> Result<Option<Value>> {
    let value: Option<String> = db
        .query_row("SELECT value FROM settings WHERE key=?1", [key], |r| {
            r.get(0)
        })
        .optional()
        .map_err(err)?;
    value
        .map(|v| serde_json::from_str(&v).map_err(err))
        .transpose()
}
fn table_exists(db: &Connection, name: &str) -> bool {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [name],
        |r| r.get::<_, bool>(0),
    )
    .unwrap_or(false)
}
fn limits() -> Value {
    json!({"perSourceReceipts":MAX_ROWS,"perSourcePayloadBytes":SOURCE_BYTES,"totalPayloadBytes":TOTAL_BYTES,"maxDays":90})
}
fn days(n: i64) -> i64 {
    n.clamp(1, 90)
}
fn missing_bases(db: &Connection, source: Option<&str>) -> Result<i64> {
    db.query_row("SELECT COUNT(*) FROM account_snapshots r LEFT JOIN account_snapshots b ON b.id=json_extract(r.data,'$.subscriptionStorage.baseReceiptId') WHERE json_extract(r.data,'$.documentKind')='subscription_observation' AND (?1 IS NULL OR json_extract(r.data,'$.sourceId')=?1) AND json_extract(r.data,'$.subscriptionStorage.encoding')='reference' AND (b.id IS NULL OR json_extract(b.data,'$.subscriptionStorage.encoding') IS NOT 'full' OR json_extract(b.data,'$.subscriptionStorage.originalReceiptDigest') IS NOT json_extract(r.data,'$.subscriptionStorage.baseOriginalDigest'))",[source],|r|r.get(0)).map_err(err)
}
fn earliest(db: &Connection, source: Option<&str>, fact: bool) -> Result<Option<String>> {
    let field = if fact {
        "json_extract(data,'$.quotaRead.factTimeLower')"
    } else {
        "collected_at"
    };
    let sql=format!("SELECT data,{field} FROM account_snapshots WHERE json_extract(data,'$.documentKind')='subscription_observation' AND (?1 IS NULL OR json_extract(data,'$.sourceId')=?1) AND {field} IS NOT NULL ORDER BY julianday({field}),id LIMIT 100000");
    let mut query = db.prepare(&sql).map_err(err)?;
    let rows = query
        .query_map([source], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(err)?;
    for row in rows {
        let (raw, at) = row.map_err(err)?;
        let stored: Value = serde_json::from_str(&raw).map_err(err)?;
        let resolved = resolve(db, &stored)?;
        if resolved["outcome"] == "success"
            && resolved["observation"].is_object()
            && (!fact || instant(&json!(at)).is_some())
        {
            return Ok(Some(at));
        }
    }
    Ok(None)
}
/// Count/byte values are current stored JSON payload, never SQLite/WAL disk size.
pub(crate) fn availability(
    db: &Connection,
    source: Option<&str>,
    _at: DateTime<Utc>,
    retention_days: i64,
) -> Result<Value> {
    let (count,bytes,full,references):(i64,i64,i64,i64)=db.query_row("SELECT COUNT(*),COALESCE(SUM(length(CAST(data AS BLOB))),0),COALESCE(SUM(CASE WHEN json_extract(data,'$.subscriptionStorage.encoding')='full' OR (json_type(data,'$.subscriptionStorage') IS NULL AND json_type(data,'$.observation')='object') THEN 1 ELSE 0 END),0),COALESCE(SUM(CASE WHEN json_extract(data,'$.subscriptionStorage.encoding')='reference' THEN 1 ELSE 0 END),0) FROM account_snapshots WHERE json_extract(data,'$.documentKind')='subscription_observation' AND (?1 IS NULL OR json_extract(data,'$.sourceId')=?1)",[source],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).map_err(err)?;
    let summary=setting(db,SUMMARY)?.unwrap_or_else(||json!({"lastAppliedAt":null,"deletedByAge":0,"deletedBySourceCount":0,"deletedBySourceBytes":0,"deletedByGlobalBytes":0,"deletedTotal":0,"baseDependenciesMissing":0,"scope":"all-subscription-evidence"}));
    Ok(
        json!({"schemaVersion":1,"sourceId":source,"earliestAvailableAt":earliest(db,source,false)?,"earliestRecordedFactAt":earliest(db,source,true)?,"timeBasis":"collected-at-not-provider-fact-time","receiptCount":count,"fullEnvelopeCount":full,"referenceReceiptCount":references,"unresolvedReferenceCount":missing_bases(db,source)?,"payloadBytes":bytes,"retentionDays":days(retention_days),"limits":limits(),"capacityTruncation":summary,"integrityCheckedOnResolve":true,"payloadIsSqliteDiskSize":false}),
    )
}

/// Caller reconciles every affected source's derived archives in this transaction.
pub(crate) fn retain(db: &Connection, at: DateTime<Utc>, retention_days: i64) -> Result<Value> {
    retain_with_limits(db, at, retention_days, MAX_ROWS, SOURCE_BYTES, TOTAL_BYTES)
}
fn retain_with_limits(
    db: &Connection,
    at: DateTime<Utc>,
    retention_days: i64,
    max_rows: i64,
    max_source_bytes: i64,
    max_total_bytes: i64,
) -> Result<Value> {
    if db.is_autocommit() {
        return Err("观测保留必须在领域事务内执行".into());
    }
    let cutoff = timestamp(at - Duration::days(days(retention_days)));
    let age=db.execute("DELETE FROM account_snapshots WHERE json_extract(data,'$.documentKind')='subscription_observation' AND julianday(collected_at)<julianday(?1)",[cutoff]).map_err(err)?;
    let count=db.execute("DELETE FROM account_snapshots WHERE id IN (SELECT id FROM (SELECT id,ROW_NUMBER() OVER(PARTITION BY json_extract(data,'$.sourceId') ORDER BY id DESC) n FROM account_snapshots WHERE json_extract(data,'$.documentKind')='subscription_observation') WHERE n>?1)",[max_rows]).map_err(err)?;
    let source_bytes=db.execute("DELETE FROM account_snapshots WHERE id IN (SELECT id FROM (SELECT id,SUM(length(CAST(data AS BLOB))) OVER(PARTITION BY json_extract(data,'$.sourceId') ORDER BY id DESC) bytes FROM account_snapshots WHERE json_extract(data,'$.documentKind')='subscription_observation') WHERE bytes>?1)",[max_source_bytes]).map_err(err)?;
    let global_bytes=db.execute("DELETE FROM account_snapshots WHERE id IN (SELECT id FROM (SELECT id,SUM(length(CAST(data AS BLOB))) OVER(ORDER BY id DESC) bytes FROM account_snapshots WHERE json_extract(data,'$.documentKind')='subscription_observation') WHERE bytes>?1)",[max_total_bytes]).map_err(err)?;
    let report = json!({"lastAppliedAt":timestamp(at),"retentionDays":days(retention_days),"deletedByAge":age,"deletedBySourceCount":count,"deletedBySourceBytes":source_bytes,"deletedByGlobalBytes":global_bytes,"deletedTotal":age+count+source_bytes+global_bytes,"baseDependenciesMissing":missing_bases(db,None)?,"scope":"all-subscription-evidence"});
    // Preserve the latest actual truncation so frequent zero-deletion scans do
    // not erase the user's explanation of why older evidence disappeared.
    if age + count + source_bytes + global_bytes > 0 || setting(db, SUMMARY)?.is_none() {
        db.execute("INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![SUMMARY,report.to_string()]).map_err(err)?;
    }
    Ok(report)
}

fn manifest_ids(value: &Value, ids: &mut BTreeSet<String>) {
    match value {
        Value::Object(object) => {
            for (key, v) in object {
                if key == "holdoutManifestId" {
                    if let Some(id) = v.as_str() {
                        ids.insert(id.into());
                    }
                }
                manifest_ids(v, ids);
            }
        }
        Value::Array(values) => {
            for v in values {
                manifest_ids(v, ids);
            }
        }
        _ => {}
    }
}
/// Evict only unreferenced policies. A policy with any surviving assignment
/// endpoint or a receipt/archive reference remains protected, including failures.
/// Existing assembler admission also refuses >100 policies or >256 KiB each.
pub(crate) fn retain_holdouts(
    db: &Connection,
    at: DateTime<Utc>,
    retention_days: i64,
) -> Result<Value> {
    if db.is_autocommit() {
        return Err("留出策略保留必须在领域事务内执行".into());
    }
    let mut protected = BTreeSet::new();
    for table in ["account_snapshots", "quota_calibrations"] {
        if !table_exists(db, table) {
            continue;
        }
        let sql = format!("SELECT data FROM {table} WHERE instr(data,'holdoutManifestId')>0");
        let mut query = db.prepare(&sql).map_err(err)?;
        for row in query
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(err)?
        {
            let raw = row.map_err(err)?;
            if let Ok(value) = serde_json::from_str::<Value>(&raw) {
                manifest_ids(&value, &mut protected);
            } else {
                return Err("留出依赖元数据格式无效，停止淘汰".into());
            }
        }
    }
    let mut query = db
        .prepare(
            "SELECT key,value FROM settings WHERE key LIKE 'subscriptionHoldout:%' ORDER BY key",
        )
        .map_err(err)?;
    let rows = query
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(err)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(err)?;
    drop(query);
    let mut policies = vec![];
    for (key, raw) in rows {
        let value: Value = serde_json::from_str(&raw).map_err(err)?;
        let id = key.strip_prefix(HOLDOUT_PREFIX).ok_or("留出策略键无效")?;
        if value["id"].as_str() != Some(id) {
            return Err("留出策略身份无效，停止淘汰".into());
        }
        let mut used = protected.contains(id);
        // Archived results may retain manifest identity only through their
        // dependency graph. A surviving source archive conservatively pins its
        // source's policies until archive retention has run.
        if table_exists(db, "quota_calibrations") {
            if let Some(source) = value["sourceId"].as_str() {
                used |= db
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM quota_calibrations WHERE source_id=?1)",
                        [source],
                        |r| r.get::<_, bool>(0),
                    )
                    .map_err(err)?;
            }
        }
        if instant(&value["createdAt"]).is_none() || !value["assignments"].is_array() {
            return Err("留出策略时间或配对结构无效，停止淘汰".into());
        }
        for assignment in value["assignments"].as_array().into_iter().flatten() {
            for field in ["startReceipt", "endReceipt"] {
                if let Some(endpoint) = assignment[field].as_i64() {
                    used |= db
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM account_snapshots WHERE id=?1)",
                            [endpoint],
                            |r| r.get::<_, bool>(0),
                        )
                        .map_err(err)?;
                }
            }
        }
        policies.push((key, raw.len(), instant(&value["createdAt"]), used));
    }
    policies.sort_by(|a, b| a.2.cmp(&b.2).then(a.0.cmp(&b.0)));
    let mut count = policies.len();
    let mut bytes = policies.iter().map(|p| p.1).sum::<usize>();
    let mut deleted = 0;
    let cutoff = at - Duration::days(days(retention_days));
    for (key, size, created, used) in &policies {
        let expired = created.is_some_and(|t| t < cutoff);
        if !used
            && (expired
                || count > HOLDOUT_COUNT
                || bytes > HOLDOUT_COUNT * HOLDOUT_BYTES
                || *size > HOLDOUT_BYTES)
        {
            db.execute("DELETE FROM settings WHERE key=?1", [key])
                .map_err(err)?;
            count -= 1;
            bytes -= size;
            deleted += 1;
        }
    }
    let protected_count = policies.iter().filter(|p| p.3).count();
    Ok(
        json!({"schemaVersion":1,"retentionDays":days(retention_days),"policyCount":count,"payloadBytes":bytes,"deletedPolicies":deleted,"protectedPolicies":protected_count,"maxPolicies":HOLDOUT_COUNT,"maxPolicyBytes":HOLDOUT_BYTES,"maxPayloadBytes":HOLDOUT_COUNT*HOLDOUT_BYTES,"blockedByInUseEvidence":count>HOLDOUT_COUNT||bytes>HOLDOUT_COUNT*HOLDOUT_BYTES||policies.iter().any(|p|p.3&&p.1>HOLDOUT_BYTES),"cannotDeleteInUseManifest":true}),
    )
}

#[cfg(test)]
#[path = "observation_storage_tests.rs"]
mod tests;
