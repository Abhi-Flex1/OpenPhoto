#!/usr/bin/env bash
# Builds OpenPhoto for HarmonyOS and assembles the installable HAP.
#
# Usage: scripts/ohos-build.sh [--debug] [--skip-rust] [--skip-hap]
#
#   --debug      debug Rust module and debug HAP (default: release Rust + release HAP)
#   --skip-rust  reuse the .so already in harmony/entry/libs (ArkTS-only change)
#   --skip-hap   stop after the .so (no ohpm/hvigor packaging)
#
# Prerequisites (once):
#   rustup target add aarch64-unknown-linux-ohos
#   scripts/ohos-sign.sh (local debug signing material; needs a connected emulator for its UDID)
#   HarmonyOS command-line tools (hvigor, ohpm, SDK, emulator) at ~/Developer/command-line-tools.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CLT="${CLT:-$HOME/Developer/command-line-tools}"
SDK_NATIVE="$CLT/sdk/default/openharmony/native"
TARGET=aarch64-unknown-linux-ohos
LIBS="$ROOT/harmony/entry/libs/arm64-v8a"
SIGDIR="$ROOT/harmony/signing"

PROFILE=release
HAP_MODE=release
SKIP_RUST=0
SKIP_HAP=0
for arg in "$@"; do
    case "$arg" in
        --debug) PROFILE=debug; HAP_MODE=debug ;;
        --skip-rust) SKIP_RUST=1 ;;
        --skip-hap) SKIP_HAP=1 ;;
        *) echo "unknown flag: $arg" >&2; exit 1 ;;
    esac
done

export OHOS_SDK_NATIVE="$SDK_NATIVE"
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_OHOS_LINKER="$SDK_NATIVE/llvm/bin/aarch64-unknown-linux-ohos-clang"
export PATH="$HOME/.cargo/bin:$PATH"
# Keep OHOS artifacts out of the shared target dir (and its Cargo build lock).
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target/ohos}"

if [ "$SKIP_RUST" -eq 0 ]; then
    echo "==> cargo build -p openphoto-ohos --target $TARGET ($PROFILE)"
    # shellcheck disable=SC2086
    (cd "$ROOT" && cargo build --$PROFILE -p openphoto-ohos --target "$TARGET")
    mkdir -p "$LIBS"
    cp -f "$CARGO_TARGET_DIR/$TARGET/$PROFILE/libopenphoto_native.so" "$LIBS/libopenphoto_native.so"
    "$SDK_NATIVE/llvm/bin/llvm-strip" "$LIBS/libopenphoto_native.so"
    echo "==> staged: $LIBS/libopenphoto_native.so ($(du -h "$LIBS/libopenphoto_native.so" | cut -f1))"
fi

if [ "$SKIP_HAP" -eq 1 ]; then
    exit 0
fi

export DEVECO_SDK_HOME="$CLT/sdk"
export PATH="$CLT/tool/node/bin:/opt/homebrew/opt/openjdk@17/bin:$PATH"

echo "==> ohpm install"
(cd "$ROOT/harmony" && "$CLT/bin/ohpm" install)

echo "==> hvigor assembleHap (unsigned, $HAP_MODE)"
(cd "$ROOT/harmony" && "$CLT/bin/hvigorw" --mode module -p module=entry@default -p product=default -p buildMode="$HAP_MODE" assembleHap --no-daemon)
UNSIGNED="$(ls -t "$ROOT/harmony/entry/build/default/outputs/default/"*.hap | head -1)"
echo "==> unsigned: $UNSIGNED"

if [ ! -f "$SIGDIR/profile-debug.p7b" ]; then
    echo "no signing material: run scripts/ohos-sign.sh first (needs a connected emulator)" >&2
    exit 1
fi
KEY_PWD="$(cat "$SIGDIR/.key-pwd")"
STORE_PWD="$(cat "$SIGDIR/.store-pwd")"
SIGNED="${UNSIGNED%-unsigned.hap}-signed.hap"
echo "==> hap-sign-tool sign-app"
java -jar "$CLT/sdk/default/openharmony/toolchains/lib/hap-sign-tool.jar" sign-app \
    -mode localSign -keyAlias "openphoto-app-key" -keyPwd "$KEY_PWD" \
    -appCertFile "$SIGDIR/app-chain.pem" -profileFile "$SIGDIR/profile-debug.p7b" \
    -inFile "$UNSIGNED" -keystoreFile "$SIGDIR/app.p12" -keystorePwd "$STORE_PWD" \
    -outFile "$SIGNED" -signAlg SHA256withECDSA -signCode 1
echo "==> signed: $SIGNED"
