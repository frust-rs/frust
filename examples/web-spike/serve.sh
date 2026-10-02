#!/usr/bin/env sh
# Static file server for the w0-03 browser render probe.
#
# Serves THIS directory (the spike root), so `/` is `index.html` and
# `/pkg/web_spike.js` + `/pkg/web_spike_bg.wasm` resolve as the page's module
# import expects.
#
# Why the stdlib server and not a dependency: the only hard requirement a
# `--target web` wasm page places on its server is that `.wasm` is sent as
# `application/wasm`, because `WebAssembly.instantiateStreaming` refuses any
# other type. Python's `mimetypes` has mapped `.wasm` to `application/wasm`
# since 3.9 (verified on this host's 3.14.6 with
# `python3 -c "import mimetypes; print(mimetypes.guess_type('a.wasm'))"`, and
# again over the wire with `curl -I`), so `http.server` is sufficient and adds
# no tool to install.
#
# Bound to 127.0.0.1 on purpose — this serves a local build directory and has
# no business on a LAN interface. The browser used by this probe runs in a
# container started with `--network host`, so `http://localhost:PORT/` reaches
# this process from inside it without any port publishing.
#
# Usage:
#   ./serve.sh          # pick a free high port, print it, serve until Ctrl-C
#   ./serve.sh 8931     # use that exact port
set -eu

cd "$(dirname "$0")"

if [ "$#" -ge 1 ]; then
    PORT="$1"
else
    # Ask the OS for a free ephemeral port and then reuse the number. There is
    # a theoretical race between closing this socket and http.server binding
    # it; on a single-user probe host that is not worth a retry loop, and an
    # explicit port argument is the escape hatch when it ever bites.
    PORT="$(python3 -c 'import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()')"
fi

echo "web-spike: serving $(pwd) at http://127.0.0.1:${PORT}/"
echo "web-spike:   WebGPU arm  http://localhost:${PORT}/?arm=webgpu"
echo "web-spike:   WebGL2 arm  http://localhost:${PORT}/?arm=webgl"

exec python3 -m http.server "${PORT}" --bind 127.0.0.1
