#!/usr/bin/env bash
# Optional Linux deployment wrapper, not a dependency on Codex in the workbench.
# Layout: runtime/{node,acp,codex}, codex-home, workspace under one private folder.
set -euo pipefail
analysis_home=$(realpath "${1:?Supply the private analysis installation folder}")
[[ -f "$analysis_home/codex-home/auth.json" ]] || { echo 'Authenticate the dedicated analysis account first.' >&2; exit 1; }
args=(--die-with-parent --new-session --unshare-user --unshare-pid --unshare-uts --unshare-ipc
      --ro-bind /usr /usr --proc /proc --dev /dev --tmpfs /tmp --dir /run)
for item in /bin /sbin /lib /lib64; do
  if [[ -L "$item" ]]; then args+=(--symlink "$(readlink "$item")" "$item");
  elif [[ -d "$item" ]]; then args+=(--ro-bind "$item" "$item"); fi
done
for item in /etc/ssl /etc/resolv.conf /etc/hosts /etc/nsswitch.conf /etc/localtime; do
  [[ ! -e "$item" ]] || args+=(--ro-bind "$item" "$item")
done
args+=(--ro-bind "$analysis_home/runtime" "$analysis_home/runtime"
       --bind "$analysis_home/codex-home" "$analysis_home/codex-home"
       --bind "$analysis_home/workspace" "$analysis_home/workspace"
       --setenv HOME "$analysis_home" --setenv CODEX_HOME "$analysis_home/codex-home"
       --setenv CODEX_PATH "$analysis_home/runtime/codex/bin/codex"
       --setenv PATH "$analysis_home/runtime/node/bin:/usr/bin:/bin"
       --setenv INITIAL_AGENT_MODE read-only --setenv NO_BROWSER 1
       --chdir "$analysis_home/workspace")
exec bwrap "${args[@]}" "$analysis_home/runtime/node/bin/node" "$analysis_home/runtime/acp/dist/index.js"
