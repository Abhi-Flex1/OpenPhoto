//! Registers the HarmonyOS N-API module metadata for `libopenphoto_native.so`.
//!
//! `napi-build-ohos` emits the module registration the ArkTS `import … from "lib*.so"` needs. It is
//! a no-op on other hosts, where the crate is empty anyway.
fn main() {
    #[cfg(target_env = "ohos")]
    napi_build_ohos::setup();
}
