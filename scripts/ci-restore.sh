#!/bin/sh
# Restore packed prep tools. clippy/rustfmt ELFs have RUNPATH $ORIGIN/../lib
# (librustc_driver), so they must live in $(rustc --print sysroot)/bin.
# Do not copy those ELFs over rustup shims in $CARGO_HOME/bin.
set -eu

local_bin="${CI_RESTORE_LOCAL_BIN:-/usr/local/bin}"
mkdir -p "$local_bin"

if [ ! -d .ci-home/bin ]; then
  echo "missing .ci-home/bin" >&2
  exit 1
fi

cp .ci-home/bin/gitleaks "$local_bin/"

if [ -x .ci-home/bin/cargo-audit ]; then
  cp .ci-home/bin/cargo-audit "$local_bin/"
  cargo_bin="${CARGO_HOME:-/usr/local/cargo}/bin"
  if [ -d "$cargo_bin" ]; then
    cp .ci-home/bin/cargo-audit "$cargo_bin/"
  fi
fi

if command -v rustc >/dev/null 2>&1; then
  sysroot="$(rustc --print sysroot)"
  mkdir -p "$sysroot/bin"
  for name in cargo-clippy clippy-driver cargo-fmt rustfmt; do
    cp ".ci-home/bin/$name" "$sysroot/bin/"
    # PATH fallback that does not overwrite rustup shims in cargo/bin.
    # exec so $ORIGIN is sysroot/bin and librustc_driver resolves.
    printf '#!/bin/sh\nexec "%s/bin/%s" "$@"\n' "$sysroot" "$name" > "$local_bin/$name"
    chmod +x "$local_bin/$name"
  done
  if [ -n "${GITHUB_PATH:-}" ]; then
    echo "$sysroot/bin" >> "$GITHUB_PATH"
  fi
fi
