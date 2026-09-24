mod common;

use std::process::Stdio;

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[tokio::test]
async fn delivery_survives_pool_reopen() {
    let db = common::TestDb::new();
    let payload = json!({ "event": "invoice.created", "invoice_id": "inv_789" });

    let first_router = db.router().await;
    let (status, created) = common::post_json(
        &first_router,
        Some("reopen-key"),
        &json!({ "target_url": "https://example.test/webhooks", "payload": payload }),
    )
    .await;
    assert_eq!(status, http::StatusCode::CREATED);
    let id = created["id"]
        .as_str()
        .expect("response has an id")
        .to_owned();
    drop(first_router);

    let second_router = db.router().await;
    let (status, fetched) = common::get_delivery(&second_router, &id).await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(fetched["payload"], payload);
    assert_eq!(fetched["created_at"], created["created_at"]);
}

fn reserve_local_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("reserve a local port");
    let port = listener.local_addr().expect("get reserved port").port();
    drop(listener);
    port
}

fn spawn_server(database_url: &str, port: u16) -> tokio::process::Child {
    tokio::process::Command::new(env!("CARGO_BIN_EXE_relaybox"))
        .env("RELAYBOX_DATABASE_URL", database_url)
        .env("RELAYBOX_BIND", format!("127.0.0.1:{port}"))
        .env("RUST_LOG", "error")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn relaybox binary")
}

async fn http_roundtrip(port: u16, request: &str) -> std::io::Result<(u16, String)> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await?;
    stream.write_all(request.as_bytes()).await?;
    let mut buffer = Vec::new();
    stream.read_to_end(&mut buffer).await?;

    let text = String::from_utf8_lossy(&buffer).to_string();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((text.as_str(), ""));
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "no status line"))?;

    Ok((status, body.to_owned()))
}

async fn wait_for_health(port: u16, child: &mut tokio::process::Child) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let request =
            format!("GET /health HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
        if http_roundtrip(port, &request)
            .await
            .is_ok_and(|(status, _)| status == 200)
        {
            return true;
        }
        // The child may have exited early (for example because it lost the race
        // to bind its reserved port); detect that instead of waiting out the
        // deadline.
        if child.try_wait().ok().flatten().is_some() {
            return false;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// Spawns the server on a freshly reserved port and retries with another fresh
/// port if the child exits before becoming healthy, which can happen when
/// another process binds the reserved port between reservation and bind.
async fn spawn_healthy_server(database_url: &str) -> (u16, tokio::process::Child) {
    const MAX_ATTEMPTS: u32 = 5;
    for _ in 0..MAX_ATTEMPTS {
        let port = reserve_local_port();
        let mut child = spawn_server(database_url, port);
        if wait_for_health(port, &mut child).await {
            return (port, child);
        }
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    panic!("server did not become healthy after {MAX_ATTEMPTS} attempts");
}

#[tokio::test]
async fn delivery_survives_full_process_restart() {
    let db = common::TestDb::new();

    let (port1, mut first) = spawn_healthy_server(&db.url()).await;

    let payload = json!({ "event": "invoice.created", "invoice_id": "inv_456" });
    let body_text = serde_json::to_string(
        &json!({ "target_url": "https://example.test/webhooks", "payload": payload }),
    )
    .expect("serialize request");
    let request = format!(
        "POST /v1/deliveries HTTP/1.1\r\nHost: 127.0.0.1:{port1}\r\nIdempotency-Key: restart-key\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body_text}",
        body_text.len()
    );

    let (status, text) = http_roundtrip(port1, &request).await.expect("post request");
    assert_eq!(status, 201);
    let created: Value = serde_json::from_str(&text).expect("response is JSON");
    let id = created["id"]
        .as_str()
        .expect("response has an id")
        .to_owned();

    first.kill().await.expect("kill first process");
    first.wait().await.expect("wait for first process");

    let (port2, mut second) = spawn_healthy_server(&db.url()).await;

    let request = format!(
        "GET /v1/deliveries/{id} HTTP/1.1\r\nHost: 127.0.0.1:{port2}\r\nConnection: close\r\n\r\n"
    );
    let (status, text) = http_roundtrip(port2, &request).await.expect("get request");
    assert_eq!(status, 200);
    let fetched: Value = serde_json::from_str(&text).expect("response is JSON");
    assert_eq!(fetched["payload"], payload);
    assert_eq!(fetched["created_at"], created["created_at"]);

    second.kill().await.expect("kill second process");
    second.wait().await.expect("wait for second process");
}
