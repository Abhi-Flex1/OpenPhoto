#!/usr/bin/env bash
# Generates local debug signing material for the HarmonyOS shell and wires it into
# harmony/build-profile.json5.
#
# HarmonyOS requires every installable HAP to be signed. This script creates a fully local debug
# chain (root CA -> app/profile sub-CAs -> certs -> signed provisioning profile) with the SDK's
# hap-sign-tool, so no Huawei developer account is needed for emulator installs.
#
# Usage: scripts/ohos-sign.sh [--udid <device-udid>]
#
# The emulator's UDID is picked up automatically from the connected device when omitted. Re-run
# with --udid after switching emulators: the debug profile lists the devices it may install on.
#
# Outputs (never committed, see .gitignore):
#   harmony/signing/   keystore, certificates and the signed profile
#   harmony/build-profile.json5 is updated in place to point at them.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CLT="${CLT:-$HOME/Developer/command-line-tools}"
SIGN_JAR="$CLT/sdk/default/openharmony/toolchains/lib/hap-sign-tool.jar"
SIGDIR="$ROOT/harmony/signing"
HDC="$CLT/sdk/default/openharmony/toolchains/hdc"
BUNDLE="ai.storyteller.openphoto"

UDID=""
for ((i = 1; i <= $#; i++)); do
    if [ "${!i}" = "--udid" ]; then
        i=$((i + 1))
        UDID="${!i}"
    fi
done
if [ -z "$UDID" ]; then
    # First connected target's UDID.
    TARGET="$("$HDC" list targets 2>/dev/null | head -1)"
    UDID="$("$HDC" -t "$TARGET" shell bm get --udid 2>/dev/null | tail -1 | tr -d '\r\n ' || true)"
fi
if [ -z "$UDID" ]; then
    echo "no device UDID: connect an emulator (or pass --udid <udid>) and re-run" >&2
    exit 1
fi
echo "device UDID: $UDID"

export PATH="/opt/homebrew/opt/openjdk@17/bin:$PATH"
command -v java >/dev/null || { echo "java not found (need JRE 8+)" >&2; exit 1; }

mkdir -p "$SIGDIR"
cd "$SIGDIR"

# Long passwords: hvigor rejects signing passwords shorter than 32 characters. Reused across
# runs so existing keystores stay readable.
if [ -f .store-pwd ] && [ -f .key-pwd ]; then
    STORE_PWD="$(cat .store-pwd)"
    KEY_PWD="$(cat .key-pwd)"
else
    STORE_PWD="$(openssl rand -hex 32)"
    KEY_PWD="$(openssl rand -hex 32)"
    echo "$STORE_PWD" > .store-pwd
    echo "$KEY_PWD" > .key-pwd
    chmod 600 .store-pwd .key-pwd
fi

JAR=(java -jar "$SIGN_JAR")
ALG=SHA256withECDSA

# Keys and certificates are reused across runs; only the profile (which lists the device UDID)
# is regenerated every time.
if [ ! -f root-ca.cer ]; then
echo "==> root CA"
"${JAR[@]}" generate-ca -keyAlias "openphoto-root-ca" -keyPwd "$KEY_PWD" \
    -keyAlg ECC -keySize NIST-P-256 \
    -subject "C=CN,O=OpenPhoto,OU=OpenPhoto Local Debug,CN=OpenPhoto Local Root CA" \
    -signAlg "$ALG" -keystoreFile root.p12 -keystorePwd "$STORE_PWD" -outFile root-ca.cer
fi

if [ ! -f sub-app-ca.cer ]; then
echo "==> app sub-CA"
"${JAR[@]}" generate-ca -keyAlias "openphoto-app-ca" -keyPwd "$KEY_PWD" \
    -keyAlg ECC -keySize NIST-P-256 \
    -issuer "C=CN,O=OpenPhoto,OU=OpenPhoto Local Debug,CN=OpenPhoto Local Root CA" \
    -issuerKeyAlias "openphoto-root-ca" -issuerKeyPwd "$KEY_PWD" \
    -subject "C=CN,O=OpenPhoto,OU=OpenPhoto Local Debug,CN=OpenPhoto Application CA" \
    -signAlg "$ALG" -keystoreFile root.p12 -keystorePwd "$STORE_PWD" -outFile sub-app-ca.cer
fi

if [ ! -f sub-profile-ca.cer ]; then
echo "==> profile sub-CA"
"${JAR[@]}" generate-ca -keyAlias "openphoto-profile-ca" -keyPwd "$KEY_PWD" \
    -keyAlg ECC -keySize NIST-P-256 \
    -issuer "C=CN,O=OpenPhoto,OU=OpenPhoto Local Debug,CN=OpenPhoto Local Root CA" \
    -issuerKeyAlias "openphoto-root-ca" -issuerKeyPwd "$KEY_PWD" \
    -subject "C=CN,O=OpenPhoto,OU=OpenPhoto Local Debug,CN=OpenPhoto Profile CA" \
    -signAlg "$ALG" -keystoreFile root.p12 -keystorePwd "$STORE_PWD" -outFile sub-profile-ca.cer
fi

if [ ! -f app-chain.pem ]; then
echo "==> app key + certificate chain"
"${JAR[@]}" generate-keypair -keyAlias "openphoto-app-key" -keyPwd "$KEY_PWD" \
    -keyAlg ECC -keySize NIST-P-256 -keystoreFile app.p12 -keystorePwd "$STORE_PWD"
"${JAR[@]}" generate-app-cert -keyAlias "openphoto-app-key" -keyPwd "$KEY_PWD" \
    -issuer "C=CN,O=OpenPhoto,OU=OpenPhoto Local Debug,CN=OpenPhoto Application CA" \
    -issuerKeyAlias "openphoto-app-ca" -issuerKeyPwd "$KEY_PWD" \
    -subject "C=CN,O=OpenPhoto,OU=OpenPhoto Local Debug,CN=OpenPhoto Application" \
    -signAlg "$ALG" -keystoreFile app.p12 -keystorePwd "$STORE_PWD" \
    -issuerKeystoreFile root.p12 -issuerKeystorePwd "$STORE_PWD" \
    -outForm certChain -rootCaCertFile root-ca.cer -subCaCertFile sub-app-ca.cer -outFile app-chain.pem
fi

if [ ! -f profile-chain.pem ]; then
echo "==> profile key + certificate chain"
"${JAR[@]}" generate-keypair -keyAlias "openphoto-profile-key" -keyPwd "$KEY_PWD" \
    -keyAlg ECC -keySize NIST-P-256 -keystoreFile profile.p12 -keystorePwd "$STORE_PWD"
"${JAR[@]}" generate-profile-cert -keyAlias "openphoto-profile-key" -keyPwd "$KEY_PWD" \
    -issuer "C=CN,O=OpenPhoto,OU=OpenPhoto Local Debug,CN=OpenPhoto Profile CA" \
    -issuerKeyAlias "openphoto-profile-ca" -issuerKeyPwd "$KEY_PWD" \
    -subject "C=CN,O=OpenPhoto,OU=OpenPhoto Local Debug,CN=OpenPhoto Profile Debug" \
    -signAlg "$ALG" -keystoreFile profile.p12 -keystorePwd "$STORE_PWD" \
    -issuerKeystoreFile root.p12 -issuerKeystorePwd "$STORE_PWD" \
    -outForm certChain -rootCaCertFile root-ca.cer -subCaCertFile sub-profile-ca.cer -outFile profile-chain.pem
fi

NOW="$(date +%s)"
LATER="$((NOW + 10 * 365 * 86400))"
UUID="$(uuidgen | tr '[:upper:]' '[:lower:]')"
# sign-profile requires the app (leaf) certificate inside bundle-info: first block of the chain.
LEAF_CERT="$(awk '/BEGIN CERTIFICATE/{p=1} p{print} /END CERTIFICATE/{if(p) exit}' app-chain.pem)"
python3 - "$SIGDIR/profile-unsigned.json" "$BUNDLE" "$UDID" "$NOW" "$LATER" "$UUID" "$LEAF_CERT" <<'EOF'
import json, sys
path, bundle, udid, not_before, not_after, uuid, leaf = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4]), int(sys.argv[5]), sys.argv[6], sys.argv[7]
profile = {
    "version-name": "1.0.0",
    "version-code": 1,
    "uuid": uuid,
    "validity": {"not-before": not_before, "not-after": not_after},
    "type": "debug",
    "bundle-info": {
        "developer-id": "OpenPhoto",
        "development-certificate": leaf + "\n",
        "bundle-name": bundle,
        "apl": "normal",
        "app-feature": "hos_normal_app",
    },
    "acls": {"allowed-acls": [""]},
    "permissions": {"restricted-permissions": [""]},
    "debug-info": {"device-ids": [udid], "device-id-type": "udid"},
}
with open(path, "w") as f:
    json.dump(profile, f, indent=4)
print(f"profile for {bundle} on {udid}")
EOF

echo "==> sign profile"
"${JAR[@]}" sign-profile -mode localSign -keyAlias "openphoto-profile-key" -keyPwd "$KEY_PWD" \
    -profileCertFile profile-chain.pem -inFile profile-unsigned.json -signAlg "$ALG" \
    -keystoreFile profile.p12 -keystorePwd "$STORE_PWD" -outFile profile-debug.p7b

echo "==> verify profile"
"${JAR[@]}" verify-profile -inFile profile-debug.p7b -outFile profile-verified.json
python3 -c "import json; d = json.load(open('$SIGDIR/profile-verified.json')); print(json.dumps(d)[:200])"
echo "done: material in $SIGDIR (used by scripts/ohos-build.sh to sign the HAP)"
