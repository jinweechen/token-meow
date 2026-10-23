//! 仅保存本进程的逐来源官方快照；目录身份不能证明不同账户。
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Default)]
pub(crate) struct Cache {
    epoch: u64,
    next_generation: u64,
    entries: BTreeMap<String, Entry>,
}
struct Entry {
    binding: Value,
    generation: u64,
    snapshot: Option<Value>,
    last_attempt_at: Option<String>,
}
#[derive(Clone)]
pub(crate) struct Ticket {
    source_id: String,
    binding: Value,
    epoch: u64,
    generation: u64,
}
pub(crate) fn binding(source: &Value) -> Value {
    json!({"id":source["id"],"root":source["root"],"clientKind":source["clientKind"],
        "environment":source["environment"],"enabled":source["enabled"],"configurationRevision":source["configurationRevision"]})
}
impl Cache {
    pub(crate) fn sync(&mut self, sources: &[Value]) {
        self.entries
            .retain(|id, _| sources.iter().any(|s| s["id"].as_str() == Some(id)));
        for source in sources {
            let Some(id) = source["id"].as_str() else {
                continue;
            };
            let current = binding(source);
            if self
                .entries
                .get(id)
                .is_some_and(|entry| entry.binding == current)
            {
                continue;
            }
            self.next_generation = self.next_generation.saturating_add(1);
            self.entries.insert(
                id.into(),
                Entry {
                    binding: current,
                    generation: self.next_generation,
                    snapshot: None,
                    last_attempt_at: None,
                },
            );
        }
    }
    pub(crate) fn clear(&mut self) {
        self.epoch = self.epoch.saturating_add(1);
        self.entries.clear();
    }
    pub(crate) fn begin(&mut self, source: &Value) -> Result<Ticket, String> {
        let id = source["id"].as_str().ok_or("来源缺少标识。")?;
        let entry = self
            .entries
            .get_mut(id)
            .ok_or("来源登记已变化，请重新读取。")?;
        if entry.binding != binding(source) {
            return Err("来源登记已变化，请重新读取。".into());
        }
        entry.last_attempt_at = Some(chrono::Utc::now().to_rfc3339());
        Ok(Ticket {
            source_id: id.into(),
            binding: entry.binding.clone(),
            epoch: self.epoch,
            generation: entry.generation,
        })
    }
    pub(crate) fn publish(&mut self, ticket: &Ticket, mut snapshot: Value) -> Result<(), String> {
        let entry = self
            .entries
            .get_mut(&ticket.source_id)
            .ok_or("来源已移除，已丢弃旧快照。")?;
        if self.epoch != ticket.epoch
            || entry.generation != ticket.generation
            || entry.binding != ticket.binding
        {
            return Err("来源绑定或数据范围已变化，已丢弃旧快照。".into());
        }
        snapshot["sourceId"] = json!(ticket.source_id);
        // 失败的读取不能把过去成功的数据标记为当前成功。
        if snapshot["status"] != "connected" && snapshot["collectedAt"].as_str().is_none() {
            if let Some(previous) = entry
                .snapshot
                .as_ref()
                .filter(|p| p["collectedAt"].as_str().is_some())
            {
                let mut stale = previous.clone();
                stale["status"] = json!("stale");
                stale["error"] = snapshot["error"].clone();
                stale["lastAttemptAt"] = snapshot["lastAttemptAt"].clone();
                snapshot = stale;
            }
        }
        entry.snapshot = Some(snapshot);
        Ok(())
    }
    pub(crate) fn observe(&mut self, source: &Value, snapshot: Value) -> Result<(), String> {
        let ticket = self.begin(source)?;
        self.publish(&ticket, snapshot)
    }
    pub(crate) fn resource_stop(&mut self, binding: &Value, error: &str) -> bool {
        let mut changed = false;
        for entry in self
            .entries
            .values_mut()
            .filter(|entry| entry.binding == *binding)
        {
            if let Some(snapshot) = entry.snapshot.as_mut() {
                let previous = snapshot.clone();
                snapshot["status"] = json!(if snapshot["collectedAt"].as_str().is_some() {
                    "stale"
                } else {
                    "unavailable"
                });
                snapshot["error"] = json!(error);
                changed |= *snapshot != previous;
            }
        }
        changed
    }
    pub(crate) fn view(&self, sources: &[Value], primary_id: &str) -> Value {
        let now = chrono::Utc::now();
        let rows: Vec<Value> = sources.iter().map(|source| {
            let id = source["id"].as_str().unwrap_or_default();
            let enabled = source["enabled"].as_bool().unwrap_or(false);
            let supported = source["clientKind"] == "codex";
            let entry = self.entries.get(id).filter(|e| e.binding == binding(source));
            let mut snapshot = entry.and_then(|e| e.snapshot.clone());
            if !enabled || !supported { snapshot = None; }
            if let Some(value) = snapshot.as_mut() {
                let expired = value["collectedAt"].as_str().and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .is_some_and(|at| now.signed_duration_since(at).num_seconds() > 120);
                if expired && value["status"] == "connected" { value["status"] = json!("stale"); }
            }
            let status = if !enabled { "disabled" } else if !supported { "unsupported" }
                else { match snapshot.as_ref().and_then(|s| s["status"].as_str()) {
                    Some("connected") => "ready", Some("stale") => "stale",
                    Some(_) => "unavailable", None => "not_read",
                }};
            let mut reasons = vec!["来源目录不等于不同账户；当前协议尚未证明跨 RPC 身份绑定，额度逐项显示，不合并。".to_owned()];
            if !supported { reasons.push("此工具目前只读取本地计量日志，尚未接入官方账户额度。".into()); }
            if let Some(error) = snapshot.as_ref().and_then(|s| s["error"].as_str()) { reasons.push(error.to_owned()); }
            json!({"sourceId":id,"clientKind":source["clientKind"],"primary":id==primary_id,"enabled":enabled,
                "bindingGeneration":entry.map(|e|e.generation.to_string()).unwrap_or_else(||"0".into()),
                "status":status,"identityStatus":if snapshot.is_some(){"unproven"}else{"unknown"},
                "snapshot":snapshot,"lastAttemptAt":entry.and_then(|e|e.last_attempt_at.clone()),"reasons":reasons})
        }).collect();
        json!({"asOf":now.to_rfc3339(),"sourceCount":sources.len(),"confirmedAccountCount":null,"rows":rows})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source(id: &str, root: &str) -> Value {
        json!({"id":id,"root":root,"clientKind":"codex","enabled":true,"environment":"local"})
    }
    fn snapshot() -> Value {
        json!({"status":"connected","collectedAt":chrono::Utc::now().to_rfc3339(),"buckets":[{"id":"same","primary":{"usedPercent":20}}]})
    }
    #[test]
    fn sources_remain_separate_without_claiming_distinct_accounts_or_adding_percentages() {
        let sources = vec![source("a", "one"), source("b", "two")];
        let mut cache = Cache::default();
        cache.sync(&sources);
        for source in &sources {
            cache.observe(source, snapshot()).unwrap();
        }
        let view = cache.view(&sources, "a");
        // --nocapture 的合成 DTO 供跨语言契约检查使用，不含真实来源路径或账户信息。
        println!("SOURCE_ACCOUNT_CONTRACT_FIXTURE={view}");
        assert!(view["confirmedAccountCount"].is_null());
        assert_eq!(view["rows"].as_array().unwrap().len(), 2);
        assert_eq!(view["rows"][0]["status"], "ready");
        assert_eq!(
            view["rows"][1]["snapshot"]["buckets"][0]["primary"]["usedPercent"],
            20
        );
    }
    #[test]
    fn switching_away_and_back_rejects_previous_visit_reply() {
        let a = source("a", "one");
        let b = source("a", "two");
        let mut cache = Cache::default();
        cache.sync(&[a.clone()]);
        let ticket = cache.begin(&a).unwrap();
        cache.sync(&[b]);
        cache.sync(&[a.clone()]);
        assert!(cache.publish(&ticket, snapshot()).is_err());
        assert_eq!(cache.view(&[a], "a")["rows"][0]["status"], "not_read");
    }
    #[test]
    fn clear_rejects_inflight_reply_and_erases_all_source_data() {
        let a = source("a", "one");
        let mut cache = Cache::default();
        cache.sync(&[a.clone()]);
        let ticket = cache.begin(&a).unwrap();
        cache.clear();
        cache.sync(&[a.clone()]);
        assert!(cache.publish(&ticket, snapshot()).is_err());
        assert!(cache.view(&[a], "a")["rows"][0]["snapshot"].is_null());
    }
    #[test]
    fn failed_refresh_preserves_old_success_as_stale_only() {
        let a = source("a", "one");
        let mut cache = Cache::default();
        cache.sync(&[a.clone()]);
        cache.observe(&a, snapshot()).unwrap();
        cache
            .observe(
                &a,
                json!({"status":"unavailable","collectedAt":null,"error":"读取失败"}),
            )
            .unwrap();
        let view = cache.view(&[a], "a");
        assert_eq!(view["rows"][0]["status"], "stale");
        assert_eq!(view["rows"][0]["snapshot"]["buckets"][0]["id"], "same");
    }
    #[test]
    fn disabled_or_unsupported_sources_never_expose_old_official_snapshot() {
        let a = source("a", "one");
        let mut cache = Cache::default();
        cache.sync(&[a.clone()]);
        cache.observe(&a, snapshot()).unwrap();
        let mut disabled = a.clone();
        disabled["enabled"] = json!(false);
        cache.sync(&[disabled.clone()]);
        assert!(cache.view(&[disabled], "a")["rows"][0]["snapshot"].is_null());
        let mut gemini = a;
        gemini["clientKind"] = json!("gemini");
        cache.sync(&[gemini.clone()]);
        assert_eq!(
            cache.view(&[gemini], "a")["rows"][0]["status"],
            "unsupported"
        );
    }
    #[test]
    fn health_changes_do_not_invalidate_configuration_but_environment_changes_do() {
        let mut a = source("a", "one");
        let mut cache = Cache::default();
        cache.sync(&[a.clone()]);
        cache.observe(&a, snapshot()).unwrap();
        a["health"] = json!({"files":99});
        cache.sync(&[a.clone()]);
        assert_eq!(cache.view(&[a.clone()], "a")["rows"][0]["status"], "ready");
        a["environment"] = json!("wsl");
        cache.sync(&[a.clone()]);
        assert_eq!(cache.view(&[a], "a")["rows"][0]["status"], "not_read");
    }
    #[test]
    fn secondary_resource_stop_cannot_poison_primary_and_stale_binding_is_ignored() {
        let a = source("a", "one");
        let b = source("b", "two");
        let mut cache = Cache::default();
        cache.sync(&[a.clone(), b.clone()]);
        cache.observe(&a, snapshot()).unwrap();
        cache.observe(&b, snapshot()).unwrap();
        assert!(cache.resource_stop(&binding(&b), "资源预算停止"));
        assert!(!cache.resource_stop(&binding(&b), "资源预算停止"));
        let view = cache.view(&[a.clone(), b.clone()], "a");
        assert_eq!(view["rows"][0]["status"], "ready");
        assert_eq!(view["rows"][1]["status"], "stale");
        let moved = source("b", "elsewhere");
        cache.sync(&[a.clone(), moved.clone()]);
        assert!(!cache.resource_stop(&binding(&b), "旧资源事件"));
        assert_eq!(
            cache.view(&[a, moved], "a")["rows"][1]["status"],
            "not_read"
        );
    }
    #[test]
    fn clear_allocates_new_monotone_decimal_generation_and_old_snapshots_expire() {
        let a = source("a", "one");
        let mut cache = Cache::default();
        cache.sync(&[a.clone()]);
        let old_generation = cache.view(&[a.clone()], "a")["rows"][0]["bindingGeneration"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap();
        cache.clear();
        cache.sync(&[a.clone()]);
        let generation = cache.view(&[a.clone()], "a")["rows"][0]["bindingGeneration"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap();
        assert!(generation > old_generation);
        let mut expired = snapshot();
        expired["collectedAt"] =
            json!((chrono::Utc::now() - chrono::Duration::seconds(121)).to_rfc3339());
        cache.observe(&a, expired).unwrap();
        assert_eq!(cache.view(&[a], "a")["rows"][0]["status"], "stale");
    }
}
