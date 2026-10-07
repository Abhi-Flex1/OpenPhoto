import { DefaultXComponent, LazyPlugin, NativeAbility } from "@ohos-rs/ability";
import { FilesPlugin } from "@ohos-rs/ability-plugin-files";
import window from "@ohos.window";

/// The Rust native module built by the `openphoto-ohos` crate (`libopenphoto_native.so`,
/// shipped in `entry/libs/arm64-v8a/`). One module owns the single editor window.
const NATIVE_MODULE = "openphoto_native";

export default class EntryAbility extends NativeAbility {
  public moduleName: string = NATIVE_MODULE;

  // System file dialogs (open/save picker) used by File › Open / Save As. The Rust side drives
  // them through the matching `FilesBridgePlugin` facade; see `apps/openphoto-ohos/src/services.rs`.
  public bridgePlugins = [new LazyPlugin(() => new FilesPlugin())];

  protected override async loadWindowStageContent(windowStage: window.WindowStage): Promise<void> {
    try {
      const mainWindow = windowStage.getMainWindowSync();
      // The editor draws its own title bar and chrome, edge to edge.
      await mainWindow.setWindowLayoutFullScreen(true);
      await windowStage.loadContent("pages/Index");
    } catch (error) {
      throw new Error(`Unable to load OpenPhoto content: ${String(error)}`);
    }
  }
}
