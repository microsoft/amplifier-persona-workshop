#!/usr/bin/env bash
set -euo pipefail
umask 077
repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
instance_home="${XDG_DATA_HOME:-$HOME/.local/share}/twin-workbench/default"
bind=127.0.0.1
port=0
public_url=
extractor_config=
open_browser=1
while (($#)); do
  case "$1" in
    --home) instance_home=$2; shift 2;;
    --bind) bind=$2; shift 2;;
    --port) port=$2; shift 2;;
    --public-url) public_url=$2; shift 2;;
    --extractor-config) extractor_config=$2; shift 2;;
    --no-browser) open_browser=0; shift;;
    *) echo "Unknown option: $1" >&2; exit 2;;
  esac
done
[[ $(uname -s) == Linux ]] || { echo 'This first installer supports Linux with PAM and systemd.' >&2; exit 1; }
if ! command -v cc >/dev/null || ! pkg-config --exists pam 2>/dev/null; then
  command -v apt-get >/dev/null || { echo 'Install a C compiler, pkg-config and PAM development headers, then retry.' >&2; exit 1; }
  sudo apt-get update
  sudo apt-get install -y build-essential pkg-config libpam0g-dev ca-certificates curl
fi
if ! command -v python3 >/dev/null || ! command -v pdftotext >/dev/null; then
  command -v apt-get >/dev/null || { echo 'Install Python 3 and poppler-utils for document imports, then retry.' >&2; exit 1; }
  sudo apt-get update
  sudo apt-get install -y python3 poppler-utils
fi
if ! command -v cargo >/dev/null; then
  rustup_script=$(mktemp)
  trap 'rm -f "$rustup_script"' EXIT
  curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs -o "$rustup_script"
  sh "$rustup_script" -y --profile minimal
  source "$HOME/.cargo/env"
fi
cargo build --locked --release --manifest-path "$repo_dir/Cargo.toml"
mkdir -p "$instance_home"
instance_home=$(realpath "$instance_home")
chmod 700 "$instance_home"
mkdir -p "$instance_home/bin"
install -m 700 "$repo_dir/target/release/twin-workbench" "$instance_home/bin/twin-workbench.next"
mv "$instance_home/bin/twin-workbench.next" "$instance_home/bin/twin-workbench"
if [[ ! -f "$instance_home/config.json" ]]; then
  args=(--home "$instance_home" init --owner "$(id -un)" --bind "$bind" --port "$port" --public-url "$public_url")
  [[ -z "$extractor_config" ]] || args+=(--extractor-config "$extractor_config")
  "$instance_home/bin/twin-workbench" "${args[@]}"
fi
if [[ ! -f /etc/pam.d/twin-workbench ]]; then
  sudo install -m 644 "$repo_dir/ops/pam-twin-workbench" /etc/pam.d/twin-workbench
fi
instance_id=$(python3 - "$instance_home/config.json" <<'PY'
import json,sys
print(json.load(open(sys.argv[1]))['id'])
PY
)
service_name="twin-workbench-$instance_id.service"
mkdir -p "$HOME/.config/systemd/user"
python3 - "$instance_home" "$HOME/.config/systemd/user/$service_name" <<'PY'
from pathlib import Path
import sys
home,out=map(Path,sys.argv[1:])
def quote(p):
    return '"'+str(p).replace('\\','\\\\').replace('"','\\"').replace('%','%%')+'"'
Path(out).write_text('[Unit]\nDescription=Persona Workshop\nAfter=network-online.target\n\n[Service]\nType=simple\nExecStart='+quote(home/'bin/twin-workbench')+' --home '+quote(home)+' serve\nRestart=on-failure\nRestartSec=3\nUMask=0077\n\n[Install]\nWantedBy=default.target\n')
PY
systemctl --user daemon-reload
systemctl --user enable --now "$service_name"
rm -f "$instance_home/runtime.json"
systemctl --user restart "$service_name"
for n in $(seq 1 40); do
  if [[ -s "$instance_home/runtime.json" ]]; then
    local_url=$(python3 - "$instance_home/runtime.json" <<'PY'
import json,sys
print(json.load(open(sys.argv[1]))['url'])
PY
)
    if curl --cacert "$instance_home/tls/root-ca.crt" --max-time 2 -fsS "$local_url/health" >/dev/null 2>&1; then break; fi
  fi
  sleep 0.25
done
[[ -n "${local_url:-}" ]] && curl --cacert "$instance_home/tls/root-ca.crt" --max-time 2 -fsS "$local_url/health" >/dev/null || {
  echo "Service did not become ready. Inspect: journalctl --user -u $service_name" >&2; exit 1;
}
url=$(python3 - "$instance_home/runtime.json" <<'PY'
import json,sys
r=json.load(open(sys.argv[1]));print(r.get('public_url') or 'https://127.0.0.1:'+str(r['port']))
PY
)
printf 'Persona Workshop: %s\nService: %s\nInstance: %s\n' "$url" "$service_name" "$instance_home"
printf 'Sign in with this machine’s owner account. The local HTTPS certificate needs trust; a configured trusted HTTPS proxy avoids that warning.\n'
if [[ "$open_browser" == 1 ]] && command -v xdg-open >/dev/null && [[ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]]; then
  xdg-open "$url" >/dev/null 2>&1 || true
fi
