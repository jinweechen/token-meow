#[cfg(all(feature = "desktop", feature = "headless-audit"))]
compile_error!("desktop and headless-audit are mutually exclusive; use --no-default-features --features headless-audit for the offline CLI");
#[cfg(not(any(feature = "desktop", feature = "headless-audit")))]
compile_error!("select desktop or headless-audit; the offline CLI requires --no-default-features --features headless-audit");

fn main() {
    #[cfg(feature = "desktop")]
    tauri_build::build();
}
