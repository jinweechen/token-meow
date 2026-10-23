use super::*;
use serde_json::json;

fn committed(registry: &mut Registry) -> (Reservation, PetDescription) {
    let reservation = registry.reserve(PetConfig::default()).unwrap();
    let description = registry
        .commit(&reservation, &reservation.window_label)
        .unwrap();
    (reservation, description)
}
fn renderer(registry: &mut Registry, description: &PetDescription) -> RendererLease {
    registry
        .register_renderer(
            &description.label,
            &description.identity(),
            &uuid::Uuid::new_v4().to_string(),
        )
        .unwrap()
}
fn bounds() -> PlacementBounds {
    PlacementBounds {
        work_area: WorkArea {
            x: -1920,
            y: -100,
            width: 1920,
            height: 1080,
        },
        window_size: WindowSize {
            width: 280,
            height: 340,
        },
    }
}

#[test]
fn reservation_is_not_an_authorized_window_and_pending_or_live_entries_limit_total_to_two_pets() {
    let mut registry = Registry::default();
    let reservation = registry.reserve(PetConfig::default()).unwrap();
    assert!(registry.list().is_empty());
    assert!(registry.get_by_label(&reservation.window_label).is_none());
    assert!(registry.reserve(PetConfig::default()).is_err());
    assert!(uuid::Uuid::parse_str(&reservation.identity.pet_id).is_ok());
    assert!(uuid::Uuid::parse_str(&reservation.identity.instance_epoch).is_ok());
    assert_eq!(
        reservation.window_label,
        format!("extra-pet-{}", reservation.identity.pet_id)
    );
    let description = registry
        .commit(&reservation, &reservation.window_label)
        .unwrap();
    assert_eq!(registry.list(), [description]);
    assert!(registry.reserve(PetConfig::default()).is_err());
    assert!(registry.get_by_label("pet").is_none());
    assert!(registry.get_by_label("main").is_none());
}

#[test]
fn abort_creation_failure_or_wrong_actual_label_frees_the_slot_and_revokes_callbacks() {
    let mut registry = Registry::default();
    let first = registry.reserve(PetConfig::default()).unwrap();
    let stale = first.clone();
    assert!(registry.abort(&first));
    assert!(!stale.is_live());
    assert!(registry.commit(&stale, &stale.window_label).is_err());
    let second = registry.reserve(PetConfig::default()).unwrap();
    assert!(!registry.abort(&stale));
    assert!(second.is_live());
    assert!(registry.commit(&second, "extra-pet-forged").is_err());
    assert!(!second.is_live());
    assert!(registry.reserve(PetConfig::default()).is_ok());
}

#[test]
fn exact_label_and_instance_mapping_cannot_be_replaced_by_a_prefix_url_or_old_owner() {
    let mut registry = Registry::default();
    let (_, old) = committed(&mut registry);
    let old_lease = renderer(&mut registry, &old);
    registry.remove(&old.identity()).unwrap();
    assert!(!old_lease.is_live());
    let (_, current) = committed(&mut registry);
    let current_lease = renderer(&mut registry, &current);
    for label in [
        "extra-pet-",
        "pet",
        "main",
        "index.html?window=extra-pet",
        &format!("{}-extra", current.label),
        &old.label,
    ] {
        assert!(registry.get_by_label(label).is_none());
        assert!(!registry.validate_renderer(label, &current_lease));
    }
    assert!(registry
        .configure(
            &old.identity(),
            PetConfigPatch {
                visible: Some(false),
                ..Default::default()
            }
        )
        .is_err());
    assert!(registry.remove(&old.identity()).is_err());
    assert!(registry
        .revoke_window(&current.label, &old.instance_epoch)
        .is_none());
    assert!(registry.validate_renderer(&current.label, &current_lease));
}

#[test]
fn each_render_registration_cancels_old_epoch_even_if_registration_reuses_the_same_epoch() {
    let mut registry = Registry::default();
    let (_, pet) = committed(&mut registry);
    let epoch = uuid::Uuid::new_v4().to_string();
    let first = registry
        .register_renderer(&pet.label, &pet.identity(), &epoch)
        .unwrap();
    let second = registry
        .register_renderer(&pet.label, &pet.identity(), &epoch)
        .unwrap();
    assert!(!first.is_live());
    assert!(!registry.validate_renderer(&pet.label, &first));
    assert!(second.generation > first.generation);
    assert_ne!(first.token, second.token);
    first.cancel();
    assert!(registry.validate_renderer(&pet.label, &second));
    let third = renderer(&mut registry, &pet);
    assert!(!second.is_live());
    assert!(registry.validate_renderer(&pet.label, &third));
    assert!(registry
        .register_renderer("pet", &pet.identity(), &epoch)
        .is_err());
    assert!(registry
        .register_renderer(&pet.label, &pet.identity(), "../../epoch")
        .is_err());
    assert!(registry.validate_renderer(&pet.label, &third));
}

#[test]
fn ipc_request_token_lookup_returns_only_current_native_bound_lease_without_revoking_on_bad_claims()
{
    let mut registry = Registry::default();
    let (_, pet) = committed(&mut registry);
    let lease = renderer(&mut registry, &pet);
    assert!(registry
        .renderer_for_request(
            &pet.label,
            &pet.identity(),
            &lease.renderer_epoch,
            &lease.token
        )
        .is_some());
    for (label, epoch, token) in [
        ("pet", lease.renderer_epoch.as_str(), lease.token.as_str()),
        (
            pet.label.as_str(),
            "index.html?window=extra",
            lease.token.as_str(),
        ),
        (
            pet.label.as_str(),
            lease.renderer_epoch.as_str(),
            "forged-token",
        ),
    ] {
        assert!(registry
            .renderer_for_request(label, &pet.identity(), epoch, token)
            .is_none());
    }
    let old_identity = PetIdentity {
        pet_id: pet.pet_id.clone(),
        instance_epoch: uuid::Uuid::new_v4().to_string(),
    };
    assert!(registry
        .renderer_for_request(
            &pet.label,
            &old_identity,
            &lease.renderer_epoch,
            &lease.token
        )
        .is_none());
    let next = renderer(&mut registry, &pet);
    assert!(registry
        .renderer_for_request(
            &pet.label,
            &pet.identity(),
            &lease.renderer_epoch,
            &lease.token
        )
        .is_none());
    assert!(registry
        .renderer_for_request(
            &pet.label,
            &pet.identity(),
            &next.renderer_epoch,
            &next.token
        )
        .is_some());
}

#[test]
fn hiding_or_changing_skin_or_scale_cancels_renderer_but_position_updates_do_not() {
    let mut registry = Registry::default();
    let (_, pet) = committed(&mut registry);
    let first = renderer(&mut registry, &pet);
    registry
        .set_position(&pet.identity(), PhysicalPoint { x: 0, y: 0 }, bounds())
        .unwrap();
    assert!(registry.validate_renderer(&pet.label, &first));
    registry
        .configure(
            &pet.identity(),
            PetConfigPatch {
                scale: Some(1.25),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(!first.is_live());
    let second = renderer(&mut registry, &pet);
    registry
        .configure(
            &pet.identity(),
            PetConfigPatch {
                skin_choice: Some(ActiveSkin::Vrm("a".repeat(64))),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(!second.is_live());
    let third = renderer(&mut registry, &pet);
    registry
        .configure(
            &pet.identity(),
            PetConfigPatch {
                visible: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(!third.is_live());
    assert!(registry
        .register_renderer(
            &pet.label,
            &pet.identity(),
            &uuid::Uuid::new_v4().to_string()
        )
        .is_err());
    registry
        .configure(
            &pet.identity(),
            PetConfigPatch {
                visible: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(!registry.validate_renderer(&pet.label, &third));
    assert!(renderer(&mut registry, &pet).is_live());
}

#[test]
fn invalid_or_empty_configuration_is_atomic_and_does_not_renew_a_valid_lease() {
    let mut registry = Registry::default();
    let (_, pet) = committed(&mut registry);
    let lease = renderer(&mut registry, &pet);
    for scale in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.0, 0.49, 1.51] {
        assert!(registry
            .configure(
                &pet.identity(),
                PetConfigPatch {
                    scale: Some(scale),
                    visible: Some(false),
                    ..Default::default()
                }
            )
            .is_err());
        assert_eq!(registry.get(&pet.identity()), Some(pet.clone()));
        assert!(registry.validate_renderer(&pet.label, &lease));
    }
    assert!(registry
        .configure(&pet.identity(), PetConfigPatch::default())
        .is_err());
    assert!(registry
        .configure(
            &pet.identity(),
            PetConfigPatch {
                skin_choice: Some(ActiveSkin::Png("../model".into())),
                ..Default::default()
            }
        )
        .is_err());
    registry
        .configure(
            &pet.identity(),
            PetConfigPatch {
                scale: Some(1.0),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(registry.validate_renderer(&pet.label, &lease));
}

#[test]
fn physical_positions_are_session_owned_clamped_for_negative_monitors_and_checked_without_overflow()
{
    let mut registry = Registry::default();
    let (_, pet) = committed(&mut registry);
    assert_eq!(pet.position, None);
    let moved = registry
        .set_position(
            &pet.identity(),
            PhysicalPoint {
                x: i32::MAX,
                y: i32::MIN,
            },
            bounds(),
        )
        .unwrap();
    assert_eq!(moved.position, Some(PhysicalPoint { x: -280, y: -100 }));
    let previous = moved.clone();
    for invalid in [
        PlacementBounds {
            window_size: WindowSize {
                width: 0,
                height: 340,
            },
            ..bounds()
        },
        PlacementBounds {
            window_size: WindowSize {
                width: 3000,
                height: 340,
            },
            ..bounds()
        },
        PlacementBounds {
            work_area: WorkArea {
                x: i32::MAX,
                y: 0,
                width: 10,
                height: 500,
            },
            window_size: WindowSize {
                width: 1,
                height: 1,
            },
        },
    ] {
        assert!(registry
            .set_position(&pet.identity(), PhysicalPoint { x: 0, y: 0 }, invalid)
            .is_err());
        assert_eq!(registry.get(&pet.identity()), Some(previous.clone()));
    }
    registry.remove(&pet.identity()).unwrap();
    let (_, next) = committed(&mut registry);
    assert_eq!(next.position, None);
}

#[test]
fn native_cancel_remove_window_revoke_clear_and_drop_invalidate_every_old_clone() {
    let mut registry = Registry::default();
    let (reservation, pet) = committed(&mut registry);
    let lease = renderer(&mut registry, &pet);
    reservation.cancel();
    assert!(!lease.is_live());
    assert!(registry.get_by_label(&pet.label).is_none());
    let (next_reservation, next) = committed(&mut registry);
    let next_lease = renderer(&mut registry, &next);
    reservation.cancel();
    assert!(registry.validate_renderer(&next.label, &next_lease));
    assert!(registry
        .revoke_window(&next.label, &next.instance_epoch)
        .is_some());
    assert!(!next_lease.is_live());
    assert!(!next_reservation.is_live());
    let (last_reservation, last) = committed(&mut registry);
    let last_lease = renderer(&mut registry, &last);
    registry.clear();
    assert!(!last_reservation.is_live());
    assert!(!last_lease.is_live());
    let (drop_reservation, drop_pet) = committed(&mut registry);
    let drop_lease = renderer(&mut registry, &drop_pet);
    drop(registry);
    assert!(!drop_reservation.is_live());
    assert!(!drop_lease.is_live());
    let mut pending_registry = Registry::default();
    let pending = pending_registry.reserve(PetConfig::default()).unwrap();
    drop(pending_registry);
    assert!(!pending.is_live());
}

#[test]
fn public_wire_description_has_no_notification_pointer_motion_or_reservation_authority() {
    let mut registry = Registry::default();
    let (_, pet) = committed(&mut registry);
    let value = serde_json::to_value(&pet).unwrap();
    assert_eq!(value["skinChoice"], json!({"kind":"builtin"}));
    assert!(value.get("token").is_none());
    assert!(value.get("generation").is_none());
    for permission in [
        "notificationAck",
        "pointer",
        "primaryMotion",
        "popup",
        "collector",
        "dataDirectory",
    ] {
        assert!(value.get(permission).is_none());
    }
    let lease = renderer(&mut registry, &pet);
    let serialized = serde_json::to_value(&lease).unwrap();
    assert!(serialized["generation"].is_string());
    assert!(serialized.get("cancelled").is_none());
    assert!(serialized.get("instanceCancelled").is_none());
}

#[test]
fn config_payloads_reject_paths_unknown_fields_nulls_and_wrong_types() {
    for value in [
        json!({"skinChoice":{"kind":"builtin"},"visible":true,"scale":1,"path":"C:/model"}),
        json!({"skinChoice":{"kind":"builtin","id":"a".repeat(64)},"visible":true,"scale":1}),
        json!({"skinChoice":{"kind":"png","id":"A".repeat(64)},"visible":true,"scale":1}),
        json!({"skinChoice":{"kind":"vrm","id":"../file"},"visible":true,"scale":1}),
    ] {
        assert!(serde_json::from_value::<PetConfig>(value).is_err());
    }
    for value in [
        json!({"visible":null}),
        json!({"scale":null}),
        json!({"skinChoice":null}),
        json!({"visible":"true"}),
        json!({"scale":"1"}),
        json!({"position":{"x":0,"y":0}}),
        json!({"windowLabel":"pet"}),
    ] {
        assert!(serde_json::from_value::<PetConfigPatch>(value).is_err());
    }
    let config: PetConfig = serde_json::from_value(
        json!({"skinChoice":{"kind":"png","id":"a".repeat(64)},"visible":true,"scale":1.5}),
    )
    .unwrap();
    assert_eq!(config.skin_choice, ActiveSkin::Png("a".repeat(64)));
    assert!(config.validate().is_ok());
}

#[test]
fn lifecycle_generation_exhaustion_never_wraps_or_preserves_an_old_renderer_lease() {
    let mut registry = Registry::default();
    let (_, pet) = committed(&mut registry);
    let lease = renderer(&mut registry, &pet);
    registry.generation = u64::MAX;
    assert!(registry
        .register_renderer(
            &pet.label,
            &pet.identity(),
            &uuid::Uuid::new_v4().to_string()
        )
        .is_err());
    assert!(!lease.is_live());
    registry.remove(&pet.identity()).unwrap();
    assert!(registry.reserve(PetConfig::default()).is_err());
    assert!(registry.list().is_empty());
}
