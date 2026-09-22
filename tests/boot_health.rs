//! `/health` must answer before Postgres accepts a session and before CMS registration returns.
//! This drives the shipped binary (current_thread runtime), not a reimplemented server.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

struct SilentPeer {
    port: u16,
    shutdown: Arc<AtomicBool>,
    accept: Option<thread::JoinHandle<()>>,
}

impl SilentPeer {
    fn start() -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("silent peer bind");
        listener.set_nonblocking(true).expect("nonblocking");
        let port = listener.local_addr().expect("addr").port();
        let shutdown = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&shutdown);
        let accept = thread::spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((sock, _)) => {
                        let flag = Arc::clone(&flag);
                        thread::spawn(move || hold_socket(sock, &flag));
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            port,
            shutdown,
            accept: Some(accept),
        }
    }
}

impl Drop for SilentPeer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(handle) = self.accept.take() {
            let _ = handle.join();
        }
    }
}

fn hold_socket(mut sock: TcpStream, shutdown: &AtomicBool) {
    let _ = sock.set_read_timeout(Some(Duration::from_millis(100)));
    let mut buf = [0u8; 16];
    while !shutdown.load(Ordering::SeqCst) {
        match sock.read(&mut buf) {
            Ok(0) => break,
            Ok(_) => {}
            Err(err)
                if err.kind() == std::io::ErrorKind::WouldBlock
                    || err.kind() == std::io::ErrorKind::TimedOut =>
            {
                thread::sleep(Duration::from_millis(20));
            }
            Err(_) => break,
        }
    }
    let _ = sock.shutdown(Shutdown::Both);
}

struct Running {
    child: Child,
    stderr: Option<thread::JoinHandle<String>>,
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(handle) = self.stderr.take() {
            if let Ok(err) = handle.join() {
                if !err.is_empty() {
                    eprintln!("binary stderr:\n{err}");
                }
            }
        }
    }
}

fn free_port() -> u16 {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("free port");
    listener.local_addr().expect("addr").port()
}

fn connect_timeout() -> Duration {
    let src = include_str!("../src/main.rs");
    let marker = "connect_timeout(Duration::from_secs(";
    let rest = src.split_once(marker).expect("shipped connect_timeout").1;
    let secs: u64 = rest
        .split(')')
        .next()
        .expect("seconds")
        .trim()
        .parse()
        .expect("seconds parse");
    Duration::from_secs(secs)
}

fn http_get(port: u16, path: &str) -> Result<String, String> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(200))
        .map_err(|err| err.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .ok();
    stream
        .set_write_timeout(Some(Duration::from_millis(500)))
        .ok();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .map_err(|err| err.to_string())?;
    let mut buf = String::new();
    stream
        .read_to_string(&mut buf)
        .map_err(|err| err.to_string())?;
    Ok(buf)
}

fn header<'a>(raw: &'a str, name: &str) -> &'a str {
    raw.lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            if key.eq_ignore_ascii_case(name) {
                Some(value.trim())
            } else {
                None
            }
        })
        .unwrap_or_else(|| panic!("missing {name} in {raw}"))
}

#[test]
fn health_ok_before_postgres_connect_timeout_and_unreachable_registration() {
    let peer = SilentPeer::start();
    let port = free_port();
    let timeout = connect_timeout();
    let mut child = Command::new(env!("CARGO_BIN_EXE_carolina-codes-rust"))
        .env(
            "DATABASE_URL",
            format!(
                "postgres://postgres:postgres@127.0.0.1:{}/carolina_dev",
                peer.port
            ),
        )
        .env("CAROLINA_URL", "http://192.0.2.1:9")
        .env("POLYGLOT_REGISTER_TOKEN", "dev")
        .env("PUBLIC_BASE_URL", format!("http://127.0.0.1:{port}"))
        .env("PORT", port.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn carolina-codes-rust");
    let stderr = child.stderr.take().expect("stderr");
    let stderr = thread::spawn(move || {
        let mut err = String::new();
        let mut stderr = stderr;
        let _ = stderr.read_to_string(&mut err);
        err
    });
    let mut running = Running {
        child,
        stderr: Some(stderr),
    };

    let started = Instant::now();
    let mut last_err = String::from("not started");
    let mut ok: Option<String> = None;
    while started.elapsed() < timeout {
        if let Some(status) = running.child.try_wait().expect("try_wait") {
            panic!("binary exited {status} before /health");
        }
        match http_get(port, "/health") {
            Ok(raw) => {
                ok = Some(raw);
                break;
            }
            Err(err) => {
                last_err = err;
                thread::sleep(Duration::from_millis(20));
            }
        }
    }
    let elapsed = started.elapsed();
    let raw = ok.unwrap_or_else(|| {
        panic!("GET /health did not return within {timeout:?} ({elapsed:?}): {last_err}")
    });
    assert!(
        elapsed < timeout,
        "GET /health took {elapsed:?}, which is not below the Postgres connect timeout {timeout:?}\n{raw}"
    );
    let body = raw
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.trim())
        .unwrap_or("");
    assert_eq!(body, r#"{"ok":true}"#, "{raw}");
    assert_eq!(header(&raw, "x-polyglot-language"), "Rust");
    assert_eq!(header(&raw, "x-polyglot-framework"), "axum");
    assert!(raw.starts_with("HTTP/1.1 200"), "{raw}");
}
