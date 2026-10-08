#!/usr/bin/env bash
# Installs the Tauri/GTK build dependencies on Ubuntu runners.
#
# The Azure Ubuntu mirror sometimes stalls for minutes or serves a package
# index that 404s on .debs. Each apt call is time-boxed and the whole
# update + install is retried once, so a bad mirror day costs a few minutes
# instead of 14+, and a stale index is refreshed before the retry.
set -uo pipefail

packages=(
  libwebkit2gtk-4.1-dev
  libssl-dev
  libdbus-1-dev
  libasound2-dev
  pkg-config
  libayatana-appindicator3-dev
  libgtk-3-dev
)
apt_opts=(-o Acquire::Retries=3 -o Acquire::http::Timeout=30 -o Acquire::https::Timeout=30)

install() {
  sudo timeout 90 apt-get "${apt_opts[@]}" update &&
    sudo timeout 240 apt-get "${apt_opts[@]}" install -y "${packages[@]}"
}

for attempt in 1 2; do
  if install; then
    exit 0
  fi
  echo "::warning::apt install attempt ${attempt} failed"
  [ "$attempt" = 1 ] && sleep 15
done

echo "::error::apt install failed after 2 attempts"
exit 1
