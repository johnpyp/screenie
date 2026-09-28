#!/usr/bin/env bash
# PipeWire's Rust bindings don't compile against PipeWire 0.3.x's headers (Ubuntu 22.04
# has 0.3.48, where the release archives are built): some of SPA's helpers were still
# macros, and structs have since changed. So where the system's PipeWire is that old,
# they compile against Ubuntu 24.04's headers (PipeWire 1.0.5) and the binary still links
# against the system's library, which fails if it uses anything 0.3.48 lacks: it runs
# wherever that runs.
#
#   pipewire-headers.sh DIR
#
# Unpacks the headers into DIR and points pkg-config at them (in $GITHUB_ENV, or prints
# the assignment). Does nothing where the system's PipeWire is 1.0 or newer.
set -euo pipefail

dir=${1:?usage: pipewire-headers.sh DIR}
[[ $(pkg-config --modversion libpipewire-0.3) == 0.* ]] || exit 0

# Launchpad keeps every published package. Headers are the same for every architecture.
base=https://launchpad.net/ubuntu/+archive/primary/+files
debs=(
  "libpipewire-0.3-dev_1.0.5-1_amd64.deb c0b34ed6a34391a94c0711966dbea9b6b5a98758d88813824c05d6662e0b60c9"
  "libspa-0.2-dev_1.0.5-1_amd64.deb 6c99d162e23e13cacf8bc3edc312c60545281078398e52e34dc5ebb4209649dc"
)
mkdir -p "$dir/pkgconfig"
for entry in "${debs[@]}"; do
  read -r deb sum <<<"$entry"
  curl -fsSL --retry 3 -o "$dir/$deb" "$base/$deb"
  echo "$sum  $dir/$deb" | sha256sum --check --quiet
  dpkg-deb -x "$dir/$deb" "$dir/root"
done
# The system's pkg-config files, but for the headers: the libraries stay the system's.
for pc in libpipewire-0.3 libspa-0.2; do
  sed "s|^includedir=.*|includedir=$dir/root/usr/include|" \
    "$(pkg-config --variable=pcfiledir "$pc")/$pc.pc" >"$dir/pkgconfig/$pc.pc"
done
echo "PKG_CONFIG_PATH=$dir/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}" >>"${GITHUB_ENV:-/dev/stdout}"
