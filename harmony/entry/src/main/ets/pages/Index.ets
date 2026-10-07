import { DefaultXComponent } from "@ohos-rs/ability";

/// The whole window is the editor: one full-bleed XComponent whose surface the Rust host renders
/// into with wgpu (Vulkan on devices, OpenGL ES on the emulator).
@Entry
@Component
struct Index {
  build() {
    Column() {
      DefaultXComponent({ moduleName: "openphoto_native" })
        .width("100%")
        .height("100%")
    }
    .width("100%")
    .height("100%")
  }
}
