//! Immutable derived-result versions. A surviving archive is not surviving
//! evidence: dependencies are checked again before any current guidance.
use super::super::{err, hash, Result};
use super::model::{CalibrationPair, NativeParams, NativeSample};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const MAX_RECORD_BYTES: usize = 512 * 1024;
const MAX_PAIRS: usize = 256;
const MAX_PROFILES: usize = 1000;
const MAX_SOURCE_BYTES: i64 = 32 * 1024 * 1024;
const MAX_TOTAL_BYTES: i64 = 128 * 1024 * 1024;
const MAX_HISTORY: usize = 100_000;
const MAX_DEPENDENCIES: usize = 32_768;
const RECEIPT_DIGEST_VERSION: &str = "receipt-window-v1";

fn timestamp(v: DateTime<Utc>) -> String {
    v.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
fn seconds(v: f64) -> Result<DateTime<Utc>> {
    // The initial allocator only admits exact millisecond-aligned boundaries.
    let ms = v * 1000.0;
    if !ms.is_finite() || ms < 0.0 || ms >= i64::MAX as f64 || (ms - ms.round()).abs() > 0.001 {
        return Err("校准时间边界不能精确映射到账本".into());
    }
    DateTime::<Utc>::from_timestamp_millis(ms.round() as i64).ok_or("校准时间边界无效".into())
}
fn key(s: &NativeSample) -> String {
    json!([
        s.scope.runtime_id,
        s.scope.source_id,
        s.scope.binding_generation.to_string(),
        s.scope.identity_epoch.to_string(),
        s.scope.clear_generation,
        s.sequence.to_string()
    ])
    .to_string()
}
fn receipt_key(v: &Value) -> Option<String> {
    if v["documentKind"] != "subscription_observation"
        || v["schemaVersion"] != 1
        || v["outcome"] != "success"
    {
        return None;
    }
    let clear = v["clearGeneration"].to_string();
    Some(
        json!([
            v["runtimeId"].as_str()?,
            v["sourceId"].as_str()?,
            v["bindingGeneration"].as_str()?,
            v["identityEpoch"].as_str()?,
            clear,
            v["observationSequence"].as_str()?
        ])
        .to_string(),
    )
}
fn canonical_digest(v: &Value) -> String {
    let mut v = v.clone();
    v.sort_all_objects();
    hash(v.to_string().as_bytes())
}

/// One index and lazy digest memo per public operation. Shared profiles often
/// depend on the same receipts and intervals; hashing them once prevents a
/// profile count multiplier over the complete retained history.
struct DependencyCache<'a> {
    db: &'a Connection,
    receipts: BTreeMap<String, &'a Value>,
    receipt_digests: BTreeMap<(String, Option<String>), Option<String>>,
    deferred_receipts: BTreeSet<String>,
    ledger_digests: BTreeMap<(String, DateTime<Utc>, DateTime<Utc>), String>,
    #[cfg(test)]
    receipt_digest_computations: usize,
    #[cfg(test)]
    ledger_digest_computations: usize,
}
impl<'a> DependencyCache<'a> {
    fn new(db: &'a Connection, history: &'a [Value], source: &str) -> Result<Self> {
        if history.len() > MAX_HISTORY {
            return Err("校准历史观测超过单次索引预算".into());
        }
        let receipts = history
            .iter()
            .filter(|r| r["sourceId"].as_str() == Some(source))
            .filter_map(|r| Some((receipt_key(r)?, r)))
            .collect();
        Ok(Self {
            db,
            receipts,
            receipt_digests: BTreeMap::new(),
            deferred_receipts: BTreeSet::new(),
            ledger_digests: BTreeMap::new(),
            #[cfg(test)]
            receipt_digest_computations: 0,
            #[cfg(test)]
            ledger_digest_computations: 0,
        })
    }
    fn receipt_digest(&mut self, key: &str, window: Option<&str>) -> Result<Option<String>> {
        let cache_key = (key.to_owned(), window.map(str::to_owned));
        if let Some(digest) = self.receipt_digests.get(&cache_key) {
            return Ok(digest.clone());
        }
        let Some(receipt) = self.receipts.get(key) else {
            return Ok(None);
        };
        if self.receipt_digests.len() >= MAX_DEPENDENCIES {
            return Err("校准观测依赖超过单次摘要预算".into());
        }
        // Archive callers retain raw encoded headers. Resolve only the exact
        // referenced receipt, hash it and release its body; never expand all
        // retained history or cache complete reconstructed observations.
        // A bounded projector view is not evidence of deletion if supplied by
        // mistake: hide numerical guidance, but never persist invalidation.
        if receipt["storageEvidence"]["reason"] == "projection_budget_exceeded"
            && receipt["storageEvidence"]["evidenceRemoved"] == false
        {
            self.deferred_receipts.insert(key.to_owned());
            self.receipt_digests.insert(cache_key, None);
            return Ok(None);
        }
        let receipt = super::observation_storage::resolve(self.db, receipt)?;
        if receipt["storageEvidence"]["available"] == false
            || !receipt["observation"].is_object()
            || receipt_key(&receipt).as_deref() != Some(key)
        {
            self.receipt_digests.insert(cache_key, None);
            return Ok(None);
        }
        let digest = if let Some(window) = window {
            let mut scoped = receipt;
            let namespace = scoped
                .as_object_mut()
                .ok_or("校准观测信封无效")?
                .remove("derivedCalibrationPatches");
            if let Some(namespace) = namespace {
                if let Some(patches) = namespace.as_object() {
                    if let Some(patch) = patches.get(window) {
                        scoped["derivedCalibrationPatches"] = json!({(window):patch});
                    }
                } else {
                    // Malformed metadata is a changed dependency, never a
                    // reason to silently discard the old proof requirement.
                    scoped["derivedCalibrationPatches"] = namespace;
                }
            }
            canonical_digest(&scoped)
        } else {
            // Existing pre-window dependency records retain their original
            // whole-envelope contract; do not silently reinterpret a hash.
            canonical_digest(&receipt)
        };
        #[cfg(test)]
        {
            self.receipt_digest_computations += 1;
        }
        self.receipt_digests.insert(cache_key, Some(digest.clone()));
        Ok(Some(digest))
    }
    fn ledger_digest(
        &mut self,
        source: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<String> {
        let key = (source.to_owned(), start, end);
        if let Some(digest) = self.ledger_digests.get(&key) {
            return Ok(digest.clone());
        }
        if self.ledger_digests.len() >= MAX_DEPENDENCIES {
            return Err("校准账本区间超过单次摘要预算".into());
        }
        let digest = super::demand_history::ledger_fingerprint(self.db, source, start, end)?;
        #[cfg(test)]
        {
            self.ledger_digest_computations += 1;
        }
        self.ledger_digests.insert(key, digest.clone());
        Ok(digest)
    }
}
fn latest(db: &Connection, source: Option<&str>) -> Result<Vec<Value>> {
    let mut stmt=db.prepare("SELECT p.data FROM quota_calibrations p JOIN (SELECT calibration_id,MAX(revision) revision FROM quota_calibrations GROUP BY calibration_id) c ON p.calibration_id=c.calibration_id AND p.revision=c.revision WHERE (?1 IS NULL OR p.source_id=?1) ORDER BY p.updated_at DESC LIMIT 1001").map_err(err)?;
    let rows = stmt
        .query_map([source], |r| r.get::<_, String>(0))
        .map_err(err)?;
    let values = rows
        .map(|v| serde_json::from_str(&v.map_err(err)?).map_err(err))
        .collect::<Result<Vec<Value>>>()?;
    if values.len() > MAX_PROFILES {
        return Err("校准档案数量超过领域预算".into());
    }
    Ok(values)
}
fn insert(db: &Connection, mut value: Value, at: DateTime<Utc>) -> Result<()> {
    let id = value["archiveId"]
        .as_str()
        .ok_or("校准档案ID缺失")?
        .to_string();
    let revision: i64 = db
        .query_row(
            "SELECT COALESCE(MAX(revision),0) FROM quota_calibrations WHERE calibration_id=?1",
            [&id],
            |r| r.get(0),
        )
        .map_err(err)?;
    let next = revision
        .checked_add(1)
        .filter(|n| *n <= 9_007_199_254_740_991)
        .ok_or("校准档案修订耗尽")?;
    value["revision"] = json!(next);
    value["updatedAt"] = json!(timestamp(at));
    if value.to_string().len() > MAX_RECORD_BYTES {
        return Err("校准档案单条超过512KiB预算".into());
    }
    db.execute("INSERT INTO quota_calibrations(calibration_id,revision,source_id,updated_at,data) VALUES(?1,?2,?3,?4,?5)",params![id,next,value["sourceId"].as_str().ok_or("校准来源缺失")?,timestamp(at),value.to_string()]).map_err(err)?;
    Ok(())
}
fn invalidate(value: &Value, reason: &str) -> Value {
    let mut value = value.clone();
    value["result"]["quality"] = json!("invalidated");
    for field in [
        "empiricalCapacity",
        "capacityLower",
        "capacityUpper",
        "remainingLower",
        "remainingUpper",
    ] {
        value["result"][field] = Value::Null;
    }
    let mut reasons = value["result"]["reasons"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if !reasons.contains(&json!(reason)) {
        reasons.push(json!(reason));
    }
    value["result"]["reasons"] = json!(reasons);
    value["invalidationReason"] = json!(reason);
    value
}
fn dependency_reason(
    value: &Value,
    cache: &mut DependencyCache<'_>,
) -> Result<Option<&'static str>> {
    let Some(observations) = value["dependencies"]["observations"].as_array() else {
        return Ok(Some("evidence_removed"));
    };
    if observations.len() > MAX_PAIRS * 2 {
        return Err("校准观测依赖超过档案预算".into());
    }
    for dependency in observations {
        let Some(key) = dependency["key"].as_str() else {
            return Ok(Some("evidence_removed"));
        };
        let window = if dependency["digestVersion"].is_null() {
            None
        } else if dependency["digestVersion"] == RECEIPT_DIGEST_VERSION {
            dependency["windowId"].as_str()
        } else {
            return Ok(Some("evidence_removed"));
        };
        if !dependency["digestVersion"].is_null() && window.is_none() {
            return Ok(Some("evidence_removed"));
        }
        let Some(current) = cache.receipt_digest(key, window)? else {
            return Ok(Some(if cache.deferred_receipts.contains(key) {
                "projection_budget_exceeded"
            } else {
                "evidence_removed"
            }));
        };
        if dependency["digest"].as_str() != Some(current.as_str()) {
            return Ok(Some("historical_evidence_changed"));
        }
    }
    let Some(intervals) = value["dependencies"]["ledgerIntervals"].as_array() else {
        return Ok(Some("evidence_removed"));
    };
    if intervals.len() > MAX_PAIRS {
        return Err("校准账本区间超过档案预算".into());
    }
    for interval in intervals {
        let Some(source) = interval["sourceId"].as_str() else {
            return Ok(Some("evidence_removed"));
        };
        let Some(start) = interval["startsAt"]
            .as_str()
            .and_then(|v| DateTime::parse_from_rfc3339(v).ok())
            .map(|v| v.with_timezone(&Utc))
        else {
            return Ok(Some("evidence_removed"));
        };
        let Some(end) = interval["endsAt"]
            .as_str()
            .and_then(|v| DateTime::parse_from_rfc3339(v).ok())
            .map(|v| v.with_timezone(&Utc))
        else {
            return Ok(Some("evidence_removed"));
        };
        if end <= start {
            return Ok(Some("evidence_removed"));
        }
        let digest = cache.ledger_digest(source, start, end)?;
        if interval["digest"] != digest {
            return Ok(Some("historical_evidence_changed"));
        }
    }
    Ok(None)
}
/// Only a writer transaction may append a result or an invalidation version.
pub(crate) fn reconcile(
    db: &Connection,
    source: &str,
    history: &[Value],
    at: DateTime<Utc>,
) -> Result<()> {
    if db.is_autocommit() {
        return Err("校准档案必须与领域事务一起提交".into());
    }
    let mut cache = DependencyCache::new(db, history, source)?;
    reconcile_cached(db, source, &mut cache, at)
}
fn reconcile_cached(
    db: &Connection,
    source: &str,
    cache: &mut DependencyCache<'_>,
    at: DateTime<Utc>,
) -> Result<()> {
    for value in latest(db, Some(source))? {
        if value["result"]["quality"] == "invalidated" {
            continue;
        }
        if let Some(reason) = dependency_reason(&value, cache)? {
            if reason != "projection_budget_exceeded" {
                insert(db, invalidate(&value, reason), at)?;
            }
        }
    }
    Ok(())
}
/// The computation is derived from committed (or this transaction's newly
/// inserted) receipts. No raw vendor response or credential is duplicated.
pub(crate) fn advance(
    db: &Connection,
    source: &str,
    derived: &Value,
    history: &[Value],
    ledger_revision: i64,
    at: DateTime<Utc>,
) -> Result<()> {
    if db.is_autocommit() {
        return Err("校准档案必须与观测事务一起提交".into());
    }
    let mut cache = DependencyCache::new(db, history, source)?;
    reconcile_cached(db, source, &mut cache, at)?;
    let existing = latest(db, Some(source))?;
    let mut known: BTreeMap<_, _> = existing
        .iter()
        .filter_map(|v| Some((v["archiveId"].as_str()?.to_string(), v.clone())))
        .collect();
    'results: for result in derived["calibrations"]
        .as_array()
        .into_iter()
        .flatten()
        .take(200)
    {
        let pairs: Vec<CalibrationPair> =
            serde_json::from_value(result["inputPairs"].clone()).map_err(err)?;
        if pairs.is_empty() {
            continue;
        }
        if pairs.len() > MAX_PAIRS {
            return Err("单档案配对数量超过256预算".into());
        }
        let first = &pairs[0].start.scope;
        if first.source_id != source {
            return Err("校准来源不属于当前归档域".into());
        }
        if pairs.iter().any(|p| {
            !p.start.scope.same_calibration_domain(first)
                || p.start.scope.clear_generation != first.clear_generation
                || p.start.scope != p.end.scope
        }) {
            return Err("校准档案不能混合账号或清理域".into());
        }
        let mut observations = BTreeMap::new();
        let mut intervals = BTreeMap::new();
        for pair in &pairs {
            for sample in [&pair.start, &pair.end] {
                let key = key(sample);
                let digest = match cache.receipt_digest(&key, Some(&sample.scope.pool_id))? {
                    Some(digest) => digest,
                    None if cache.deferred_receipts.contains(&key) => continue 'results,
                    None => return Err("校准档案缺少原始观测依赖".into()),
                };
                observations.insert(
                    key.clone(),
                    json!({"key":key,"windowId":sample.scope.pool_id,"digestVersion":RECEIPT_DIGEST_VERSION,"digest":digest}),
                );
            }
            let a = pair.start.fact_time.ok_or("配对时间未知")?;
            let b = pair.end.fact_time.ok_or("配对时间未知")?;
            if a.lower != a.upper || b.lower != b.upper {
                return Err("当前档案分配器不支持不确定扣额边界".into());
            }
            let start = seconds(a.lower)?;
            let end = seconds(b.lower)?;
            let digest = cache.ledger_digest(&first.source_id, start, end)?;
            intervals.insert((timestamp(start),timestamp(end)),json!({"sourceId":first.source_id,"startsAt":timestamp(start),"endsAt":timestamp(end),"digest":digest}));
        }
        let scope = json!({"sourceId":first.source_id,"accountScope":first.account_scope,"entitlementSegment":first.entitlement_segment,"ruleVersion":first.rule_version,"poolId":first.pool_id,"clearGeneration":first.clear_generation,"profile":result["profileEvidence"]});
        let id = hash(scope.to_string().as_bytes());
        let mut frozen_result = result.clone();
        frozen_result["remainingLower"] = Value::Null;
        frozen_result["remainingUpper"] = Value::Null;
        let value = json!({"schemaVersion":1,"archiveId":id,"sourceId":first.source_id,"scope":scope,"result":frozen_result,"asOfLedgerRevision":ledger_revision.to_string(),"parameters":{"algorithmVersion":derived["algorithmVersion"],"native":NativeParams::default(),"calibration":{"minConditionalPairs":3,"minSpanSeconds":900,"minSignalPercent":15,"minVerifiedTrainingPairs":20,"minVerifiedHoldoutPairs":5,"minVerifiedPeriods":3,"maxRelativeMad":0.2,"maxHoldoutWape":0.2}},"dependencies":{"observations":observations.into_values().collect::<Vec<_>>(),"ledgerIntervals":intervals.into_values().collect::<Vec<_>>()}});
        // More recent remaining percentages do not erase a frozen capacity
        // version; duplicate calculations create no new archive revision.
        let identity =
            |v: &Value| json!([v["scope"], v["result"], v["dependencies"], v["parameters"]]);
        if known
            .get(&id)
            .is_none_or(|old| identity(old) != identity(&value))
        {
            if !known.contains_key(&id) && known.len() >= MAX_PROFILES {
                return Err("校准档案数量已达1000预算".into());
            }
            insert(db, value.clone(), at)?;
            known.insert(id, value);
        }
    }
    Ok(())
}
/// This remains read only. If a writer has not yet persisted an invalidation,
/// the query still refuses numerical guidance immediately.
pub(crate) fn cards(
    db: &Connection,
    source: &str,
    history: &[Value],
    current: Option<&Value>,
) -> Result<Vec<Value>> {
    let mut cache = DependencyCache::new(db, history, source)?;
    latest(db, Some(source))?
        .into_iter()
        .map(|value| {
            let value = if value["result"]["quality"] != "invalidated" {
                match dependency_reason(&value, &mut cache)? {
                    Some(reason) => invalidate(&value, reason),
                    None => value,
                }
            } else {
                value
            };
            let same_current = current.is_some_and(|c| {
                c["identityConsistency"] == "proven"
                    && c["accountScope"] == value["scope"]["accountScope"]
                    && c["entitlementSegment"] == value["scope"]["entitlementSegment"]
                    && c["clearGeneration"].to_string()
                        == value["scope"]["clearGeneration"].as_str().unwrap_or("")
            });
            let value = if same_current {
                value
            } else {
                invalidate(&value, "scope_changed")
            };
            let mut card = value["result"].clone();
            card["id"] = json!(format!(
                "archive:{}",
                value["archiveId"].as_str().unwrap_or("unknown")
            ));
            card["archiveId"] = value["archiveId"].clone();
            card["archiveRevision"] = value["revision"].clone();
            card["archived"] = json!(true);
            card["label"] = json!("历史校准档案");
            card["recordedAt"] = value["updatedAt"].clone();
            // Only a fresh live projection may compute current remaining tokens.
            card["remainingLower"] = Value::Null;
            card["remainingUpper"] = Value::Null;
            if let Some(o) = card.as_object_mut() {
                o.remove("inputPairs");
                o.remove("evidence");
            }
            Ok(card)
        })
        .collect()
}
/// Archives are bounded derived history. Eviction may remove old versions,
/// never authoritative vendor logs; invalidation precedes dependency eviction.
pub(crate) fn retain(db: &Connection, at: DateTime<Utc>, days: i64) -> Result<()> {
    let cutoff = timestamp(at - Duration::days(days.clamp(1, 90)));
    db.execute(
        "DELETE FROM quota_calibrations WHERE updated_at<?1",
        [cutoff],
    )
    .map_err(err)?;
    db.execute("DELETE FROM quota_calibrations WHERE rowid IN (SELECT rowid FROM (SELECT rowid,SUM(length(CAST(data AS BLOB))) OVER(PARTITION BY source_id ORDER BY rowid DESC) bytes FROM quota_calibrations) WHERE bytes>?1)",[MAX_SOURCE_BYTES]).map_err(err)?;
    db.execute("DELETE FROM quota_calibrations WHERE rowid IN (SELECT rowid FROM (SELECT rowid,SUM(length(CAST(data AS BLOB))) OVER(ORDER BY rowid DESC) bytes FROM quota_calibrations) WHERE bytes>?1)",[MAX_TOTAL_BYTES]).map_err(err)?;
    Ok(())
}

#[cfg(test)]
#[path = "calibration_archive_tests.rs"]
mod tests;
