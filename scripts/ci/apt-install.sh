#!/usr/bin/env bash
# Installs apt packages on a hosted Linux runner: `apt-install.sh <package>...`.
#
# The runner image points apt at `azure.archive.ubuntu.com` through
# `/etc/apt/apt-mirrors.txt`. When name resolution on the runner turns flaky,
# each download that misses waits out a DNS timeout, and the step fails after
# about twenty minutes with `Temporary failure resolving`. This script resolves
# the archive hosts once (through public resolvers if the runner's own fails)
# and pins them in `/etc/hosts` so the downloads no longer depend on DNS; it bounds each fetch; and it falls back to
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

# Prints one IPv4 address of a host: the system resolver first, then two public
# resolvers over HTTPS by address, which work when the runner's own resolver is
# the part that is failing.
resolve() {
    local host=$1 ip url
    for _ in 1 2 3; do
        ip=$(getent ahostsv4 "$host" | awk 'NR == 1 { print $1 }') || true
        if [ -n "${ip:-}" ]; then
            echo "$ip"
            return 0
        fi
        sleep 2
    done
    for url in "https://1.1.1.1/dns-query" "https://8.8.8.8/resolve"; do
        ip=$(curl -fsS -m 10 -H 'accept: application/dns-json' "$url?name=$host&type=A" |
            python3 -c 'import json, sys
answers = [a["data"] for a in json.load(sys.stdin).get("Answer", []) if a.get("type") == 1]
print(answers[0] if answers else "")') || true
        if [ -n "${ip:-}" ]; then
            echo "$ip"
            return 0
        fi
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
