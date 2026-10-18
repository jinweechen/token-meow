//! Explicit models.dev mapping and atomic price synchronization receipts.
//! Transport stays outside Core; an outdated configuration can never apply prices.
use super::*;
use chrono::DateTime;
use rusqlite::TransactionBehavior;
use std::collections::{BTreeMap, BTreeSet};

const SETTING_KEY: &str = "usagePricingSync";
const SCHEMA_VERSION: u64 = 1;
const MAX_MAPPINGS: usize = 100;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Mapping {
    catalog_provider: String,
    catalog_model: String,
    model: String,
    provider_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Config {
    schema_version: u64,
    revision: u64,
    enabled: bool,
    overwrite_manual: bool,
    mappings: Vec<Mapping>,
    last_attempt_at: Option<String>,
    last_success_at: Option<String>,
    error: Option<String>,
    changed: usize,
    skipped: usize,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            revision: 0,
            enabled: false,
            overwrite_manual: false,
            mappings: vec![],
            last_attempt_at: None,
            last_success_at: None,
            error: None,
            changed: 0,
            skipped: 0,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SaveInput {
    revision: u64,
    enabled: bool,
    overwrite_manual: bool,
    mappings: Vec<Mapping>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CatalogItem {
    catalog_provider: String,
    catalog_model: String,
    input_per_million: String,
    output_per_million: String,
    cache_read_per_million: Option<String>,
    cache_write_per_million: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApplyInput {
    revision: u64,
    items: Vec<CatalogItem>,
    skipped: usize,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ErrorInput {
    revision: u64,
    error: String,
}

fn label(value: &str, field: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 160
        || value.trim() != value
        || value == "unknown"
        || value.chars().any(char::is_control)
    {
        return Err(format!(
            "{field}必须是1至160字节的明确标识，不能包含控制字符或首尾空白"
        ));
    }
    Ok(())
}

fn error_text(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 2048 || value.chars().any(char::is_control) {
        return Err("同步错误必须是1至2048字节且不含控制字符的文本".into());
    }
    Ok(())
}

fn validate_mappings(mappings: &[Mapping]) -> Result<()> {
    if mappings.len() > MAX_MAPPINGS {
        return Err(format!("价格同步最多配置 {MAX_MAPPINGS} 条映射"));
    }
    let mut sources = BTreeSet::new();
    let mut targets = BTreeSet::new();
    for mapping in mappings {
        label(&mapping.catalog_provider, "catalogProvider")?;
        label(&mapping.catalog_model, "catalogModel")?;
        label(&mapping.model, "model")?;
        if let Some(provider) = &mapping.provider_id {
            label(provider, "providerId")?;
        }
        if !sources.insert((&mapping.catalog_provider, &mapping.catalog_model)) {
            return Err("价格同步存在重复的目录供应商与模型".into());
        }
        if !targets.insert((&mapping.model, &mapping.provider_id)) {
            return Err("价格同步存在重复的本地模型与供应商".into());
        }
    }
    Ok(())
}

fn read_config(db: &Connection) -> Result<Config> {
    let Some(value) = get_setting(db, SETTING_KEY)? else {
        return Ok(Config::default());
    };
    if value.get("schemaVersion").and_then(Value::as_u64) != Some(SCHEMA_VERSION) {
        return Err("价格同步配置版本未知，原配置已保留".into());
    }
    let config: Config = serde_json::from_value(value)
        .map_err(|error| format!("价格同步配置损坏，原配置已保留：{error}"))?;
    validate_mappings(&config.mappings)?;
    for timestamp in [&config.last_attempt_at, &config.last_success_at]
        .into_iter()
        .flatten()
    {
        if timestamp.len() > 64 || DateTime::parse_from_rfc3339(timestamp).is_err() {
            return Err("价格同步配置中的时间无效，原配置已保留".into());
        }
    }
    if let Some(error) = &config.error {
        error_text(error)?;
    }
    if config.changed > MAX_MAPPINGS
        || config.skipped > MAX_MAPPINGS
        || config
            .changed
            .checked_add(config.skipped)
            .is_none_or(|total| total > MAX_MAPPINGS)
    {
        return Err("价格同步记录数量无效，原配置已保留".into());
    }
    Ok(config)
}

fn check_revision(config: &Config, revision: u64) -> Result<()> {
    if config.revision != revision {
        return Err("价格同步配置已更新，请重新读取配置后重试".into());
    }
    Ok(())
}

fn value(config: &Config) -> Result<Value> {
    serde_json::to_value(config).map_err(err)
}
fn store_config(db: &Connection, config: &Config) -> Result<()> {
    put_setting(db, SETTING_KEY, &value(config)?)
}

impl Core {
    pub fn usage_price_sync(&self) -> Result<Value> {
        value(&read_config(&self.db)?)
    }
    pub fn save_usage_price_sync(&mut self, payload: Value) -> Result<Value> {
        let input: SaveInput = serde_json::from_value(payload).map_err(err)?;
        validate_mappings(&input.mappings)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(err)?;
        let mut config = read_config(&tx)?;
        check_revision(&config, input.revision)?;
        config.revision = config
            .revision
            .checked_add(1)
            .ok_or("价格同步配置修订号已用尽")?;
        config.enabled = input.enabled;
        config.overwrite_manual = input.overwrite_manual;
        config.mappings = input.mappings;
        store_config(&tx, &config)?;
        tx.commit().map_err(err)?;
        value(&config)
    }
    pub fn apply_usage_price_sync(&mut self, payload: Value) -> Result<Value> {
        let input: ApplyInput = serde_json::from_value(payload).map_err(err)?;
        if input.items.len() > MAX_MAPPINGS {
            return Err("单次价格同步最多100项".into());
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(err)?;
        let mut config = read_config(&tx)?;
        check_revision(&config, input.revision)?;
        let expected_skipped = config
            .mappings
            .len()
            .checked_sub(input.items.len())
            .ok_or("同步价格数量超过已配置映射数量")?;
        if input.skipped != expected_skipped {
            return Err("同步遗漏数量与当前映射不一致，请重新读取目录后重试".into());
        }
        let mapping: BTreeMap<_, _> = config
            .mappings
            .iter()
            .map(|mapping| ((&mapping.catalog_provider, &mapping.catalog_model), mapping))
            .collect();
        let at = now();
        let mut sources = BTreeSet::new();
        let mut prices = Vec::with_capacity(input.items.len());
        for item in &input.items {
            label(&item.catalog_provider, "catalogProvider")?;
            label(&item.catalog_model, "catalogModel")?;
            let source = (&item.catalog_provider, &item.catalog_model);
            if !sources.insert(source) {
                return Err("同步价格包含重复目录模型".into());
            }
            let target = mapping
                .get(&source)
                .ok_or("同步价格没有匹配当前映射，原配置与价格已保留")?;
            prices.push(json!({"model":target.model,"providerId":target.provider_id,"effectiveFrom":at,
                "inputPerMillion":item.input_per_million,"outputPerMillion":item.output_per_million,
                "cacheReadPerMillion":item.cache_read_per_million,"cacheWritePerMillion":item.cache_write_per_million,
                "catalogProvider":item.catalog_provider,"catalogModel":item.catalog_model}));
        }
        let (changed, skipped) = pricing::sync_prices(&tx, &prices, &at, config.overwrite_manual)?;
        if changed.checked_add(skipped) != Some(prices.len()) {
            return Err("同步结果数量与输入不一致，原配置与价格已保留".into());
        }
        config.last_attempt_at = Some(at.clone());
        config.last_success_at = Some(at);
        config.error = None;
        config.changed = changed;
        config.skipped = skipped
            .checked_add(input.skipped)
            .ok_or("同步跳过数量超出范围")?;
        store_config(&tx, &config)?;
        tx.commit().map_err(err)?;
        value(&config)
    }
    pub fn record_usage_price_sync_error(&mut self, payload: Value) -> Result<Value> {
        let input: ErrorInput = serde_json::from_value(payload).map_err(err)?;
        error_text(&input.error)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(err)?;
        let mut config = read_config(&tx)?;
        check_revision(&config, input.revision)?;
        config.last_attempt_at = Some(now());
        config.error = Some(input.error);
        store_config(&tx, &config)?;
        tx.commit().map_err(err)?;
        value(&config)
    }
}

#[cfg(test)]
#[path = "pricing_sync_tests.rs"]
mod tests;
