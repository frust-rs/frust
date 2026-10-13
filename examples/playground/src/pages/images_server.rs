//! Loopback fixture server for the Images page's "Use local fixtures" toggle.
//!
//! Started on demand (first toggle), never at app launch: a tokio runtime on its own
//! thread serves PNGs generated in memory at that moment. Same shape as
//! `plugins/network-image/tests/support/mod.rs`'s server, reduced to the one route the
//! page needs and copied here so the playground has no dev-dependency on the plugin's tests.
//! The images are plain-colour gradients; `/img/<n>.png` for `n` in `0..FIXTURE_COUNT`.
//! The server and its thread live for the rest of the process.

use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// How many distinct fixture images the server serves.
pub const FIXTURE_COUNT: usize = 6;

const WIDTH: u32 = 160;
const HEIGHT: u32 = 120;

static SERVER: OnceLock<Option<SocketAddr>> = OnceLock::new();

/// Base URL (`http://127.0.0.1:<port>`) of the fixture server, starting it on the first
/// call. `None` when the listener could not be bound.
pub fn base_url() -> Option<String> {
    let addr = (*SERVER.get_or_init(start))?;
    Some(format!("http://{addr}"))
}

fn start() -> Option<SocketAddr> {
    let bodies: Arc<Vec<Vec<u8>>> = Arc::new((0..FIXTURE_COUNT).map(fixture_png).collect());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("images-fixture-server".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_io()
                .build()
            {
                Ok(runtime) => runtime,
                Err(_) => {
                    let _ = tx.send(None);
                    return;
                }
            };
            runtime.block_on(async move {
                let Ok(listener) = TcpListener::bind("127.0.0.1:0").await else {
                    let _ = tx.send(None);
                    return;
                };
                let _ = tx.send(listener.local_addr().ok());
                serve(listener, bodies).await;
            });
        })
        .ok()?;
    rx.recv().ok().flatten()
}

async fn serve(listener: TcpListener, bodies: Arc<Vec<Vec<u8>>>) {
    loop {
        let Ok((mut sock, _)) = listener.accept().await else {
            return;
        };
        let bodies = bodies.clone();
        tokio::spawn(async move {
            let mut buf = Vec::new();
            let mut tmp = [0u8; 2048];
            while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                match sock.read(&mut tmp).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => buf.extend_from_slice(&tmp[..n]),
                }
            }
            let request = String::from_utf8_lossy(&buf);
            let path = request
                .split("\r\n")
                .next()
                .and_then(|line| line.split(' ').nth(1))
                .unwrap_or("");
            let body = path
                .strip_prefix("/img/")
                .and_then(|name| name.strip_suffix(".png"))
                .and_then(|n| n.parse::<usize>().ok())
                .and_then(|n| bodies.get(n));
            let _ = match body {
                Some(body) => {
                    let head = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: image/png\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = sock.write_all(head.as_bytes()).await;
                    sock.write_all(body).await
                }
                None => {
                    sock.write_all(
                        b"HTTP/1.1 404 X\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                    )
                    .await
                }
            };
            let _ = sock.shutdown().await;
        });
    }
}

/// A `WIDTH` x `HEIGHT` RGB PNG whose hue is derived from `index`, with a diagonal
/// brightness ramp so Cover cropping is visible.
fn fixture_png(index: usize) -> Vec<u8> {
    let hue = index as f32 / FIXTURE_COUNT as f32;
    let base = [
        channel(hue),
        channel(hue + 1.0 / 3.0),
        channel(hue + 2.0 / 3.0),
    ];
    let mut raw = Vec::with_capacity((WIDTH as usize * 3 + 1) * HEIGHT as usize);
    for y in 0..HEIGHT {
        raw.push(0); // filter: none
        for x in 0..WIDTH {
            let ramp = 0.45 + 0.55 * ((x + y) as f32 / (WIDTH + HEIGHT) as f32);
            raw.extend(base.iter().map(|c| (f32::from(*c) * ramp) as u8));
        }
    }
    encode_png(WIDTH, HEIGHT, &raw)
}

/// One triangle-wave colour channel in `0..=255` for a hue phase.
fn channel(phase: f32) -> u8 {
    let t = phase.rem_euclid(1.0);
    let v = (1.0 - (t * 2.0 - 1.0).abs()).clamp(0.0, 1.0);
    (80.0 + 175.0 * v) as u8
}

/// Minimal PNG writer: 8-bit RGB, one IDAT of stored (uncompressed) deflate blocks.
fn encode_png(width: u32, height: u32, scanlines: &[u8]) -> Vec<u8> {
    let mut zlib = vec![0x78, 0x01];
    let mut chunks = scanlines.chunks(65_535).peekable();
    while let Some(chunk) = chunks.next() {
        zlib.push(u8::from(chunks.peek().is_none()));
        zlib.extend_from_slice(&(chunk.len() as u16).to_le_bytes());
        zlib.extend_from_slice(&(!(chunk.len() as u16)).to_le_bytes());
        zlib.extend_from_slice(chunk);
    }
    zlib.extend_from_slice(&adler32(scanlines).to_be_bytes());

    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);

    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    write_chunk(&mut out, b"IHDR", &ihdr);
    write_chunk(&mut out, b"IDAT", &zlib);
    write_chunk(&mut out, b"IEND", &[]);
    out
}

fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    use super::*;

    fn get(addr: &str, path: &str) -> Vec<u8> {
        let mut stream = TcpStream::connect(addr).expect("connect");
        write!(stream, "GET {path} HTTP/1.1\r\nhost: x\r\n\r\n").unwrap();
        let mut out = Vec::new();
        stream.read_to_end(&mut out).unwrap();
        out
    }

    #[test]
    fn png_has_signature_and_expected_stored_length() {
        let png = fixture_png(0);
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
        assert!(png.len() > (WIDTH * HEIGHT * 3) as usize);
    }

    #[test]
    fn crc_and_adler_match_known_vectors() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn serves_fixtures_and_404s_unknown_paths() {
        let base = base_url().expect("server starts");
        let addr = base.trim_start_matches("http://");
        let ok = get(addr, "/img/2.png");
        assert!(ok.starts_with(b"HTTP/1.1 200 OK"));
        assert!(ok.windows(8).any(|w| w == b"\x89PNG\r\n\x1a\n"));
        assert!(get(addr, "/img/99.png").starts_with(b"HTTP/1.1 404"));
        assert_eq!(base_url().as_deref(), Some(base.as_str()));
    }
}
