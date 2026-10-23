//! The writer's typed evidence boundary. This is deliberately not an IPC DTO:
//! frontend JSON cannot supply provider, coverage or billing attestations.
use super::super::{adapter::ResponseUsage, err, Result};
use super::calibration_assembler::{
    self as assembler, AdapterQuotaProof, CoverageAttestation, EvidenceBatch, LedgerSettlement,
    UsageModeAttestation,
};
use crate::budget_clock::Reading;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) use assembler::observation_proof::{AdapterResetTransitionProof, ResetTransition};

pub(crate) struct CollectorBatch<'a> {
    pub proofs: &'a [AdapterQuotaProof],
    pub settlement: Option<&'a LedgerSettlement>,
    pub coverage: &'a [CoverageAttestation],
    pub modes: &'a [UsageModeAttestation],
    pub reset_transitions: &'a [AdapterResetTransitionProof],
}

/// Called by Core only after assigning its current runtime/source/clear domain,
/// before deciding admission. Payload JSON cannot grant its own protected facts.
pub(crate) fn admit_observation(
    snapshot: &mut Value,
    previous: Option<&Value>,
    batches: &[CollectorBatch<'_>],
    at: &Reading,
) -> Value {
    if batches.len() > 200
        || batches
            .iter()
            .try_fold(0usize, |n, batch| n.checked_add(batch.proofs.len()))
            .is_none_or(|n| n > 1024)
        || batches
            .iter()
            .try_fold(0usize, |n, batch| {
                n.checked_add(batch.reset_transitions.len())
            })
            .is_none_or(|n| n > 200)
    {
        assembler::observation_proof::admit(snapshot, previous, &[], &[], at);
        return json!({"status":"unavailable","reasonCodes":["observation_proof_limit"]});
    }
    let proofs = batches
        .iter()
        .flat_map(|batch| batch.proofs.iter())
        .collect::<Vec<_>>();
    let resets = batches
        .iter()
        .flat_map(|batch| batch.reset_transitions.iter())
        .collect::<Vec<_>>();
    assembler::observation_proof::admit(snapshot, previous, &proofs, &resets, at)
}

pub(crate) fn seal_receipt(receipt: &mut Value) -> Result<()> {
    assembler::observation_proof::seal_receipt(receipt)
}
fn receipt_key(v: &Value) -> String {
    json!([
        v["runtimeId"],
        v["sourceId"],
        v["bindingGeneration"],
        v["identityEpoch"],
        v["clearGeneration"].to_string(),
        v["observationSequence"]
    ])
    .to_string()
}
fn proof_key(p: &AdapterQuotaProof) -> String {
    let s = &p.scope;
    json!([
        s.runtime_id,
        s.source_id,
        s.binding_generation.to_string(),
        s.identity_epoch.to_string(),
        s.clear_generation,
        p.sequence.to_string()
    ])
    .to_string()
}

pub(crate) fn advance(
    db: &Connection,
    source: &str,
    batches: &[CollectorBatch<'_>],
    usage: &[ResponseUsage],
    at: &Reading,
) -> Result<Value> {
    if db.is_autocommit() {
        return Err("学习证据只能在观测写入事务中组装".into());
    }
    if batches.is_empty() {
        return Ok(
            json!({"status":"unavailable","reasonCodes":["adapter_quota_proof_missing"],"assembledPairs":0}),
        );
    }
    if batches.len() > 200 || batches.iter().map(|b| b.proofs.len()).sum::<usize>() > 1024 {
        return Err("学习证据超出单轮有界输入".into());
    }
    let mut s=db.prepare("SELECT id,data FROM account_snapshots WHERE json_extract(data,'$.documentKind')='subscription_observation' AND json_extract(data,'$.sourceId')=?1 ORDER BY id DESC LIMIT 1024").map_err(err)?;
    let rows = s
        .query_map([source], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(err)?;
    let mut receipts = BTreeMap::new();
    for row in rows {
        let (id, raw) = row.map_err(err)?;
        let stored: Value = serde_json::from_str(&raw).map_err(err)?;
        let value = super::observation_storage::resolve(db, &stored)?;
        if receipts.insert(receipt_key(&value), id).is_some() {
            return Err("学习输入存在重复观测作用域与序号".into());
        }
    }
    drop(s);
    let mut reasons = BTreeSet::new();
    let mut pairs = 0usize;
    let mut manifests = BTreeSet::new();
    for batch in batches {
        let mut observations = Vec::new();
        for proof in batch.proofs {
            if proof.scope.source_id != source {
                return Err("受信学习证据不属于当前来源".into());
            }
            let Some(id) = receipts.get(&proof_key(proof)) else {
                reasons.insert("committed_receipt_missing".to_string());
                continue;
            };
            match assembler::load_observation(db, *id, Some(proof)) {
                Ok(observation) => observations.push((*id, observation)),
                Err(reason) => {
                    reasons.insert(reason.detail);
                }
            }
        }
        // Committed database order is the writer's acquisition order. Runtime
        // UUID lexical order and a restarted sequence are not chronology.
        observations.sort_by_key(|(id, _)| *id);
        let observations = observations
            .into_iter()
            .map(|(_, value)| value)
            .collect::<Vec<_>>();
        let assembly = assembler::assemble(
            db,
            &EvidenceBatch {
                observations: &observations,
                usage,
                settlement: batch.settlement,
                coverage: batch.coverage,
                modes: batch.modes,
            },
            at,
        )?;
        assembler::apply(db, &assembly)?;
        pairs = pairs
            .checked_add(assembly.pairs.len())
            .ok_or("学习配对数量溢出")?;
        for missing in assembly.missing {
            reasons.insert(missing.detail);
        }
        for manifest in assembly.manifests {
            manifests.insert(manifest.id);
        }
    }
    Ok(
        json!({"status":if pairs>0{"assembled"}else{"insufficient"},"reasonCodes":reasons.into_iter().take(64).collect::<Vec<_>>(),"assembledPairs":pairs,"manifestCount":manifests.len()}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_adapter_proof_is_explicit_and_cannot_write_a_manifest() {
        let mut db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE settings(key TEXT PRIMARY KEY,value TEXT);")
            .unwrap();
        let at = crate::budget_clock::now();
        let tx = db.transaction().unwrap();
        let before = tx.total_changes();
        let result = advance(&tx, "source", &[], &[], &at).unwrap();
        assert_eq!(result["status"], "unavailable");
        assert_eq!(
            result["reasonCodes"],
            json!(["adapter_quota_proof_missing"])
        );
        assert_eq!(before, tx.total_changes());
        tx.rollback().unwrap();
        assert!(advance(&db, "source", &[], &[], &at).is_err());
    }
}
