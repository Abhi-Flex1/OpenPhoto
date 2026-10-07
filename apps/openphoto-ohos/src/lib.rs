//! OpenPhoto's native HarmonyOS host.
//!
//! HarmonyOS runs this crate as a N-API native module (`libopenphoto_native.so`) loaded by an
//! ArkUI `XComponent`. The editor itself is *not* ported or re-implemented: this crate is the
//! platform shell that feeds the existing [`openphoto_ui_egui::OpenPhotoApp`] the HarmonyOS
//! window, surface and input events, exactly like `eframe` does on desktop.
//!
//! ```text
//! ArkTS EntryAbility  ──▶  XComponent surface (OH_NativeWindow)
//!        │  napi module init
//!        ▼
//!   #[ability] ──▶ winit-ohos event loop ──▶ egui::Context::run_ui ──▶ egui-wgpu ──▶ wgpu surface
//! ```
#![cfg(any(target_env = "ohos", test))]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
// Host builds only run the input-mapping unit tests; the ohos-only modules below are empty there,
// so their items look unused. The OHOS build (the real one) has no such allowance.
#![cfg_attr(not(target_env = "ohos"), allow(dead_code))]

mod bridge;
#[cfg(target_env = "ohos")]
mod gpu;
#[cfg(target_env = "ohos")]
mod host;
mod input;
#[cfg(target_env = "ohos")]
mod logging;
#[cfg(target_env = "ohos")]
mod services;

#[cfg(target_env = "ohos")]
use openharmony_ability::OpenHarmonyApp;
#[cfg(target_env = "ohos")]
use openharmony_ability_derive::ability;
#[cfg(target_env = "ohos")]
use openharmony_ability_plugin_files::FilesBridgePlugin;
#[cfg(target_env = "ohos")]
use winit_ohos::{EventLoop, PlatformSpecificEventLoopAttributes};

/// `hilog` tag for every record (visible with `hdc hilog -T <tag>`).
#[cfg(target_env = "ohos")]
pub(crate) const LOG_TAG: &str = "OpenPhoto";

/// HarmonyOS entry point.
///
/// The macro registers this as the native module's Ability callback; it must return quickly, so
/// the winit event loop is registered (not run) here and HarmonyOS keeps driving frames.
#[cfg(target_env = "ohos")]
#[ability]
fn openphoto_ability(app: OpenHarmonyApp) {
    if let Err(error) = logging::install() {
        // Logging is best-effort: a missing hilog must never keep the editor from starting.
        writeln_hilog(&format!("hilog unavailable: {error}"));
    }
    // System file dialogs (open/save picker) need the Rust facade registered before first use;
    // without it the corresponding menu items behave as cancelled, as on dialog-less builds.
    if let Err(error) = app.register_plugin(FilesBridgePlugin) {
        log::warn!("file dialogs unavailable: {error}");
    }
    let attributes = PlatformSpecificEventLoopAttributes { openharmony_app: Some(app.clone()) };
    let event_loop = match EventLoop::new(&attributes) {
        Ok(event_loop) => event_loop,
        Err(error) => {
            writeln_hilog(&format!("cannot create the OHOS event loop: {error}"));
            return;
        }
    };
    if let Err(error) = event_loop.run_app(host::OpenPhoto::new(app)) {
        writeln_hilog(&format!("cannot register the OHOS event loop: {error}"));
    }
}

/// Last-resort logging for failures that happen before (or instead of) the `log` facade.
#[cfg(target_env = "ohos")]
fn writeln_hilog(message: &str) {
    ohos_hilog_binding::error(format!("{LOG_TAG}: {message}"));
}
