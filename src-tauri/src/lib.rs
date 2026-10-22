#[cfg(all(feature = "desktop", feature = "headless-audit"))]
compile_error!("desktop and headless-audit are mutually exclusive; use --no-default-features --features headless-audit for the offline CLI");
#[cfg(not(any(feature = "desktop", feature = "headless-audit")))]
compile_error!("select desktop or headless-audit; the offline CLI requires --no-default-features --features headless-audit");

mod account_bridge;
pub(crate) mod budget_clock;
pub(crate) mod notification_gate;
mod core;
mod update_policy;
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

// Native audit harnesses use the production ledger without widening the IPC surface.
pub use crate::core::Core;

#[cfg(feature = "desktop")]
pub fn run() {
    desktop::run();
}
