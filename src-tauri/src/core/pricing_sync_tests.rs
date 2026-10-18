use super::*;

struct Fixture {
    _tmp: tempfile::TempDir,
    core: Core,
}
impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        fs::create_dir(&source).unwrap();
        let core = Core::open(&tmp.path().join("data"), &source).unwrap();
        Self { _tmp: tmp, core }
    }
    fn configure(&mut self, mappings: Value) -> Value {
        let revision = self.core.usage_price_sync().unwrap()["revision"].clone();
        self.core.save_usage_price_sync(json!({"revision":revision,"enabled":false,"overwriteManual":false,"mappings":mappings})).unwrap()
    }
    fn apply(&mut self, revision: u64, items: Value, skipped: usize) -> Result<Value> {
        self.core
            .apply_usage_price_sync(json!({"revision":revision,"items":items,"skipped":skipped}))
    }
}

fn mapping(catalog_model: &str, model: &str, provider: Option<&str>) -> Value {
    json!({"catalogProvider":"openai","catalogModel":catalog_model,"model":model,"providerId":provider})
}
fn item(catalog_model: &str) -> Value {
    json!({"catalogProvider":"openai","catalogModel":catalog_model,"inputPerMillion":"1","outputPerMillion":"2",
        "cacheReadPerMillion":"0.1","cacheWritePerMillion":null})
}

#[test]
fn default_sync_configuration_is_disabled_and_does_not_create_settings() {
    let fixture = Fixture::new();
    assert_eq!(
        fixture.core.usage_price_sync().unwrap(),
        json!({"schemaVersion":1,"revision":0,"enabled":false,
        "overwriteManual":false,"mappings":[],"lastAttemptAt":null,"lastSuccessAt":null,"error":null,"changed":0,"skipped":0})
    );
    assert!(get_setting(&fixture.core.db, SETTING_KEY)
        .unwrap()
        .is_none());
    assert!(get_setting(&fixture.core.db, "usagePricing")
        .unwrap()
        .is_none());
}

#[test]
fn config_save_is_compare_and_swap_and_preserves_previous_receipt() {
    let mut fixture = Fixture::new();
    let first = fixture.configure(json!([mapping("catalog-a", "local-a", None)]));
    assert_eq!(first["revision"], 1);
    fixture
        .core
        .record_usage_price_sync_error(
            json!({"revision":1,"error":"Directory temporarily unavailable"}),
        )
        .unwrap();
    let previous = fixture.core.usage_price_sync().unwrap();
    let saved = fixture
        .core
        .save_usage_price_sync(json!({"revision":1,"enabled":true,"overwriteManual":true,
        "mappings":[mapping("catalog-b","local-b",Some("provider"))]}))
        .unwrap();
    assert_eq!(saved["revision"], 2);
    assert_eq!(saved["enabled"], true);
    assert_eq!(saved["lastAttemptAt"], previous["lastAttemptAt"]);
    assert_eq!(saved["error"], previous["error"]);
    assert!(fixture
        .core
        .save_usage_price_sync(
            json!({"revision":1,"enabled":false,"overwriteManual":false,"mappings":[]})
        )
        .is_err());
    assert_eq!(fixture.core.usage_price_sync().unwrap(), saved);
}

#[test]
fn invalid_mappings_and_unknown_payload_fields_never_replace_config() {
    let mut fixture = Fixture::new();
    let existing = fixture.configure(json!([mapping("catalog-a", "local-a", None)]));
    let mut bad_label = mapping("catalog-b", "local-b", None);
    bad_label["catalogProvider"] = json!(" openai ");
    let mut bad_model = mapping("catalog-b", "local-b", None);
    bad_model["model"] = json!("unknown");
    let mut bad_provider = mapping("catalog-b", "local-b", None);
    bad_provider["providerId"] = json!("bad\nprovider");
    for mappings in [
        json!([
            mapping("catalog-a", "local-a", None),
            mapping("catalog-a", "local-b", None)
        ]),
        json!([
            mapping("catalog-a", "local-a", None),
            mapping("catalog-b", "local-a", None)
        ]),
        json!([bad_label]),
        json!([bad_model]),
        json!([bad_provider]),
        json!((0..101)
            .map(|index| mapping(&format!("source-{index}"), &format!("target-{index}"), None))
            .collect::<Vec<_>>()),
    ] {
        assert!(fixture
            .core
            .save_usage_price_sync(
                json!({"revision":1,"enabled":true,"overwriteManual":false,"mappings":mappings})
            )
            .is_err());
        assert_eq!(fixture.core.usage_price_sync().unwrap(), existing);
    }
    assert!(fixture
        .core
        .save_usage_price_sync(
            json!({"revision":1,"enabled":true,"overwriteManual":false,"mappings":[],"extra":true})
        )
        .is_err());
    assert_eq!(fixture.core.usage_price_sync().unwrap(), existing);
}

#[test]
fn manual_sync_uses_explicit_mapping_without_enabling_automatic_sync() {
    let mut fixture = Fixture::new();
    fixture.configure(json!([mapping(
        "catalog-a",
        "local-alias",
        Some("local-provider")
    )]));
    let result = fixture.apply(1, json!([item("catalog-a")]), 0).unwrap();
    assert_eq!(result["revision"], 1);
    assert_eq!(result["enabled"], false);
    assert_eq!(result["changed"], 1);
    assert_eq!(result["skipped"], 0);
    assert!(result["error"].is_null());
    assert_eq!(result["lastAttemptAt"], result["lastSuccessAt"]);
    assert!(DateTime::parse_from_rfc3339(result["lastSuccessAt"].as_str().unwrap()).is_ok());
    let versions = fixture.core.usage_price_versions().unwrap();
    assert_eq!(versions.as_array().unwrap().len(), 1);
    let version = &versions[0];
    assert_eq!(version["model"], "local-alias");
    assert_eq!(version["providerId"], "local-provider");
    assert_eq!(version["origin"]["catalogProvider"], "openai");
    assert_eq!(version["origin"]["catalogModel"], "catalog-a");
    let succeeded_at =
        DateTime::parse_from_rfc3339(result["lastSuccessAt"].as_str().unwrap()).unwrap();
    assert_eq!(
        DateTime::parse_from_rfc3339(version["origin"]["syncedAt"].as_str().unwrap()).unwrap(),
        succeeded_at
    );
    assert_eq!(
        DateTime::parse_from_rfc3339(version["effectiveFrom"].as_str().unwrap()).unwrap(),
        succeeded_at
    );
    assert!(version["cacheWritePerMillion"].is_null());
}

#[test]
fn stale_sync_or_error_cannot_write_after_configuration_changes() {
    let mut fixture = Fixture::new();
    fixture.configure(json!([mapping("catalog-a", "local-a", None)]));
    let current = fixture.configure(json!([mapping("catalog-b", "local-b", None)]));
    assert!(fixture.apply(1, json!([item("catalog-a")]), 0).is_err());
    assert!(fixture
        .core
        .record_usage_price_sync_error(json!({"revision":1,"error":"old failure"}))
        .is_err());
    assert_eq!(fixture.core.usage_price_sync().unwrap(), current);
    assert!(fixture
        .core
        .usage_price_versions()
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn apply_requires_unique_mapped_sources_and_exact_missing_item_count() {
    let mut fixture = Fixture::new();
    let config = fixture.configure(json!([
        mapping("catalog-a", "local-a", None),
        mapping("catalog-b", "local-b", None)
    ]));
    for (items, skipped) in [
        (json!([item("catalog-a")]), 0),
        (json!([item("catalog-a")]), 2),
        (json!([item("catalog-a"), item("catalog-a")]), 0),
        (json!([item("unknown")]), 1),
        (
            json!([item("catalog-a"), item("catalog-b"), item("catalog-c")]),
            0,
        ),
    ] {
        assert!(fixture.apply(1, items, skipped).is_err());
        assert_eq!(fixture.core.usage_price_sync().unwrap(), config);
        assert!(fixture
            .core
            .usage_price_versions()
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty());
    }
    let applied = fixture.apply(1, json!([item("catalog-a")]), 1).unwrap();
    assert_eq!(applied["changed"], 1);
    assert_eq!(applied["skipped"], 1);
}

#[test]
fn invalid_later_price_rolls_back_price_versions_and_sync_receipt_together() {
    let mut fixture = Fixture::new();
    let config = fixture.configure(json!([
        mapping("catalog-a", "local-a", None),
        mapping("catalog-b", "local-b", None)
    ]));
    let mut invalid = item("catalog-b");
    invalid["outputPerMillion"] = json!("NaN");
    assert!(fixture
        .apply(1, json!([item("catalog-a"), invalid]), 0)
        .is_err());
    assert_eq!(fixture.core.usage_price_sync().unwrap(), config);
    assert!(get_setting(&fixture.core.db, "usagePricing")
        .unwrap()
        .is_none());
}

#[test]
fn receipt_write_failure_rolls_back_already_prepared_price_store() {
    let mut fixture = Fixture::new();
    let config = fixture.configure(json!([mapping("catalog-a", "local-a", None)]));
    fixture.core.db.execute_batch("CREATE TRIGGER reject_sync_receipt BEFORE UPDATE ON settings
        WHEN NEW.key='usagePricingSync' BEGIN SELECT RAISE(ABORT,'fixture receipt write refused'); END;").unwrap();
    let error = fixture.apply(1, json!([item("catalog-a")]), 0).unwrap_err();
    assert!(error.contains("fixture receipt write refused"), "{error}");
    assert_eq!(fixture.core.usage_price_sync().unwrap(), config);
    assert!(get_setting(&fixture.core.db, "usagePricing")
        .unwrap()
        .is_none());
}

#[test]
fn unchanged_prices_are_skipped_without_appending_or_enabling_sync() {
    let mut fixture = Fixture::new();
    fixture.configure(json!([mapping("catalog-a", "local-a", None)]));
    fixture.apply(1, json!([item("catalog-a")]), 0).unwrap();
    let versions = fixture.core.usage_price_versions().unwrap();
    let result = fixture.apply(1, json!([item("catalog-a")]), 0).unwrap();
    assert_eq!(result["changed"], 0);
    assert_eq!(result["skipped"], 1);
    assert_eq!(result["enabled"], false);
    assert_eq!(fixture.core.usage_price_versions().unwrap(), versions);
}

#[test]
fn failure_receipt_preserves_last_success_prices_and_changed_counts() {
    let mut fixture = Fixture::new();
    fixture.configure(json!([mapping("catalog-a", "local-a", None)]));
    let success = fixture.apply(1, json!([item("catalog-a")]), 0).unwrap();
    let prices = fixture.core.usage_price_versions().unwrap();
    let failed = fixture
        .core
        .record_usage_price_sync_error(json!({"revision":1,"error":"models.dev returned HTTP 503"}))
        .unwrap();
    assert_eq!(failed["lastSuccessAt"], success["lastSuccessAt"]);
    assert_eq!(failed["changed"], 1);
    assert_eq!(failed["skipped"], 0);
    assert_eq!(failed["error"], "models.dev returned HTTP 503");
    assert_eq!(fixture.core.usage_price_versions().unwrap(), prices);
    for error in [
        json!(""),
        json!("bad\nerror"),
        json!("a".repeat(2049)),
        json!(null),
    ] {
        assert!(fixture
            .core
            .record_usage_price_sync_error(json!({"revision":1,"error":error}))
            .is_err());
        assert_eq!(fixture.core.usage_price_sync().unwrap(), failed);
    }
}

#[test]
fn manual_prices_are_preserved_until_overwrite_is_explicitly_enabled() {
    let mut fixture = Fixture::new();
    fixture.configure(json!([mapping("catalog-a", "local-a", None)]));
    fixture.core.save_usage_price(json!({"model":"local-a","providerId":null,"effectiveFrom":"2020-01-01T00:00:00Z",
        "inputPerMillion":"5","outputPerMillion":"6","cacheReadPerMillion":"0.5","cacheWritePerMillion":"0"})).unwrap();
    let original = fixture.core.usage_price_versions().unwrap();
    let protected = fixture.apply(1, json!([item("catalog-a")]), 0).unwrap();
    assert_eq!(protected["changed"], 0);
    assert_eq!(protected["skipped"], 1);
    assert_eq!(fixture.core.usage_price_versions().unwrap(), original);
    fixture
        .core
        .save_usage_price_sync(json!({"revision":1,"enabled":false,"overwriteManual":true,
        "mappings":[mapping("catalog-a","local-a",None)]}))
        .unwrap();
    let overwritten = fixture.apply(2, json!([item("catalog-a")]), 0).unwrap();
    assert_eq!(overwritten["changed"], 1);
    assert_eq!(
        fixture
            .core
            .usage_price_versions()
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn malformed_or_unknown_stored_configuration_fails_closed_without_replacement() {
    let mut fixture = Fixture::new();
    for invalid in [
        json!({"schemaVersion":2}),
        json!({"schemaVersion":1,"revision":0}),
        json!({"schemaVersion":1,"revision":0,"enabled":false,"overwriteManual":false,"mappings":[],
            "lastAttemptAt":"not-a-time","lastSuccessAt":null,"error":null,"changed":0,"skipped":0}),
    ] {
        put_setting(&fixture.core.db, SETTING_KEY, &invalid).unwrap();
        assert!(fixture.core.usage_price_sync().is_err());
        assert!(fixture
            .core
            .save_usage_price_sync(
                json!({"revision":0,"enabled":false,"overwriteManual":false,"mappings":[]})
            )
            .is_err());
        assert!(fixture.apply(0, json!([]), 0).is_err());
        assert!(fixture
            .core
            .record_usage_price_sync_error(json!({"revision":0,"error":"failure"}))
            .is_err());
        assert_eq!(
            get_setting(&fixture.core.db, SETTING_KEY).unwrap(),
            Some(invalid)
        );
    }
}
