//! Loopback HTTP/1.1 test server on a tokio `TcpListener`.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Clone, Debug)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Clone)]
pub enum Route {
    Ok {
        content_type: &'static str,
        body: Vec<u8>,
    },
    Status(u16),
    /// Headers promptly, then the body trickles one byte per `delay`.
    Slow {
        body: Vec<u8>,
        delay: Duration,
    },
    /// Accepts the connection and never answers.
    Hang,
    /// A body of `len` zero bytes without announcing a length (chunked-style close).
    Oversized {
        len: usize,
    },
}

pub struct TestServer {
    pub addr: std::net::SocketAddr,
    pub requests: Arc<Mutex<Vec<Recorded>>>,
}

impl TestServer {
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }
    pub fn https_url(&self, path: &str) -> String {
        format!("https://{}{}", self.addr, path)
    }
    pub fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

pub async fn spawn(routes: Vec<(&'static str, Route)>) -> TestServer {
    let routes: Arc<HashMap<String, Route>> = Arc::new(
        routes
            .into_iter()
            .map(|(p, r)| (p.to_string(), r))
            .collect(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let log = requests.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let routes = routes.clone();
            let log = log.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 2048];
                // A TLS ClientHello (0x16) is not HTTP: close, as a plain server would.
                let mut first = [0u8; 1];
                if sock.peek(&mut first).await.ok() == Some(1) && first[0] == 0x16 {
                    return;
                }
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match sock.read(&mut tmp).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    }
                }
                let text = String::from_utf8_lossy(&buf).into_owned();
                let mut lines = text.split("\r\n");
                let mut first = lines.next().unwrap_or("").split(' ');
                let method = first.next().unwrap_or("").to_string();
                let path = first.next().unwrap_or("").to_string();
                let headers = lines
                    .take_while(|l| !l.is_empty())
                    .filter_map(|l| l.split_once(':'))
                    .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
                    .collect();
                log.lock().unwrap().push(Recorded {
                    method,
                    path: path.clone(),
                    headers,
                });
                let route = routes.get(&path).cloned().unwrap_or(Route::Status(404));
                match route {
                    Route::Ok { content_type, body } => {
                        let head = format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = sock.write_all(head.as_bytes()).await;
                        let _ = sock.write_all(&body).await;
                    }
                    Route::Status(code) => {
                        let head = format!(
                            "HTTP/1.1 {code} X\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                        );
                        let _ = sock.write_all(head.as_bytes()).await;
                    }
                    Route::Slow { body, delay } => {
                        let head = format!(
                            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = sock.write_all(head.as_bytes()).await;
                        for b in body {
                            tokio::time::sleep(delay).await;
                            if sock.write_all(&[b]).await.is_err() {
                                return;
                            }
                        }
                    }
                    Route::Hang => {
                        tokio::time::sleep(Duration::from_secs(60)).await;
                    }
                    Route::Oversized { len } => {
                        let _ = sock
                            .write_all(b"HTTP/1.1 200 OK\r\nconnection: close\r\n\r\n")
                            .await;
                        let chunk = vec![0u8; 64 * 1024];
                        let mut sent = 0;
                        while sent < len {
                            let n = chunk.len().min(len - sent);
                            if sock.write_all(&chunk[..n]).await.is_err() {
                                return;
                            }
                            sent += n;
                        }
                    }
                }
                let _ = sock.shutdown().await;
            });
        }
    });
    TestServer { addr, requests }
}
