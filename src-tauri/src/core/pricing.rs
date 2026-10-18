//! Append-only, local USD estimates. Prices never rewrite source usage facts.
use super::adapter::{Metric, ResponseUsage, SEMANTICS};
use super::*;
use chrono::{DateTime, SecondsFormat};
use rusqlite::{Transaction, TransactionBehavior};
use std::collections::BTreeSet;

const SETTING_KEY: &str = "usagePricing";
const SCHEMA_VERSION: u64 = 1;
const MAX_PRICE_MICROS: i128 = 1_000_000_000_000;
const USD_UNITS: i128 = 1_000_000_000_000;
const MAX_VERSIONS: usize = 10_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PriceVersion {
    pub id: String,
    pub revision: u64,
    pub model: String,
    pub provider_id: Option<String>,
    pub effective_from: String,
    pub created_at: String,
    pub input_per_million: String,
    pub output_per_million: String,
    pub cache_read_per_million: Option<String>,
    pub cache_write_per_million: Option<String>,
    #[serde(default)]
    pub origin: Option<PriceOrigin>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PriceOrigin {
    pub kind: String,
    pub catalog_provider: String,
    pub catalog_model: String,
    pub synced_at: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PriceInput {
    model: String,
    provider_id: Option<String>,
    effective_from: String,
    input_per_million: String,
    output_per_million: String,
    cache_read_per_million: Option<String>,
    cache_write_per_million: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PriceStore {
    schema_version: u64,
    revision: u64,
    versions: Vec<PriceVersion>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CostEstimate {
    pub estimated_cost_usd: Option<String>,
    pub pricing_version_id: Option<String>,
    pub reason: Option<String>,
    /// One unit is 10^-12 USD. Sum with checked_add, then call format_usd.
    #[serde(skip)]
    pub units: Option<i128>,
}

impl CostEstimate {
    fn unavailable(reason: &str) -> Self {
        Self {
            estimated_cost_usd: None,
            pricing_version_id: None,
            reason: Some(reason.into()),
            units: None,
        }
    }
}

fn label(value: &str, field: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 160
        || value.trim() != value
        || value.chars().any(char::is_control)
        || value == "unknown"
    {
        return Err(format!(
            "{field}必须是1至160字节的明确标识，不能包含控制字符或首尾空白"
        ));
    }
    Ok(())
}

fn timestamp(value: &str, field: &str) -> Result<DateTime<chrono::FixedOffset>> {
    if value.is_empty() || value.len() > 64 {
        return Err(format!("{field}必须是带时区的RFC3339时间"));
    }
    DateTime::parse_from_rfc3339(value).map_err(|_| format!("{field}必须是带时区的RFC3339时间"))
}

/// Exact millionths of one USD per million tokens; no floating point parsing.
fn price_micros(value: &str) -> Result<i128> {
    if value.is_empty() || value.len() > 20 {
        return Err("价格必须是0至1000000之间、最多6位小数的十进制字符串".into());
    }
    let mut pieces = value.split('.');
    let whole = pieces.next().unwrap_or("");
    let fraction = pieces.next();
    if pieces.next().is_some()
        || whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || (whole.len() > 1 && whole.starts_with('0'))
        || fraction
            .is_some_and(|s| s.is_empty() || s.len() > 6 || !s.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err("价格必须是0至1000000之间、最多6位小数的十进制字符串".into());
    }
    let whole = whole.parse::<i128>().map_err(err)?;
    let fraction = fraction.unwrap_or("");
    let fraction_value = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<i128>().map_err(err)?
    };
    let micros = whole
        .checked_mul(1_000_000)
        .and_then(|v| v.checked_add(fraction_value * 10_i128.pow(6 - fraction.len() as u32)))
        .ok_or("价格超出精确整数范围")?;
    if micros > MAX_PRICE_MICROS {
        return Err("价格不能超过每百万Token 1000000 USD".into());
    }
    Ok(micros)
}

fn validate_version(version: &PriceVersion) -> Result<()> {
    label(&version.model, "model")?;
    if let Some(provider) = &version.provider_id {
        label(provider, "providerId")?;
    }
    timestamp(&version.effective_from, "effectiveFrom")?;
    timestamp(&version.created_at, "createdAt")?;
    if uuid::Uuid::parse_str(&version.id).is_err() || version.revision == 0 {
        return Err("价格版本身份或修订号无效".into());
    }
    for price in [&version.input_per_million, &version.output_per_million] {
        price_micros(price)?;
    }
    for price in [
        &version.cache_read_per_million,
        &version.cache_write_per_million,
    ]
    .into_iter()
    .flatten()
    {
        price_micros(price)?;
    }
    if let Some(origin) = &version.origin {
        if origin.kind != "models.dev" {
            return Err("未知价格来源类型；原配置已保留".into());
        }
        label(&origin.catalog_provider, "catalogProvider")?;
        label(&origin.catalog_model, "catalogModel")?;
        timestamp(&origin.synced_at, "syncedAt")?;
    }
    Ok(())
}

fn read_store(db: &Connection) -> Result<PriceStore> {
    let Some(raw) = get_setting(db, SETTING_KEY)? else {
        return Ok(PriceStore {
            schema_version: SCHEMA_VERSION,
            revision: 0,
            versions: vec![],
        });
    };
    if raw.get("schemaVersion").and_then(Value::as_u64) != Some(SCHEMA_VERSION) {
        return Err("价格配置版本未知；原配置已保留，请升级应用或检查配置".into());
    }
    let store: PriceStore =
        serde_json::from_value(raw).map_err(|e| format!("价格配置损坏，原配置已保留：{e}"))?;
    if store.versions.len() > MAX_VERSIONS {
        return Err("价格版本数量超出支持范围，原配置已保留".into());
    }
    let mut ids = BTreeSet::new();
    let mut revision = 0;
    for version in &store.versions {
        validate_version(version)?;
        if version.revision <= revision || !ids.insert(&version.id) {
            return Err("价格配置存在重复身份或非递增修订号，原配置已保留".into());
        }
        revision = version.revision;
    }
    if revision != store.revision {
        return Err("价格配置修订号不一致，原配置已保留".into());
    }
    Ok(store)
}

pub(super) fn versions(db: &Connection) -> Result<Vec<PriceVersion>> {
    Ok(read_store(db)?.versions)
}

pub(super) fn save(db: &Connection, payload: &Value) -> Result<PriceVersion> {
    let input: PriceInput =
        serde_json::from_value(payload.clone()).map_err(|e| format!("价格表单无效：{e}"))?;
    // Lock before reading revision, so two connections cannot overwrite history.
    let tx = Transaction::new_unchecked(db, TransactionBehavior::Immediate).map_err(err)?;
    let mut store = read_store(&tx)?;
    if store.versions.len() >= MAX_VERSIONS {
        return Err("已达到10000个价格版本上限，历史版本已保留".into());
    }
    let revision = store
        .revision
        .checked_add(1)
        .ok_or("价格版本修订号已耗尽")?;
    let version = PriceVersion {
        id: uuid::Uuid::new_v4().to_string(),
        revision,
        model: input.model,
        provider_id: input.provider_id,
        effective_from: timestamp(&input.effective_from, "effectiveFrom")?
            .with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::AutoSi, true),
        created_at: now(),
        input_per_million: input.input_per_million,
        output_per_million: input.output_per_million,
        cache_read_per_million: input.cache_read_per_million,
        cache_write_per_million: input.cache_write_per_million,
        origin: None,
    };
    validate_version(&version)?;
    store.revision = revision;
    store.versions.push(version.clone());
    put_setting(&tx, SETTING_KEY, &serde_json::to_value(store).map_err(err)?)?;
    tx.commit().map_err(err)?;
    Ok(version)
}

fn same_prices(left: &PriceVersion, right: &PriceVersion) -> Result<bool> {
    if price_micros(&left.input_per_million)? != price_micros(&right.input_per_million)?
        || price_micros(&left.output_per_million)? != price_micros(&right.output_per_million)?
    {
        return Ok(false);
    }
    for (left, right) in [
        (&left.cache_read_per_million, &right.cache_read_per_million),
        (
            &left.cache_write_per_million,
            &right.cache_write_per_million,
        ),
    ] {
        match (left, right) {
            (None, None) => {}
            (Some(left), Some(right)) if price_micros(left)? == price_micros(right)? => {}
            _ => return Ok(false),
        }
    }
    Ok(true)
}

/// Atomic append within the caller's transaction. No network or nested transaction.
/// Returns (new versions, unchanged or protected manual entries).
pub(super) fn sync_prices(
    db: &Connection,
    items: &[Value],
    at: &str,
    overwrite_manual: bool,
) -> Result<(usize, usize)> {
    if db.is_autocommit() {
        return Err("价格同步必须在调用方事务内执行".into());
    }
    if items.len() > 100 {
        return Err("单批价格同步最多100项".into());
    }
    let at = timestamp(at, "syncedAt")?
        .with_timezone(&Utc)
        .to_rfc3339_opts(SecondsFormat::AutoSi, true);
    let mut prepared = Vec::with_capacity(items.len());
    let mut targets = BTreeSet::new();
    // Validate the complete batch before changing even the in-memory store.
    for raw in items {
        let mut fields = raw.as_object().cloned().ok_or("同步价格项必须是对象")?;
        let catalog_provider = fields
            .remove("catalogProvider")
            .and_then(|value| value.as_str().map(str::to_owned))
            .ok_or("同步价格缺少catalogProvider")?;
        let catalog_model = fields
            .remove("catalogModel")
            .and_then(|value| value.as_str().map(str::to_owned))
            .ok_or("同步价格缺少catalogModel")?;
        let input: PriceInput = serde_json::from_value(Value::Object(fields))
            .map_err(|e| format!("同步价格项无效：{e}"))?;
        timestamp(&input.effective_from, "effectiveFrom")?;
        let version = PriceVersion {
            id: uuid::Uuid::new_v4().to_string(),
            revision: 1,
            model: input.model,
            provider_id: input.provider_id,
            effective_from: at.clone(),
            created_at: at.clone(),
            input_per_million: input.input_per_million,
            output_per_million: input.output_per_million,
            cache_read_per_million: input.cache_read_per_million,
            cache_write_per_million: input.cache_write_per_million,
            origin: Some(PriceOrigin {
                kind: "models.dev".into(),
                catalog_provider,
                catalog_model,
                synced_at: at.clone(),
            }),
        };
        validate_version(&version)?;
        if !targets.insert((version.model.clone(), version.provider_id.clone())) {
            return Err("同一批同步包含重复模型和供应商目标".into());
        }
        prepared.push(version);
    }
    let mut store = read_store(db)?;
    let mut appended = 0;
    let mut skipped = 0;
    for mut version in prepared {
        let current = store
            .versions
            .iter()
            .filter(|current| {
                current.model == version.model && current.provider_id == version.provider_id
            })
            .max_by_key(|current| current.revision);
        if let Some(current) = current {
            if current.origin.is_none() && !overwrite_manual || same_prices(current, &version)? {
                skipped += 1;
                continue;
            }
        }
        if store.versions.len() >= MAX_VERSIONS {
            return Err("已达到10000个价格版本上限，历史版本已保留".into());
        }
        store.revision = store
            .revision
            .checked_add(1)
            .ok_or("价格版本修订号已耗尽")?;
        version.revision = store.revision;
        store.versions.push(version);
        appended += 1;
    }
    if appended > 0 {
        put_setting(db, SETTING_KEY, &serde_json::to_value(store).map_err(err)?)?;
    }
    Ok((appended, skipped))
}

/// Exact decimal string. Consumers only pass non-negative checked cost units.
pub(super) fn format_usd(units: i128) -> String {
    let whole = units / USD_UNITS;
    let remainder = units % USD_UNITS;
    if remainder == 0 {
        return whole.to_string();
    }
    let fraction = format!("{remainder:012}");
    format!("{whole}.{}", fraction.trim_end_matches('0'))
}

fn count(metric: &Metric) -> Option<i128> {
    metric.value().filter(|n| *n >= 0).map(i128::from)
}

pub(super) fn estimate(
    versions: &[PriceVersion],
    response: &ResponseUsage,
) -> Result<CostEstimate> {
    if response.semantics_version != SEMANTICS
        || response.completeness != "source-recorded"
        || response.phase != "final"
    {
        return Ok(CostEstimate::unavailable("unsupported_usage_semantics"));
    }
    let counts = &response.counts;
    let (Some(input), Some(output), Some(read), Some(write), Some(total)) = (
        count(&counts.input),
        count(&counts.output),
        count(&counts.cache_read),
        count(&counts.cache_write),
        count(&counts.total),
    ) else {
        return Ok(CostEstimate::unavailable(
            "incomplete_or_invalid_token_counts",
        ));
    };
    if input.checked_add(output) != Some(total)
        || read.checked_add(write).is_none_or(|cache| cache > input)
    {
        return Ok(CostEstimate::unavailable("inconsistent_token_counts"));
    }
    // Pricing accepts native response facts only; normalized zeroes must never
    // conceal missing native fields. Reasoning is already included in output.
    for (key, expected) in [
        ("input_tokens", input),
        ("output_tokens", output),
        ("cached_input_tokens", read),
        ("cache_write_input_tokens", write),
    ] {
        if response.native_counts.get(key).and_then(count) != Some(expected) {
            return Ok(CostEstimate::unavailable(
                "incomplete_or_invalid_native_counts",
            ));
        }
    }
    // The current adapter explicitly derives total from valid input + output
    // when native total is absent. Respect that contract without inventing
    // zeroes for the four billable fields. An explicit bad total stays bad.
    match response.native_counts.get("total_tokens") {
        None | Some(Metric::Missing) => {}
        Some(Metric::Valid(value)) if i128::from(*value) == total => {}
        _ => {
            return Ok(CostEstimate::unavailable(
                "incomplete_or_invalid_native_counts",
            ));
        }
    }
    let Some(model) = response
        .observed_model
        .as_deref()
        .filter(|v| !v.is_empty())
        .or_else(|| {
            response
                .requested_model
                .as_deref()
                .filter(|v| !v.is_empty())
        })
    else {
        return Ok(CostEstimate::unavailable("unknown_model"));
    };
    let Ok(event_time) = timestamp(&response.event_time, "eventTime") else {
        return Ok(CostEstimate::unavailable("invalid_event_time"));
    };
    let mut selected: Option<(&PriceVersion, DateTime<chrono::FixedOffset>)> = None;
    for version in versions {
        // Validation also protects internal callers from constructing bad prices.
        validate_version(version)?;
        if version.model != model {
            continue;
        }
        if version.provider_id.is_some()
            && version.provider_id.as_deref() != response.provider_id.as_deref()
        {
            continue;
        }
        let effective = timestamp(&version.effective_from, "effectiveFrom")?;
        if effective > event_time {
            continue;
        }
        let replace = selected.as_ref().is_none_or(|(old, old_effective)| {
            (version.provider_id.is_some(), effective, version.revision)
                > (old.provider_id.is_some(), *old_effective, old.revision)
        });
        if replace {
            selected = Some((version, effective));
        }
    }
    let Some((version, _)) = selected else {
        return Ok(CostEstimate::unavailable("no_matching_price"));
    };
    let fresh = input
        .checked_sub(read)
        .and_then(|value| value.checked_sub(write))
        .ok_or("新增输入Token计算超出精确整数范围")?;
    let mut units: i128 = 0;
    for (tokens, price, missing) in [
        (
            fresh,
            Some(version.input_per_million.as_str()),
            "missing_input_price",
        ),
        (
            output,
            Some(version.output_per_million.as_str()),
            "missing_output_price",
        ),
        (
            read,
            version.cache_read_per_million.as_deref(),
            "missing_cache_read_price",
        ),
        (
            write,
            version.cache_write_per_million.as_deref(),
            "missing_cache_write_price",
        ),
    ] {
        if tokens == 0 {
            continue;
        }
        let Some(price) = price else {
            let mut estimate = CostEstimate::unavailable(missing);
            estimate.pricing_version_id = Some(version.id.clone());
            return Ok(estimate);
        };
        units = tokens
            .checked_mul(price_micros(price)?)
            .and_then(|cost| units.checked_add(cost))
            .ok_or("估算成本超出精确整数范围，原始账本已保留")?;
    }
    Ok(CostEstimate {
        estimated_cost_usd: Some(format_usd(units)),
        pricing_version_id: Some(version.id.clone()),
        reason: None,
        units: Some(units),
    })
}

impl Core {
    pub fn usage_price_versions(&self) -> Result<Value> {
        serde_json::to_value(versions(&self.db)?).map_err(err)
    }

    pub fn save_usage_price(&mut self, payload: Value) -> Result<Value> {
        serde_json::to_value(save(&self.db, &payload)?).map_err(err)
    }
}

#[cfg(test)]
#[path = "pricing_tests.rs"]
mod tests;
