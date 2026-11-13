//! For the desktop host routes (M6): a bare HTTP/1.1 client, enough for long polls and posted replies.
#![allow(dead_code)] // Each test binary uses part of this.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|_| panic!("JSON body, got {}", String::from_utf8_lossy(&self.body)))
    }
}

/// One request on its own connection (`Connection: close`). `Host` defaults to this machine's loopback and port.
pub async fn http(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Reply {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    if !headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("host"))
    {
        head.push_str(&format!("Host: 127.0.0.1:{port}\r\n"));
    }
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await.unwrap();
    // A refused oversized body may close the connection before every byte is written.
    let _ = stream.write_all(body).await;
    let mut raw = Vec::new();
    timeout(Duration::from_secs(60), stream.read_to_end(&mut raw))
        .await
        .expect("a reply in time")
        .ok();
    let text = String::from_utf8_lossy(&raw);
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("an HTTP reply, got {text:?}"));
    let start = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map_or(raw.len(), |at| at + 4);
    Reply {
        status,
        body: raw[start..].to_vec(),
    }
}

pub async fn get(port: u16, path: &str, headers: &[(&str, &str)]) -> Reply {
    http(port, "GET", path, headers, b"").await
}

pub async fn post(port: u16, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Reply {
    http(port, "POST", path, headers, body).await
}
