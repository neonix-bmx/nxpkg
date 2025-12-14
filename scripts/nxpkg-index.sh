#!/usr/bin/env bash
# Architecture-aware index.json manager for nxpkg repositories
# Requirements: jq, sha256sum
#
# Environment:
#  REPO_ROOT   default /srv/nxpkg/releases
#  BASE_URL    default http://localhost:8080
#
# Commands:
#  init
#  add <name> <version> <file.nxpkg> [--arch <arch>] [--desc <text>]
#  rm <name>
#  rm-arch <name> <arch>
#  ls [--verbose]
#  show <name>
#  set-desc <name> <desc>
#  set-latest <name> <version>
#  sign --keypair-file <path>   # writes index.json.sig (ed25519, base64)
#
# Notes:
#  - Writes atomically using a temp file + mv
#  - Uses sudo for writing REPO_ROOT paths
#  - For add: computes sha256 of REPO_ROOT/<file.nxpkg> and updates architectures[arch]
#  - arch defaults to host arch alias (x86_64/aarch64/i686/arm) if not provided

set -euo pipefail

REPO_ROOT="${REPO_ROOT:-/srv/nxpkg/releases}"
INDEX="$REPO_ROOT/index.json"
BASE_URL="${BASE_URL:-http://localhost:8080}"

require() {
  command -v "$1" >/dev/null 2>&1 || { echo "Missing required command: $1" >&2; exit 1; }
}
require jq
require sha256sum

host_arch_alias() {
  case "$(uname -m)" in
    x86_64|amd64) echo x86_64 ;;
    aarch64|arm64) echo aarch64 ;;
    i686|i386|x86) echo i686 ;;
    armv7l|armv7|armhf|arm) echo arm ;;
    *) uname -m ;;
  esac
}

init() {
  sudo mkdir -p "$REPO_ROOT"
  if [[ ! -f "$INDEX" ]]; then
    echo '{"packages":{}}' | sudo tee "$INDEX" >/dev/null
    echo "Initialized $INDEX"
  else
    echo "$INDEX already exists"
  fi
}

safe_write_json() {
  local tmp
  tmp=$(mktemp)
  cat >"$tmp"
  sudo mv "$tmp" "$INDEX"
}

add_cmd() {
  local name="$1"; shift
  local version="$1"; shift
  local file="$1"; shift
  local arch=""
  local desc=""
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --arch) arch="${2:-}"; shift 2 ;;
      --desc) desc="${2:-}"; shift 2 ;;
      *) echo "Unknown option: $1" >&2; exit 1 ;;
    esac
  done
  [[ -n "$arch" ]] || arch=$(host_arch_alias)

  local path="$REPO_ROOT/$file"
  [[ -f "$path" ]] || { echo "Package file not found: $path" >&2; exit 1; }

  local sha
  sha=$(sha256sum "$path" | awk '{print tolower($1)}')
  local url="$BASE_URL/$file"

  # Ensure index exists
  [[ -f "$INDEX" ]] || init >/dev/null

  # Build new JSON using jq
  # Ensure packages[name] exists
  local jq_script
  read -r -d '' jq_script <<'JQ'
  .packages as $p
  | .packages |= ($p // {})
JQ
  local json
  json=$(jq "$jq_script" "$INDEX")

  # Ensure architectures map exists
  read -r -d '' jq_script <<JQ
  .packages |= (. // {})
  | .packages["$name"] |= (. // {latest_version:"$version", description:"$desc"})
  | .packages["$name"].latest_version = "$version"
  | .packages["$name"].description = "$desc"
  | .packages["$name"].architectures |= (. // {})
  | .packages["$name"].architectures["$arch"] = {download_url:"$url", sha256:"$sha"}
  | .packages["$name"].download_url = "$url"
  | .packages["$name"].sha256 = "$sha"
JQ
  echo "$json" | jq "$jq_script" | safe_write_json
  echo "Updated $name ($arch) -> $url"
}

rm_cmd() {
  local name="$1"
  [[ -f "$INDEX" ]] || { echo "Index not found: $INDEX" >&2; exit 1; }
  jq 'del(.packages["'$name'"])' "$INDEX" | safe_write_json
  echo "Removed entry: $name"
}

rm_arch_cmd() {
  local name="$1"; shift
  local arch="$1"; shift
  [[ -f "$INDEX" ]] || { echo "Index not found: $INDEX" >&2; exit 1; }
  local json
  json=$(jq '.packages["'$name'"]?.architectures? |= (del(."'$arch'"))' "$INDEX")
  echo "$json" | safe_write_json
  echo "Removed arch $arch from $name"
}

ls_cmd() {
  local verbose=0
  if [[ "${1:-}" == "--verbose" ]]; then verbose=1; fi
  [[ -f "$INDEX" ]] || { echo "Index not found: $INDEX" >&2; exit 1; }
  if [[ $verbose -eq 1 ]]; then
    jq -r '.packages | to_entries[] | "\(.key): \(.value.latest_version) [" + ((.value.architectures // {}) | keys | join(",")) + "]"' "$INDEX"
  else
    jq -r '.packages | to_entries[] | "\(.key): \(.value.latest_version)"' "$INDEX"
  fi
}

show_cmd() {
  local name="$1"
  [[ -f "$INDEX" ]] || { echo "Index not found: $INDEX" >&2; exit 1; }
  jq '.packages["'$name'"]' "$INDEX"
}

set_desc_cmd() {
  local name="$1"; shift
  local desc="$*"
  [[ -f "$INDEX" ]] || { echo "Index not found: $INDEX" >&2; exit 1; }
  jq '.packages["'$name'"]?.description = "'$desc'"' "$INDEX" | safe_write_json
  echo "Updated description for $name"
}

set_latest_cmd() {
  local name="$1"; shift
  local version="$1"
  [[ -f "$INDEX" ]] || { echo "Index not found: $INDEX" >&2; exit 1; }
  jq '.packages["'$name'"]?.latest_version = "'$version'"' "$INDEX" | safe_write_json
  echo "Updated latest_version for $name -> $version"
}

sign_cmd() {
  local keypair_file=""
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --keypair-file) keypair_file="${2:-}"; shift 2 ;;
      *) echo "Unknown option: $1" >&2; exit 1 ;;
    esac
  done
  [[ -n "$keypair_file" ]] || { echo "--keypair-file is required" >&2; exit 1; }
  [[ -f "$keypair_file" ]] || { echo "Keypair not found: $keypair_file" >&2; exit 1; }

  # Use python + pynacl if available; else try openssl + external tool is not straightforward.
  command -v python3 >/dev/null 2>&1 || { echo "python3 is required for signing" >&2; exit 1; }
  python3 - "$INDEX" "$REPO_ROOT/index.json.sig" "$keypair_file" <<'PY'
import base64, sys
from nacl import signing

index_path, sig_path, kp_path = sys.argv[1:4]
kp_b64 = open(kp_path, 'r').read().strip()
kp = base64.b64decode(kp_b64)
if len(kp) != 64:
    print('Keypair must be 64 bytes (base64)', file=sys.stderr)
    sys.exit(1)
sk = signing.SigningKey(kp[:32])
data = open(index_path, 'rb').read()
sig = sk.sign(data).signature
open(sig_path, 'w').write(base64.b64encode(sig).decode())
print('Wrote', sig_path)
PY
}

usage() {
  cat <<EOF
Usage:
  REPO_ROOT=/srv/nxpkg/releases BASE_URL=http://localhost:8080 $0 init
  $0 add <name> <version> <file.nxpkg> [--arch <arch>] [--desc <text>]
  $0 rm <name>
  $0 rm-arch <name> <arch>
  $0 ls [--verbose]
  $0 show <name>
  $0 set-desc <name> <desc>
  $0 set-latest <name> <version>
  $0 sign --keypair-file <path>
EOF
}

cmd="${1:-}"; shift || true
case "$cmd" in
  init) init ;;
  add) add_cmd "${1:?name}" "${2:?version}" "${3:?file}" "${@:4}" ;;
  rm) rm_cmd "${1:?name}" ;;
  rm-arch) rm_arch_cmd "${1:?name}" "${2:?arch}" ;;
  ls) ls_cmd "${1:-}" ;;
  show) show_cmd "${1:?name}" ;;
  set-desc) shift || true; set_desc_cmd "${1:?name}" "${*:2}" ;;
  set-latest) set_latest_cmd "${1:?name}" "${2:?version}" ;;
  sign) sign_cmd "$@" ;;
  *) usage; exit 1 ;;
esac
