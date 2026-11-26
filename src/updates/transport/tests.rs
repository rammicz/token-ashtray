//! Local HTTP fixture: `https://host/path` is served at `http://127.0.0.1:N/host/path`.
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

use super::*;
use crate::updates::{Target, sha256};

pub(in crate::updates) const LATEST: &str = DISCOVERY_URL;
pub(in crate::updates) const TAGGED: &str =
    "https://github.com/rammicz/token-ashtray/releases/download/v0.2.2/token-ashtray-updates.json";
const CDN: &str = "https://release-assets.githubusercontent.com/opaque/1?sig=TRANSPORT_ONLY";
pub(in crate::updates) const ARTIFACT: &str = "https://github.com/rammicz/token-ashtray/releases/download/v0.2.2/token-ashtray-setup-0.2.2-x86_64-unknown-linux-gnu";
const PAYLOAD: &[u8] = b"SYNTHETIC NONEXECUTABLE x86_64-unknown-linux-gnu 0.2.2";

#[derive(Clone)]
pub(in crate::updates) struct Route {
    status: u16,
    location: Option<String>,
    body: Vec<u8>,
    declare_length: bool,
    delay: Duration,
}

pub(in crate::updates) fn ok(body: &[u8]) -> Route {
    Route {
        status: 200,
        location: None,
        body: body.to_vec(),
        declare_length: true,
        delay: Duration::ZERO,
    }
}

fn status(code: u16) -> Route {
    Route {
        status: code,
        ..ok(b"")
    }
}

/// A 200 response whose body arrives after `delay`.
pub(in crate::updates) fn delayed(body: &[u8], delay: Duration) -> Route {
    Route { delay, ..ok(body) }
}

pub(in crate::updates) fn redirect(location: &str) -> Route {
    Route {
        location: Some(location.into()),
        ..status(302)
    }
}

/// Answers each path with its routes in order, repeating the last one, and
/// records every request head. Unknown paths get 404. The accept thread is
/// detached and parks in `accept` once the test ends.
pub(in crate::updates) struct Server {
    origin: String,
    heads: Arc<Mutex<Vec<String>>>,
}

impl Server {
    pub(in crate::updates) fn new(routes: &[(&str, Vec<Route>)]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let routes: HashMap<_, _> = routes
            .iter()
            .map(|(url, replies)| (path(url), replies.clone()))
            .collect();
        let routes = Arc::new(Mutex::new(routes));
        let heads = Arc::new(Mutex::new(Vec::new()));
        let seen = heads.clone();
        thread::spawn(move || {
            for socket in listener.incoming().flatten() {
                let (routes, seen) = (routes.clone(), seen.clone());
                thread::spawn(move || serve(socket, &routes, &seen));
            }
        });
        Self { origin, heads }
    }

    pub(in crate::updates) fn transport(&self, timeout: Duration) -> Transport {
        local_transport(&self.origin, timeout)
    }

    /// Request paths in arrival order.
    pub(in crate::updates) fn paths(&self) -> Vec<String> {
        let heads = self.heads.lock().unwrap();
        heads
            .iter()
            .map(|head| head.split(' ').nth(1).unwrap().to_owned())
            .collect()
    }
}

fn serve(
    mut socket: TcpStream,
    routes: &Mutex<HashMap<String, Vec<Route>>>,
    heads: &Mutex<Vec<String>>,
) {
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut head = Vec::new();
    let mut buffer = [0; 1024];
    while !head.ends_with(b"\r\n\r\n") {
        match socket.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(read) => head.extend_from_slice(&buffer[..read]),
        }
    }
    let head = String::from_utf8(head).unwrap();
    let path = head.split(' ').nth(1).unwrap().to_owned();
    heads.lock().unwrap().push(head);
    let route = match routes.lock().unwrap().get_mut(&path) {
        Some(replies) if replies.len() > 1 => replies.remove(0),
        Some(replies) => replies[0].clone(),
        None => status(404),
    };
    let mut reply = format!("HTTP/1.1 {} Fixture\r\nConnection: close\r\n", route.status);
    if let Some(location) = &route.location {
        reply += &format!("Location: {location}\r\n");
    }
    if route.declare_length {
        reply += &format!("Content-Length: {}\r\n", route.body.len());
    }
    reply += "\r\n";
    let _ = socket.write_all(reply.as_bytes());
    thread::sleep(route.delay);
    let _ = socket.write_all(&route.body);
}

fn local_transport(origin: &str, timeout: Duration) -> Transport {
    Transport {
        client: client(timeout, false).unwrap(),
        local: Some(origin.to_owned()),
    }
}

/// The local request path for a public URL.
pub(in crate::updates) fn path(url: &str) -> String {
    url.strip_prefix("https:/").unwrap().to_owned()
}

fn transport(server: &Server) -> Transport {
    server.transport(Duration::from_secs(5))
}

fn rejected(result: Result<impl std::fmt::Debug, FetchError>) -> &'static str {
    match result.unwrap_err() {
        FetchError::Rejected(reject) => reject.code(),
        other => panic!("expected a rejection, got {other:?}"),
    }
}

fn v(text: &str) -> Version {
    Version::parse(text).unwrap()
}

#[tokio::test]
async fn discovery_follows_the_exact_graph_with_anonymous_requests() {
    let server = Server::new(&[
        (LATEST, vec![redirect(TAGGED)]),
        (TAGGED, vec![redirect(CDN)]),
        (CDN, vec![ok(b"manifest bytes")]),
    ]);
    let found = transport(&server).discover().await.unwrap();
    assert_eq!(found.version, v("0.2.2"));
    assert_eq!(found.raw, b"manifest bytes");
    assert_eq!(server.paths(), [path(LATEST), path(TAGGED), path(CDN)]);
    for head in server.heads.lock().unwrap().iter() {
        let head = head.to_ascii_lowercase();
        assert!(head.starts_with("get "), "{head}");
        assert!(head.contains(&format!("user-agent: {}", USER_AGENT.to_ascii_lowercase())));
        assert!(head.contains("accept: application/octet-stream"), "{head}");
        for header in ["referer:", "cookie:", "authorization:"] {
            assert!(!head.contains(header), "{head}");
        }
    }
}

#[tokio::test]
async fn tagged_manifest_may_answer_directly() {
    let server = Server::new(&[
        (LATEST, vec![redirect(TAGGED)]),
        (TAGGED, vec![ok(b"direct")]),
    ]);
    assert_eq!(transport(&server).discover().await.unwrap().raw, b"direct");
}

/// One case per rejection; the full URL grammar is covered in `channel`.
#[tokio::test]
async fn hops_outside_the_graph_are_rejected_before_any_request() {
    let tagged_to = |location: &str| {
        Server::new(&[
            (LATEST, vec![redirect(TAGGED)]),
            (TAGGED, vec![redirect(location)]),
            (CDN, vec![redirect(CDN)]),
        ])
    };
    let cases = [
        (
            "latest-discovery",
            Server::new(&[(LATEST, vec![ok(b"x")])]),
            1,
        ),
        (
            "manifest-tag",
            Server::new(&[(LATEST, vec![redirect("https://evil.test/a")])]),
            1,
        ),
        (
            "redirect-location",
            Server::new(&[(LATEST, vec![status(302)])]),
            1,
        ),
        ("asset-cdn", tagged_to("https://evil.test/a"), 2),
        ("url-bound", tagged_to(&format!("{CDN}%20 x")), 2),
        ("asset-redirect-count", tagged_to(CDN), 3),
    ];
    for (code, server, requests) in cases {
        assert_eq!(rejected(transport(&server).discover().await), code);
        assert_eq!(server.paths().len(), requests, "{code}");
    }
}

#[tokio::test]
async fn declared_and_actual_sizes_are_bounded() {
    let exact = vec![b'x'; MAX_METADATA];
    let server = Server::new(&[(LATEST, vec![redirect(TAGGED)]), (TAGGED, vec![ok(&exact)])]);
    assert_eq!(transport(&server).discover().await.unwrap().raw, exact);
    let over = vec![b'x'; MAX_METADATA + 1];
    for declare_length in [true, false] {
        let oversized = Route {
            declare_length,
            ..ok(&over)
        };
        let server = Server::new(&[(LATEST, vec![redirect(TAGGED)]), (TAGGED, vec![oversized])]);
        assert_eq!(
            rejected(transport(&server).discover().await),
            "response-size"
        );
    }
}

#[tokio::test]
async fn temporary_failures_get_exactly_one_retry() {
    let server = Server::new(&[
        (LATEST, vec![status(503), redirect(TAGGED)]),
        (TAGGED, vec![ok(b"after retry")]),
    ]);
    assert_eq!(
        transport(&server).discover().await.unwrap().raw,
        b"after retry"
    );
    assert_eq!(server.paths(), [path(LATEST), path(LATEST), path(TAGGED)]);

    for (code, attempts) in [(503, 2), (429, 2), (404, 1), (403, 1)] {
        let server = Server::new(&[(LATEST, vec![status(code)])]);
        let error = transport(&server).discover().await.unwrap_err();
        assert_eq!(error, FetchError::Status(code));
        assert_eq!(server.paths().len(), attempts, "{code}");
    }
}

#[tokio::test]
async fn timeouts_and_outages_are_network_errors_after_one_retry() {
    let slow = Route {
        delay: Duration::from_millis(800),
        ..ok(b"late")
    };
    let server = Server::new(&[(LATEST, vec![redirect(TAGGED)]), (TAGGED, vec![slow])]);
    let short = Duration::from_millis(200);
    let error = server.transport(short).discover().await.unwrap_err();
    assert_eq!(error, FetchError::Network);
    assert_eq!(
        server.paths(),
        [path(LATEST), path(TAGGED), path(LATEST), path(TAGGED)]
    );

    let closed = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    let offline = local_transport(&origin, short);
    assert_eq!(offline.discover().await.unwrap_err(), FetchError::Network);
}

fn record() -> ArtifactRecord {
    ArtifactRecord {
        target: Target::LinuxX86_64,
        version: v("0.2.2"),
        filename: "token-ashtray-setup-0.2.2-x86_64-unknown-linux-gnu".into(),
        length: PAYLOAD.len() as u64,
        sha256: sha256(PAYLOAD),
        app_sha256: [0xbb; 32],
        descriptor_sha256: [0xcc; 32],
    }
}

#[tokio::test]
async fn downloads_stream_to_the_file_and_check_signed_size_and_hash() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("artifact");
    std::fs::write(
        &file,
        b"stale bytes that are longer than the payload itself",
    )
    .unwrap();
    let server = Server::new(&[(ARTIFACT, vec![redirect(CDN)]), (CDN, vec![ok(PAYLOAD)])]);
    transport(&server).download(&record(), &file).await.unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), PAYLOAD);
    assert_eq!(server.paths(), [path(ARTIFACT), path(CDN)]);

    let mut changed = PAYLOAD.to_vec();
    changed[0] ^= 1;
    let longer = [PAYLOAD, b"!"].concat();
    for (body, code) in [(changed, "artifact-bytes"), (longer, "response-size")] {
        let server = Server::new(&[(ARTIFACT, vec![ok(&body)])]);
        let result = transport(&server).download(&record(), &file).await;
        assert_eq!(rejected(result), code);
    }
    // A retry starts the file over rather than appending.
    let server = Server::new(&[(ARTIFACT, vec![status(503), ok(PAYLOAD)])]);
    transport(&server).download(&record(), &file).await.unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), PAYLOAD);
}

#[tokio::test]
async fn production_client_refuses_plain_http() {
    let server = Server::new(&[]);
    let production = Transport::new().unwrap();
    let plain = format!("{}/x", server.origin);
    let result = production.get(&plain, MAX_METADATA).await;
    assert!(matches!(result, Err(FetchError::Network)));
    assert!(server.paths().is_empty());
}
