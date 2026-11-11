#!/usr/bin/env bash
# Installs apt packages on a hosted Linux runner: `apt-install.sh <package>...`.
#
# The runner image points apt at `azure.archive.ubuntu.com` through
# `/etc/apt/apt-mirrors.txt`. When name resolution on the runner turns flaky,
# each download that misses waits out a DNS timeout, and the step fails after
# about twenty minutes with `Temporary failure resolving`. This script resolves
# the archive hosts once, with retries, and pins them in `/etc/hosts` so the
# downloads no longer depend on DNS; it bounds each fetch; and it falls back to
# `archive.ubuntu.com` when the Azure name cannot be resolved at all, or once
# more if the install fails anyway.
set -euo pipefail

if [ "$#" -eq 0 ]; then
    echo "usage: apt-install.sh <package>..." >&2
    exit 2
fi

mirrors=/etc/apt/apt-mirrors.txt
opts=(
    -o Acquire::Retries=2
    -o Acquire::http::Timeout=20
    -o Acquire::https::Timeout=20
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

# Prints the first IPv4 address of a host, trying a few times.
resolve() {
    local host=$1 ip
    for _ in 1 2 3 4 5 6 7 8; do
        ip=$(getent ahostsv4 "$host" | awk 'NR == 1 { print $1 }') || true
        if [ -n "${ip:-}" ]; then
            echo "$ip"
            return 0
        fi
        sleep 2
    done
    return 1
}

pin() {
    local host=$1 ip
    if ip=$(resolve "$host"); then
        echo "apt-install: pinning $host to $ip"
        echo "$ip $host" | sudo tee -a /etc/hosts >/dev/null
    else
        echo "apt-install: $host does not resolve"
        return 1
    fi
}

pin archive.ubuntu.com || true
pin security.ubuntu.com || true
if [ -f "$mirrors" ] && ! pin azure.archive.ubuntu.com; then
    use_main_archive
fi

if ! install "$@"; then
    echo "apt-install: install failed, retrying once against archive.ubuntu.com"
    use_main_archive
    install "$@"
fi
