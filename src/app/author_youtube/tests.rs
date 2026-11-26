use super::*;
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
};

#[derive(Clone)]
pub(crate) struct Reply {
    pub status: u16,
    pub body: String,
    pub delay: Duration,
}
pub(crate) struct Server {
    pub endpoint: String,
    pub reply: Arc<Mutex<Reply>>,
    pub requests: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Server {
    pub fn new(body: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}/feed", listener.local_addr().unwrap());
        let reply = Arc::new(Mutex::new(Reply {
            status: 200,
            body,
            delay: Duration::ZERO,
        }));
        let requests = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (state, count, stopped) = (reply.clone(), requests.clone(), stop.clone());
        let worker = thread::spawn(move || {
            while !stopped.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut socket, _)) => {
                        // Winsock inherits the listener's nonblocking mode.
                        socket.set_nonblocking(false).unwrap();
                        socket
                            .set_read_timeout(Some(Duration::from_secs(1)))
                            .unwrap();
                        let mut buffer = [0; 4096];
                        let _ = socket.read(&mut buffer);
                        count.fetch_add(1, Ordering::AcqRel);
                        let reply = state.lock().unwrap().clone();
                        let _ = write!(
                            socket,
                            "HTTP/1.1 {} Test\r\nContent-Type: application/atom+xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            reply.status,
                            reply.body.len()
                        );
                        let until = Instant::now() + reply.delay;
                        while Instant::now() < until && !stopped.load(Ordering::Acquire) {
                            thread::sleep(Duration::from_millis(2));
                        }
                        let _ = socket.write_all(reply.body.as_bytes());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2))
                    }
                    Err(error) => panic!("HTTP fixture: {error}"),
                }
            }
        });
        Self {
            endpoint,
            reply,
            requests,
            stop,
            worker: Some(worker),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.worker.take().unwrap().join().unwrap();
    }
}

#[test]
fn local_response_waits_for_request_after_connection() {
    let server = Server::new("fixture feed".into());
    let address = server
        .endpoint
        .strip_prefix("http://")
        .unwrap()
        .strip_suffix("/feed")
        .unwrap();
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let error = socket.read(&mut [0; 1]).unwrap_err();
    assert!(matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    ));
    assert_eq!(server.requests.load(Ordering::Acquire), 0);
    socket
        .write_all(b"GET /feed HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut response = String::new();
    socket.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200 Test\r\n"));
    assert!(response.ends_with("\r\n\r\nfixture feed"));
    assert_eq!(server.requests.load(Ordering::Acquire), 1);
}

pub(crate) fn entry(id: &str, published: &str, updated: &str) -> String {
    format!(
        "<entry><yt:videoId>{id}</yt:videoId><yt:channelId>{CHANNEL}</yt:channelId><published>{published}</published><updated>{updated}</updated></entry>"
    )
}
pub(crate) fn feed(entries: &str) -> String {
    format!(
        "<feed xmlns='http://www.w3.org/2005/Atom' xmlns:yt='http://www.youtube.com/xml/schemas/2015'><yt:channelId>9xyobUju3jFwbwT-Kbo-IA</yt:channelId>{entries}</feed>"
    )
}
fn url(id: &str) -> String {
    format!("https://www.youtube.com/watch?v={id}")
}
fn open_output(lookup: &mut LatestVideo, ctx: &Context) -> Vec<String> {
    let output = ctx.run(egui::RawInput::default(), |ctx| lookup.open_ready(ctx));
    output
        .platform_output
        .commands
        .iter()
        .filter_map(|c| match c {
            egui::OutputCommand::OpenUrl(open) => Some(open.url.clone()),
            _ => None,
        })
        .collect()
}
fn wait_open(lookup: &mut LatestVideo, ctx: &Context, timeout: Duration) -> Vec<String> {
    let start = Instant::now();
    loop {
        let output = open_output(lookup, ctx);
        if !output.is_empty() {
            return output;
        }
        assert!(
            start.elapsed() < timeout,
            "lookup did not emit an open-url request"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn http_latest_published_changes_without_rebuild_and_ignores_updated_order() {
    let entries = entry(
        "older000001",
        "2026-01-01T00:00:00Z",
        "2030-01-01T00:00:00Z",
    ) + &entry(
        "newer000002",
        "2026-10-02T01:00:00+01:00",
        "2026-10-02T01:00:00+01:00",
    ) + &entry(
        "third000003",
        "2026-09-01T00:00:00Z",
        "2029-01-01T00:00:00Z",
    );
    let server = Server::new(feed(&entries));
    let ctx = Context::default();
    let mut lookup = LatestVideo::with_endpoint(server.endpoint.clone(), TIMEOUT);
    assert!(open_output(&mut lookup, &ctx).is_empty());
    assert_eq!(
        server.requests.load(Ordering::Acquire),
        0,
        "idle must never poll"
    );
    lookup.activate(&ctx);
    assert_eq!(
        wait_open(&mut lookup, &ctx, Duration::from_secs(2)),
        [url("newer000002")]
    );
    server.reply.lock().unwrap().body = feed(
        &(entries
            + &entry(
                "later000004",
                "2026-11-01T00:00:00Z",
                "2026-11-01T00:00:00Z",
            )),
    );
    lookup.activate(&ctx);
    assert_eq!(
        wait_open(&mut lookup, &ctx, Duration::from_secs(2)),
        [url("later000004")]
    );
    assert_eq!(server.requests.load(Ordering::Acquire), 2);
}

#[test]
fn actual_five_second_complete_timeout_keeps_rendering_and_coalesces_activation() {
    let server = Server::new(feed(&entry(
        "later000004",
        "2026-11-01T00:00:00Z",
        "2026-11-01T00:00:00Z",
    )));
    server.reply.lock().unwrap().delay = Duration::from_secs(7);
    let ctx = Context::default();
    let mut lookup = LatestVideo::with_endpoint(server.endpoint.clone(), TIMEOUT);
    let start = Instant::now();
    lookup.activate(&ctx);
    for _ in 0..20 {
        lookup.activate(&ctx);
    }
    for _ in 0..30 {
        let frame = Instant::now();
        assert!(open_output(&mut lookup, &ctx).is_empty());
        assert!(
            frame.elapsed() < Duration::from_millis(100),
            "render blocked on feed body"
        );
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(server.requests.load(Ordering::Acquire), 1);
    assert_eq!(
        wait_open(&mut lookup, &ctx, Duration::from_secs(6)),
        [FALLBACK]
    );
    assert!(start.elapsed() >= Duration::from_millis(4800));
    assert!(
        start.elapsed() < Duration::from_millis(5500),
        "five-second deadline includes body"
    );
    assert!(
        open_output(&mut lookup, &ctx).is_empty(),
        "must not open twice"
    );
}

#[test]
fn http_errors_invalid_empty_oversized_and_wrong_channel_fall_back() {
    for (status, body) in [
        (503, feed("")),
        (200, "<feed>broken".into()),
        (200, feed("")),
        (200, "x".repeat(MAX_FEED_BYTES + 1)),
        (
            200,
            feed(
                &entry(
                    "later000004",
                    "2026-11-01T00:00:00Z",
                    "2030-01-01T00:00:00Z",
                )
                .replace(CHANNEL, "UCwrong"),
            ),
        ),
    ] {
        let server = Server::new(body);
        server.reply.lock().unwrap().status = status;
        let ctx = Context::default();
        let mut lookup = LatestVideo::with_endpoint(server.endpoint.clone(), TIMEOUT);
        lookup.activate(&ctx);
        assert_eq!(
            wait_open(&mut lookup, &ctx, Duration::from_secs(2)),
            [FALLBACK]
        );
    }
}

#[test]
fn parser_requires_real_namespaces_valid_ids_dates_and_complete_xml() {
    let good = entry(
        "abcde_123-4",
        "2026-10-02T00:00:00Z",
        "2030-01-01T00:00:00Z",
    );
    assert_eq!(latest_published(&feed(&good)), Some(url("abcde_123-4")));
    assert_eq!(
        latest_published(&feed(&good.replace("abcde_123-4", "abcde&#95;123-4"))),
        Some(url("abcde_123-4"))
    );
    assert_eq!(
        latest_published(
            &feed(&good)
                .replace("xmlns:yt=", "xmlns:other=")
                .replace("yt:", "other:")
        ),
        Some(url("abcde_123-4"))
    );
    for broken in [
        feed(&good).replace(
            "http://www.youtube.com/xml/schemas/2015",
            "https://evil.invalid",
        ),
        feed(&good).replace("abcde_123-4", "bad/?target"),
        feed(&good).replace("2026-10-02T00:00:00Z", "not-a-date"),
        feed(&good) + "<broken>",
        feed(&good.replace(
            "</published>",
            "</published><published>2027-01-01T00:00:00Z</published>",
        )),
        format!(
            "<!DOCTYPE feed [<!ENTITY injected 'value'>]>{}",
            feed(&good)
        ),
    ] {
        assert_eq!(
            latest_published(&broken),
            None,
            "must reject malformed/untrusted feed"
        );
    }
}

#[test]
fn invalid_entries_do_not_replace_a_valid_published_video() {
    let valid = entry(
        "valid000001",
        "2026-01-01T00:00:00Z",
        "2026-01-01T00:00:00Z",
    );
    let malformed = entry(
        "bad/?target",
        "2030-01-01T00:00:00Z",
        "2030-01-01T00:00:00Z",
    );
    let wrong_channel = entry(
        "wrong000002",
        "2031-01-01T00:00:00Z",
        "2031-01-01T00:00:00Z",
    )
    .replace(CHANNEL, "UCother");
    assert_eq!(
        latest_published(&feed(&(valid + &malformed + &wrong_channel))),
        Some(url("valid000001"))
    );
}

#[test]
fn connection_failure_opens_fallback_once() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/feed", listener.local_addr().unwrap());
    drop(listener);
    let ctx = Context::default();
    let mut lookup = LatestVideo::with_endpoint(endpoint, TIMEOUT);
    lookup.activate(&ctx);
    assert_eq!(
        // Windows can take over two seconds to report a refused connection.
        // Allow the configured lookup deadline, plus worker scheduling slack.
        wait_open(&mut lookup, &ctx, TIMEOUT + Duration::from_secs(1)),
        [FALLBACK]
    );
    assert!(open_output(&mut lookup, &ctx).is_empty());
}

#[test]
fn resolver_like_blocking_work_cannot_delay_timeout_publication() {
    let (finished, completion) = mpsc::channel();
    let (release, held) = mpsc::channel::<()>();
    let result = resolve_before_deadline(
        async move {
            // reqwest's default DNS resolver uses spawn_blocking. A timed-out
            // request must publish fallback without waiting for that OS call,
            // which here lasts until the test releases it (bounded at 10 s).
            let _ = tokio::task::spawn_blocking(move || {
                let _ = held.recv_timeout(Duration::from_secs(10));
                let _ = finished.send(());
            })
            .await;
            Some(url("later000004"))
        },
        Instant::now() + Duration::from_millis(50),
    );
    let resolver_still_held = completion.try_recv().is_err();
    // Always reap the simulated resolver before asserting, including on
    // failure; the guard doesn't leave background work behind.
    drop(release);
    completion.recv_timeout(Duration::from_secs(12)).unwrap();
    assert_eq!(result, FALLBACK);
    assert!(
        resolver_still_held,
        "runtime teardown waited for the blocking resolver before publishing fallback"
    );
}
