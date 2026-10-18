//! Versioned, bounded relationship evidence. This never changes metering identity.
use super::*;

const EVIDENCE_VERSION: u8 = 1;
const VERIFIED_THREAD_SOURCE_VERSIONS: &[&str] = &["0.147.0"];

#[derive(Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Evidence {
    schema_version: u8,
    parents: Vec<String>,
    parent_conflict: bool,
    thread_sources: Vec<String>,
    thread_source_conflict: bool,
    verified_user: bool,
    verified_subagent: bool,
    explicit_subagent: bool,
    non_user_source: bool,
    invalid: bool,
    observations: Vec<Value>,
}

fn label(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= 160 && !s.chars().any(char::is_control))
        .map(str::to_owned)
}

fn retain_distinct(values: &mut Vec<String>, value: String, conflict: &mut bool) {
    if !values.contains(&value) {
        values.push(value);
        values.sort();
        if values.len() > 1 {
            *conflict = true;
        }
        values.truncate(2);
    }
}

fn evidence(data: &Value) -> Evidence {
    if let Some(raw) = data.get("agentKindEvidence") {
        match serde_json::from_value::<Evidence>(raw.clone()) {
            Ok(e) if e.schema_version == EVIDENCE_VERSION => e,
            _ => Evidence {
                schema_version: EVIDENCE_VERSION,
                invalid: true,
                ..Evidence::default()
            },
        }
    } else {
        // Old v3 JSON may preserve a positive parent assertion without provenance.
        // Its absence is never evidence of a primary thread; reads do not write it back.
        let mut e = Evidence {
            schema_version: EVIDENCE_VERSION,
            ..Evidence::default()
        };
        if let Some(parent) = label(data.get("parentThreadId")) {
            e.parents.push(parent);
            e.observations.push(json!({"channel":"legacy-metadata","parentThreadId":e.parents[0],"producerVersion":null}));
        }
        e
    }
}

// Preserve just the source enum and relationship fields, never nicknames, paths,
// agent instructions, or arbitrary metadata nested in a future source variant.
fn subagent_source(
    source: Option<&Value>,
    variant_key: &str,
) -> (bool, Option<String>, Option<Value>) {
    let Some(source) = source else {
        return (false, None, None);
    };
    if let Some(s) = label(Some(source)) {
        let kind = if [
            "cli",
            "vscode",
            "exec",
            "appServer",
            "appserver",
            "mcp",
            "unknown",
        ]
        .contains(&s.as_str())
        {
            s.as_str()
        } else {
            "unsupported"
        };
        return (false, None, Some(json!(kind)));
    }
    if source.as_object().is_some_and(|o| o.len() != 1) {
        return (false, None, Some(json!("unsupported")));
    }
    if source.get("internal").and_then(Value::as_str) == Some("memory_consolidation") {
        return (
            false,
            None,
            Some(json!({"internal":"memory_consolidation"})),
        );
    }
    let Some(child) = source.get(variant_key) else {
        return (
            false,
            None,
            if source.is_null() {
                None
            } else {
                Some(json!("unsupported"))
            },
        );
    };
    if let Some(kind) = child
        .as_str()
        .filter(|s| ["review", "compact", "memory_consolidation"].contains(s))
    {
        return (true, None, Some(json!({"subAgent":kind})));
    }
    if !child.as_object().is_some_and(|o| o.len() == 1) {
        return (false, None, Some(json!({"subAgent":"unsupported"})));
    }
    if let Some(spawn) = child.get("thread_spawn") {
        if let (Some(parent), Some(depth)) = (
            label(spawn.get("parent_thread_id")),
            spawn.get("depth").and_then(Value::as_i64),
        ) {
            if (0..=i32::MAX as i64).contains(&depth) {
                return (
                    true,
                    Some(parent.clone()),
                    Some(
                        json!({"subAgent":{"thread_spawn":{"parent_thread_id":parent,"depth":depth}}}),
                    ),
                );
            }
        }
    }
    if child.get("other").and_then(Value::as_str).is_some() {
        return (true, None, Some(json!({"subAgent":{"other":"observed"}})));
    }
    (false, None, Some(json!({"subAgent":"unsupported"})))
}

pub(super) fn merge(data: &mut Value, incoming: &Value, channel: &str, version: Option<&str>) {
    let mut e = evidence(data);
    let (parent_key, thread_source_key, source_variant) = match channel {
        "jsonl-session-meta" => ("parent_thread_id", "thread_source", "subagent"),
        "app-server-directory" => ("parentThreadId", "threadSource", "subAgent"),
        _ => {
            e.invalid = true;
            ("", "", "")
        }
    };
    let raw_parent = incoming.get(parent_key);
    let parent = label(raw_parent);
    if raw_parent.is_some_and(|v| !v.is_null()) && parent.is_none() {
        e.invalid = true;
    }
    let raw_thread_source = incoming.get(thread_source_key);
    let thread_source = label(raw_thread_source).map(|s| {
        if ["user", "subagent", "memory_consolidation"].contains(&s.as_str()) {
            s
        } else {
            "unsupported".into()
        }
    });
    if raw_thread_source.is_some_and(|v| !v.is_null()) && thread_source.is_none() {
        e.invalid = true;
    }
    let (child, nested_parent, source) = subagent_source(incoming.get("source"), source_variant);
    let internal = source
        .as_ref()
        .is_some_and(|s| s.get("internal").and_then(Value::as_str) == Some("memory_consolidation"));
    if incoming.get("source").is_some_and(|s| {
        s.as_object().is_some_and(|o| o.len() != 1)
            || !child && s.get("subAgent").or_else(|| s.get("subagent")).is_some()
            || !internal && s.get("internal").is_some()
    }) {
        e.invalid = true;
    }
    e.explicit_subagent |= child;
    e.non_user_source |= internal;
    for p in parent.iter().chain(nested_parent.iter()) {
        retain_distinct(&mut e.parents, p.clone(), &mut e.parent_conflict);
    }
    if let Some(s) = &thread_source {
        retain_distinct(
            &mut e.thread_sources,
            s.clone(),
            &mut e.thread_source_conflict,
        );
        if version.is_some_and(|v| VERIFIED_THREAD_SOURCE_VERSIONS.contains(&v)) {
            e.verified_user |= s == "user";
            e.verified_subagent |= s == "subagent";
        }
    }
    if parent.is_some() || thread_source.is_some() || source.is_some() {
        let observation = json!({"channel":channel,"producerVersion":version.filter(|v|v.len()<=80 && !v.chars().any(char::is_control)),"parentThreadId":parent,"threadSource":thread_source,"source":source});
        if !e.observations.contains(&observation) {
            e.observations.push(observation);
            e.observations.sort_by_key(Value::to_string);
            e.observations.truncate(8);
        }
    }
    data["agentKindEvidence"] =
        serde_json::to_value(e).expect("relationship evidence is serializable");
    project(data);
}

pub(super) fn project(data: &mut Value) {
    let e = evidence(data);
    let invalid_parent = e.parents.iter().any(|p| p == &strv(data, "id"));
    let has_child = !e.parents.is_empty() || e.explicit_subagent || e.verified_subagent;
    let user_label = e.thread_sources.iter().any(|s| s == "user");
    let conflict = e.invalid
        || invalid_parent
        || e.parent_conflict
        || e.thread_source_conflict
        || (user_label && (has_child || e.non_user_source));
    let (kind, reason) = if conflict {
        ("unknown", "relationship_conflict")
    } else if e.verified_user {
        ("primary", "versioned_thread_source_user")
    } else if has_child {
        (
            "subagent",
            if !e.parents.is_empty() {
                "explicit_parent"
            } else {
                "explicit_subagent"
            },
        )
    } else if e
        .thread_sources
        .iter()
        .any(|s| s == "user" || s == "subagent")
    {
        ("unknown", "version_unverified")
    } else if !e.thread_sources.is_empty() {
        ("unknown", "unsupported_thread_source")
    } else if e.non_user_source {
        ("unknown", "non_user_internal_source")
    } else {
        ("unknown", "no_positive_evidence")
    };
    data["agentKind"] = json!(kind);
    data["agentKindReason"] = json!(reason);
    data["parentThreadId"] = if !conflict && e.parents.len() == 1 {
        json!(e.parents[0])
    } else {
        Value::Null
    };
    data["rootThreadId"] = Value::Null;
}

pub(super) fn prevents_root_fallback(session: Option<&Value>) -> bool {
    session.is_some_and(|s| {
        s["agentKind"] == "subagent"
            || matches!(
                s["agentKindReason"].as_str(),
                Some("relationship_conflict" | "non_user_internal_source")
            )
    })
}

pub(super) fn validate_query(q: &Value) -> Result<()> {
    if let Some(v) = q.get("agentKind") {
        if !matches!(v.as_str(), Some("primary" | "subagent" | "unknown")) {
            return Err(
                "agentKind 必须为 primary、subagent 或 unknown；全部范围请省略该字段".into(),
            );
        }
    }
    Ok(())
}

pub(super) fn matches(q: &Value, session: Option<&Value>) -> bool {
    q.get("agentKind")
        .and_then(Value::as_str)
        .map(|kind| {
            kind == session
                .and_then(|s| s["agentKind"].as_str())
                .unwrap_or("unknown")
        })
        .unwrap_or(true)
}
