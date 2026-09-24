#!/bin/sh
# One-time, macOS only: create a self-signed code-signing identity for dev
# builds. With a stable signature, macOS remembers "Always Allow" for the
# app's keychain entries across rebuilds instead of asking after each one.
# The identity lives only in this Mac's login keychain.
set -eu
NAME="Crate Digger Dev"
if security find-certificate -c "$NAME" >/dev/null 2>&1; then
  echo "$NAME already exists."
  exit 0
fi
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
  -keyout "$dir/key.pem" -out "$dir/cert.pem" -subj "/CN=$NAME" \
  -addext "keyUsage=critical,digitalSignature" -addext "extendedKeyUsage=critical,codeSigning" 2>/dev/null
pass=$(uuidgen)
openssl pkcs12 -export -legacy -out "$dir/id.p12" -inkey "$dir/key.pem" -in "$dir/cert.pem" -passout "pass:$pass" 2>/dev/null \
  || openssl pkcs12 -export -out "$dir/id.p12" -inkey "$dir/key.pem" -in "$dir/cert.pem" -passout "pass:$pass"
security import "$dir/id.p12" -k "$HOME/Library/Keychains/login.keychain-db" -P "$pass" -T /usr/bin/codesign
echo "Created $NAME. Dev builds are signed with it from now on."
