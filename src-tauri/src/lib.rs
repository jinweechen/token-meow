#[cfg(all(feature = "desktop", feature = "headless-audit"))]
compile_error!("desktop and headless-audit are mutually exclusive; use --no-default-features --features headless-audit for the offline CLI");
#[cfg(not(any(feature = "desktop", feature = "headless-audit")))]
compile_error!("select desktop or headless-audit; the offline CLI requires --no-default-features --features headless-audit");

mod account_bridge;
pub(crate) mod budget_clock;
pub(crate) mod notification_gate;
mod notification_registration;
mod core;
mod update_policy;
#[cfg(all(feature = "subscription-native-audit", not(debug_assertions)))]
compile_error!("subscription-native-audit is only available in a debug audit build");
#[cfg(all(debug_assertions, feature = "subscription-native-audit"))]
mod subscription_native_audit;
#[cfg(feature = "desktop")]
mod app_update;
#[cfg(feature = "desktop")]
mod desktop;
#[cfg(feature = "desktop")]
mod pet_pointer_policy;
#[cfg(feature = "desktop")]
mod pet_pointer_region;
#[cfg(feature = "desktop")]
mod local_pet_skin;
#[cfg(feature = "desktop")]
mod vrm_skin;
#[cfg(feature = "desktop")]
mod pet_skin_selection;
#[cfg(feature = "desktop")]
mod pet_registry;
#[cfg(feature = "desktop")]
mod desktop_surfaces;
#[cfg(feature = "desktop")]
mod desktop_physics;

// Native audit harnesses use the production ledger without widening the IPC surface.
pub use crate::core::Core;

#[cfg(feature = "desktop")]
pub fn run() {
    desktop::run();
}
