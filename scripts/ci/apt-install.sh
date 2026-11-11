#!/usr/bin/env bash
# Installs apt packages on a hosted Linux runner: `apt-install.sh <package>...`.
#
# The runner image points apt at `azure.archive.ubuntu.com` through
# `/etc/apt/apt-mirrors.txt`. When that name stops resolving, every download
# waits out its DNS timeout and the step fails after about twenty minutes. This
# script bounds the wait and falls back to `archive.ubuntu.com`: up front when
# the Azure name does not resolve, and once more if the install fails anyway.
set -euo pipefail

if [ "$#" -eq 0 ]; then
    echo "usage: apt-install.sh <package>..." >&2
    exit 2
fi

mirrors=/etc/apt/apt-mirrors.txt
opts=(
    -o Acquire::Retries=3
    -o Acquire::http::Timeout=30
    -o Acquire::https::Timeout=30
)

use_main_archive() {
    if [ -f "$mirrors" ]; then
        echo "apt-install: pointing apt at archive.ubuntu.com"
        printf 'http://archive.ubuntu.com/ubuntu/\tpriority:1\n' | sudo tee "$mirrors" >/dev/null
    fi
}

install() {
    sudo apt-get "${opts[@]}" update &&
        sudo apt-get "${opts[@]}" install -y --no-install-recommends "$@"
}

if [ -f "$mirrors" ] && ! getent hosts azure.archive.ubuntu.com >/dev/null; then
    echo "apt-install: azure.archive.ubuntu.com does not resolve"
    use_main_archive
fi

if ! install "$@"; then
    echo "apt-install: install failed, retrying once against archive.ubuntu.com"
    use_main_archive
    install "$@"
fi
