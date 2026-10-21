// SPDX-License-Identifier: AGPL-3.0-only
//! Native loopback half of the reverse MCP relay.
//!
//! The socket module owns the authenticated transport. This module accepts
//! only the already-decoded reverse frames and forwards the two fixed paths to
//! the local MCP HTTP server.

use super::reverse_protocol::{
    decode_body, encode_body, encode_frame, Frame, HeaderMap, MAX_CHUNK_BODY_BYTES,
    MAX_HEADER_VALUE_BYTES,
};
use super::{valid_id, RelayError};
use reqwest::{Client, Method, Response};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::{AbortHandle, JoinSet};
use tokio::time::timeout;

const MAX_PENDING: usize = 8;
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MAX_RESPONSE_CHUNKS: usize = 4096;
const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const OUTGOING_SEND_TIMEOUT: Duration = Duration::from_secs(5);
const TOMBSTONE_LIMIT: usize = 32;

type EventSender = mpsc::Sender<TaskEvent>;

/// Set by the relay on a request that belongs to a write grant. It is read here
/// to pick the local server and never forwarded to either of them.
pub(crate) const ACCESS_HEADER: &str = "x-wenlan-access";

/// The loopback port of the web-write server, when one is running. The
/// connection holds a clone and reads it for each request, so the server can
/// start or stop without reopening the relay connection. Empty means that
/// requests for a write grant fall back to the search-only server, which
/// refuses them.
#[derive(Clone, Debug, Default)]
pub(crate) struct WriteRoute(Arc<AtomicU16>);

impl WriteRoute {
    pub(crate) fn set(&self, port: Option<u16>) {
        self.0.store(port.unwrap_or(0), Ordering::SeqCst);
    }

    pub(crate) fn get(&self) -> Option<u16> {
        Some(self.0.load(Ordering::SeqCst)).filter(|port| *port != 0)
    }
}

/// Pick the local port for one request and remove the routing header. Only the
/// exact value `write` reaches the write server, and only while it runs. A
/// request without the header always goes to the search-only server.
fn route(headers: &mut HeaderMap, query_port: u16, write_port: Option<u16>) -> u16 {
    let wants_write = headers
        .remove(ACCESS_HEADER)
        .is_some_and(|value| value == "write");
    match (wants_write, write_port) {
        (true, Some(port)) => port,
        _ => query_port,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    AwaitingHeaders,
    WaitingCredit,
    CreditInFlight,
    NoBody,
}

struct Pending {
    tag: u64,
    command: mpsc::Sender<TaskCommand>,
    abort: AbortHandle,
    phase: Phase,
    next_sequence: u64,
    bytes: usize,
    chunks: usize,
}

enum TaskCommand {
    Credit(u64),
}

enum TaskEvent {
    Headers {
        tag: u64,
        id: String,
        status: u16,
        headers: HeaderMap,
        body_allowed: bool,
    },
    Chunk {
        tag: u64,
        id: String,
        sequence: u64,
        bytes: Vec<u8>,
    },
    End {
        tag: u64,
        id: String,
    },
    Failure {
        tag: u64,
        id: String,
    },
}

struct TaskExit {
    _tag: u64,
}

struct Tombstones {
    order: VecDeque<String>,
    ids: HashSet<String>,
}

impl Tombstones {
    fn new() -> Self {
        Self {
            order: VecDeque::new(),
            ids: HashSet::new(),
        }
    }

    fn contains(&self, id: &str) -> bool {
        self.ids.contains(id)
    }

    fn insert(&mut self, id: String) {
        if !self.ids.insert(id.clone()) {
            return;
        }
        self.order.push_back(id);
        if self.order.len() > TOMBSTONE_LIMIT {
            if let Some(old) = self.order.pop_front() {
                self.ids.remove(&old);
            }
        }
    }
}

struct Driver {
    incoming: mpsc::Receiver<Frame>,
    outgoing: mpsc::Sender<Frame>,
    local_port: u16,
    write_route: WriteRoute,
    backend_authorization: String,
    client: Client,
    events: mpsc::Receiver<TaskEvent>,
    event_sender: EventSender,
    tasks: JoinSet<TaskExit>,
    pending: HashMap<String, Pending>,
    tombstones: Tombstones,
    next_tag: u64,
}

pub(crate) async fn run_peer(
    port: u16,
    write_route: WriteRoute,
    backend_token: String,
    incoming: mpsc::Receiver<super::reverse_protocol::Frame>,
    outgoing: mpsc::Sender<Frame>,
) -> Result<(), RelayError> {
    if port == 0 || !valid_id(&backend_token, 32) {
        return Err(RelayError::InvalidInput);
    }

    let client = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(HTTP_CONNECT_TIMEOUT)
        .timeout(HTTP_REQUEST_TIMEOUT)
        .build()
        .map_err(|_| RelayError::Unavailable)?;
    let (event_sender, events) = mpsc::channel(MAX_PENDING * 4);
    let mut driver = Driver {
        incoming,
        outgoing,
        local_port: port,
        write_route,
        backend_authorization: format!("Bearer {backend_token}"),
        client,
        events,
        event_sender,
        tasks: JoinSet::new(),
        pending: HashMap::new(),
        tombstones: Tombstones::new(),
        next_tag: 1,
    };

    let result = driver.drive().await;
    driver.tasks.abort_all();
    while driver.tasks.join_next().await.is_some() {}
    result
}

impl Driver {
    async fn drive(&mut self) -> Result<(), RelayError> {
        loop {
            let has_tasks = !self.tasks.is_empty();
            tokio::select! {
                biased;
                frame = self.incoming.recv() => match frame {
                    Some(frame) => self.handle_input(frame).await?,
                    None => return Ok(()),
                },
                event = self.events.recv() => match event {
                    Some(event) => self.handle_event(event).await?,
                    None => return Ok(()),
                },
                joined = self.tasks.join_next(), if has_tasks => {
                    if matches!(joined, Some(Err(error)) if !error.is_cancelled()) {
                        return Err(RelayError::Unavailable);
                    }
                },
            }
        }
    }

    async fn handle_input(&mut self, frame: Frame) -> Result<(), RelayError> {
        validate_frame_shape(&frame)?;
        match frame {
            Frame::Request {
                id,
                path,
                method,
                headers,
                body,
            } => self.handle_request(id, path, method, headers, body).await,
            Frame::Credit { id, seq } => self.handle_credit(id, seq),
            Frame::Cancel { id } => self.handle_cancel(id),
            Frame::Response { .. } | Frame::Chunk { .. } | Frame::End { .. } => {
                Err(RelayError::InvalidInput)
            }
        }
    }

    async fn handle_request(
        &mut self,
        id: String,
        path: String,
        method: String,
        mut headers: HeaderMap,
        body: String,
    ) -> Result<(), RelayError> {
        let anonymous_probe =
            path == "/connector-info" && method == "GET" && !headers.contains_key("authorization");
        if (!anonymous_probe && headers.get("authorization") != Some(&self.backend_authorization))
            || (anonymous_probe && headers.contains_key("authorization"))
        {
            return Err(RelayError::Unauthorized);
        }
        if self.pending.contains_key(&id) || self.tombstones.contains(&id) {
            return Err(RelayError::InvalidInput);
        }

        if self.pending.len() >= MAX_PENDING {
            self.tombstones.insert(id.clone());
            self.send_frame(Frame::Cancel { id }).await?;
            return Ok(());
        }

        let body = decode_body(&body).map_err(|_| RelayError::InvalidInput)?;
        let method = match method.as_str() {
            "GET" => Method::GET,
            "POST" => Method::POST,
            "DELETE" => Method::DELETE,
            _ => return Err(RelayError::InvalidInput),
        };
        let port = route(&mut headers, self.local_port, self.write_route.get());
        let url = format!("http://127.0.0.1:{port}{path}");
        let (command_sender, command_receiver) = mpsc::channel(1);
        let tag = self.next_tag;
        self.next_tag = self.next_tag.checked_add(1).unwrap_or(1);
        let task_sender = self.event_sender.clone();
        let client = self.client.clone();
        let request_id = id.clone();
        let abort = self.tasks.spawn(async move {
            run_request_task(
                tag,
                request_id,
                client,
                LocalRequest {
                    method,
                    url,
                    headers,
                    body,
                },
                command_receiver,
                task_sender,
            )
            .await;
            TaskExit { _tag: tag }
        });
        self.pending.insert(
            id,
            Pending {
                tag,
                command: command_sender,
                abort,
                phase: Phase::AwaitingHeaders,
                next_sequence: 0,
                bytes: 0,
                chunks: 0,
            },
        );
        Ok(())
    }

    fn handle_credit(&mut self, id: String, sequence: u64) -> Result<(), RelayError> {
        let Some(pending) = self.pending.get_mut(&id) else {
            if self.tombstones.contains(&id) {
                return Ok(());
            }
            return Err(RelayError::InvalidInput);
        };
        if pending.phase != Phase::WaitingCredit || pending.next_sequence != sequence {
            return Err(RelayError::InvalidInput);
        }
        if pending
            .command
            .try_send(TaskCommand::Credit(sequence))
            .is_err()
        {
            return Err(RelayError::Unavailable);
        }
        pending.phase = Phase::CreditInFlight;
        Ok(())
    }

    fn handle_cancel(&mut self, id: String) -> Result<(), RelayError> {
        if self.pending.contains_key(&id) {
            self.retire(&id, None);
            return Ok(());
        }
        if self.tombstones.contains(&id) {
            return Ok(());
        }
        Err(RelayError::InvalidInput)
    }

    async fn handle_event(&mut self, event: TaskEvent) -> Result<(), RelayError> {
        match event {
            TaskEvent::Headers {
                tag,
                id,
                status,
                headers,
                body_allowed,
            } => {
                self.handle_headers(tag, id, status, headers, body_allowed)
                    .await
            }
            TaskEvent::Chunk {
                tag,
                id,
                sequence,
                bytes,
            } => self.handle_chunk(tag, id, sequence, bytes).await,
            TaskEvent::End { tag, id } => self.handle_end(tag, id).await,
            TaskEvent::Failure { tag, id } => self.handle_failure(tag, id).await,
        }
    }

    async fn handle_headers(
        &mut self,
        tag: u64,
        id: String,
        status: u16,
        headers: HeaderMap,
        body_allowed: bool,
    ) -> Result<(), RelayError> {
        let Some(pending) = self.pending.get(&id) else {
            return Ok(());
        };
        if pending.tag != tag {
            return Ok(());
        }
        if pending.phase != Phase::AwaitingHeaders {
            return Err(RelayError::InvalidInput);
        }
        self.send_frame(Frame::Response {
            id: id.clone(),
            status,
            headers,
        })
        .await?;
        if body_allowed {
            if let Some(pending) = self.pending.get_mut(&id) {
                if pending.tag == tag {
                    pending.phase = Phase::WaitingCredit;
                }
            }
        } else {
            if let Some(pending) = self.pending.get_mut(&id) {
                if pending.tag == tag {
                    pending.phase = Phase::NoBody;
                }
            }
            self.send_frame(Frame::End { id: id.clone() }).await?;
            self.retire(&id, Some(tag));
        }
        Ok(())
    }

    async fn handle_chunk(
        &mut self,
        tag: u64,
        id: String,
        sequence: u64,
        bytes: Vec<u8>,
    ) -> Result<(), RelayError> {
        let Some(pending) = self.pending.get(&id) else {
            return Ok(());
        };
        if pending.tag != tag {
            return Ok(());
        }
        if pending.phase != Phase::CreditInFlight || pending.next_sequence != sequence {
            return self.fail_request(id, tag).await;
        }
        if bytes.is_empty()
            || bytes.len() > MAX_CHUNK_BODY_BYTES
            || pending.bytes.saturating_add(bytes.len()) > MAX_RESPONSE_BYTES
            || pending.chunks >= MAX_RESPONSE_CHUNKS
        {
            return self.fail_request(id, tag).await;
        }
        let body = encode_body(&bytes).map_err(|_| RelayError::InvalidResponse)?;
        self.send_frame(Frame::Chunk {
            id: id.clone(),
            seq: sequence,
            body,
        })
        .await?;
        if let Some(pending) = self.pending.get_mut(&id) {
            if pending.tag == tag {
                pending.bytes += bytes.len();
                pending.chunks += 1;
                pending.next_sequence += 1;
                pending.phase = Phase::WaitingCredit;
            }
        }
        Ok(())
    }

    async fn handle_end(&mut self, tag: u64, id: String) -> Result<(), RelayError> {
        let Some(pending) = self.pending.get(&id) else {
            return Ok(());
        };
        if pending.tag != tag {
            return Ok(());
        }
        if pending.phase != Phase::CreditInFlight {
            return self.fail_request(id, tag).await;
        }
        self.send_frame(Frame::End { id: id.clone() }).await?;
        self.retire(&id, Some(tag));
        Ok(())
    }

    async fn handle_failure(&mut self, tag: u64, id: String) -> Result<(), RelayError> {
        if self
            .pending
            .get(&id)
            .is_some_and(|pending| pending.tag == tag)
        {
            self.fail_request(id, tag).await
        } else {
            Ok(())
        }
    }

    async fn fail_request(&mut self, id: String, tag: u64) -> Result<(), RelayError> {
        self.retire(&id, Some(tag));
        self.send_frame(Frame::Cancel { id }).await
    }

    fn retire(&mut self, id: &str, expected_tag: Option<u64>) {
        let should_retire = self
            .pending
            .get(id)
            .is_some_and(|pending| expected_tag.is_none_or(|tag| pending.tag == tag));
        if !should_retire {
            return;
        }
        if let Some(pending) = self.pending.remove(id) {
            pending.abort.abort();
        }
        self.tombstones.insert(id.to_owned());
    }

    async fn send_frame(&self, frame: Frame) -> Result<(), RelayError> {
        encode_frame(&frame).map_err(|_| RelayError::InvalidInput)?;
        timeout(OUTGOING_SEND_TIMEOUT, self.outgoing.send(frame))
            .await
            .map_err(|_| RelayError::Unavailable)?
            .map_err(|_| RelayError::Unavailable)
    }
}

struct LocalRequest {
    method: Method,
    url: String,
    headers: HeaderMap,
    body: Vec<u8>,
}

async fn run_request_task(
    tag: u64,
    id: String,
    client: Client,
    request: LocalRequest,
    mut commands: mpsc::Receiver<TaskCommand>,
    events: EventSender,
) {
    let result = timeout(
        HTTP_REQUEST_TIMEOUT,
        execute_request(tag, &id, client, request, &mut commands, &events),
    )
    .await;
    if !matches!(result, Ok(Ok(()))) {
        let _ = timeout(
            OUTGOING_SEND_TIMEOUT,
            events.send(TaskEvent::Failure { tag, id }),
        )
        .await;
    }
}

async fn execute_request(
    tag: u64,
    id: &str,
    client: Client,
    request: LocalRequest,
    commands: &mut mpsc::Receiver<TaskCommand>,
    events: &EventSender,
) -> Result<(), ()> {
    let LocalRequest {
        method,
        url,
        headers,
        body,
    } = request;
    let mut request = client.request(method, url);
    for (name, value) in headers {
        request = request.header(name, value);
    }
    let mut response = request.body(body).send().await.map_err(|_| ())?;
    let status = response.status().as_u16();
    if !(200..=599).contains(&status) {
        return Err(());
    }
    let headers = filtered_response_headers(&response);
    let body_allowed = !matches!(status, 204 | 205 | 304);
    events
        .send(TaskEvent::Headers {
            tag,
            id: id.to_owned(),
            status,
            headers,
            body_allowed,
        })
        .await
        .map_err(|_| ())?;

    if !body_allowed {
        events
            .send(TaskEvent::End {
                tag,
                id: id.to_owned(),
            })
            .await
            .map_err(|_| ())?;
        return Ok(());
    }

    let mut buffered = Vec::new();
    let mut offset = 0;
    let mut sequence = 0_u64;
    let mut bytes_total = 0_usize;
    let mut chunks_total = 0_usize;
    loop {
        match commands.recv().await {
            Some(TaskCommand::Credit(credit_sequence)) if credit_sequence == sequence => {}
            _ => return Err(()),
        }

        while offset == buffered.len() {
            let Some(chunk) = response.chunk().await.map_err(|_| ())? else {
                events
                    .send(TaskEvent::End {
                        tag,
                        id: id.to_owned(),
                    })
                    .await
                    .map_err(|_| ())?;
                return Ok(());
            };
            if !chunk.is_empty() {
                if chunk.len() > MAX_RESPONSE_BYTES.saturating_sub(bytes_total) {
                    return Err(());
                }
                buffered = chunk.to_vec();
                offset = 0;
            }
        }

        let take = (buffered.len() - offset).min(MAX_CHUNK_BODY_BYTES);
        let next_total = bytes_total.checked_add(take).ok_or(())?;
        chunks_total = chunks_total.checked_add(1).ok_or(())?;
        if next_total > MAX_RESPONSE_BYTES || chunks_total > MAX_RESPONSE_CHUNKS {
            return Err(());
        }
        let chunk = buffered[offset..offset + take].to_vec();
        offset += take;
        bytes_total = next_total;
        events
            .send(TaskEvent::Chunk {
                tag,
                id: id.to_owned(),
                sequence,
                bytes: chunk,
            })
            .await
            .map_err(|_| ())?;
        sequence = sequence.checked_add(1).ok_or(())?;
    }
}

fn filtered_response_headers(response: &Response) -> HeaderMap {
    ["content-type", "mcp-session-id", "mcp-protocol-version"]
        .into_iter()
        .filter_map(|name| {
            let value = response.headers().get(name)?.to_str().ok()?;
            if value.len() > MAX_HEADER_VALUE_BYTES
                || !value
                    .bytes()
                    .all(|byte| byte == b'\t' || (0x20..=0x7e).contains(&byte))
            {
                return None;
            }
            Some((name.to_owned(), value.to_owned()))
        })
        .collect()
}

fn validate_frame_shape(frame: &Frame) -> Result<(), RelayError> {
    encode_frame(frame)
        .map(|_| ())
        .map_err(|_| RelayError::InvalidInput)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::{oneshot, Mutex, Notify};
    use tokio::task::JoinHandle;

    const TOKEN: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[derive(Clone, Debug)]
    struct CapturedRequest {
        method: String,
        path: String,
        headers: HashMap<String, String>,
        body: Vec<u8>,
    }

    struct TestServer {
        port: u16,
        requests: Arc<Mutex<Vec<CapturedRequest>>>,
        received: Arc<Notify>,
        peer_closed: Arc<Notify>,
        stop: Option<oneshot::Sender<()>>,
        task: Option<JoinHandle<()>>,
    }

    impl TestServer {
        async fn wait_for_requests(&self, count: usize) {
            timeout(Duration::from_secs(2), async {
                loop {
                    if self.requests.lock().await.len() >= count {
                        return;
                    }
                    self.received.notified().await;
                }
            })
            .await
            .expect("server request deadline");
        }

        async fn wait_for_peer_close(&self) {
            timeout(Duration::from_secs(2), self.peer_closed.notified())
                .await
                .expect("loopback close deadline");
        }

        async fn stop(mut self) {
            if let Some(stop) = self.stop.take() {
                let _ = stop.send(());
            }
            if let Some(task) = self.task.take() {
                let _ = task.await;
            }
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            if let Some(stop) = self.stop.take() {
                let _ = stop.send(());
            }
            if let Some(task) = self.task.take() {
                task.abort();
            }
        }
    }

    async fn test_server() -> TestServer {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind loopback server");
        let port = listener.local_addr().expect("server address").port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (stop, mut stopped) = oneshot::channel();
        let stop_notify = Arc::new(Notify::new());
        let received = Arc::new(Notify::new());
        let peer_closed = Arc::new(Notify::new());
        let task_requests = requests.clone();
        let task_notify = stop_notify.clone();
        let task_received = received.clone();
        let task_peer_closed = peer_closed.clone();
        let task = tokio::spawn(async move {
            let mut children = JoinSet::new();
            loop {
                tokio::select! {
                    _ = &mut stopped => break,
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { break };
                        children.spawn(handle_connection(
                            stream,
                            task_requests.clone(),
                            task_notify.clone(),
                            task_received.clone(),
                            task_peer_closed.clone(),
                        ));
                    },
                    joined = children.join_next(), if !children.is_empty() => {
                        let _ = joined;
                    },
                }
            }
            stop_notify.notify_waiters();
            children.abort_all();
            while children.join_next().await.is_some() {}
        });
        TestServer {
            port,
            requests,
            received,
            peer_closed,
            stop: Some(stop),
            task: Some(task),
        }
    }

    async fn handle_connection(
        mut stream: TcpStream,
        requests: Arc<Mutex<Vec<CapturedRequest>>>,
        stop: Arc<Notify>,
        received: Arc<Notify>,
        peer_closed: Arc<Notify>,
    ) {
        let mut bytes = Vec::new();
        let header_end = loop {
            let mut read = [0_u8; 4096];
            let Ok(count) = stream.read(&mut read).await else {
                return;
            };
            if count == 0 {
                return;
            }
            bytes.extend_from_slice(&read[..count]);
            if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                break end + 4;
            }
        };
        let head = String::from_utf8_lossy(&bytes[..header_end - 4]).into_owned();
        let mut lines = head.split("\r\n");
        let Some(request_line) = lines.next() else {
            return;
        };
        let mut request_parts = request_line.split_whitespace();
        let Some(method) = request_parts.next() else {
            return;
        };
        let Some(path) = request_parts.next() else {
            return;
        };
        let headers: HashMap<String, String> = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
            .collect();
        let content_length = headers
            .get("content-length")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        while bytes.len() < header_end.saturating_add(content_length) {
            let mut read = [0_u8; 4096];
            let Ok(count) = stream.read(&mut read).await else {
                return;
            };
            if count == 0 {
                return;
            }
            bytes.extend_from_slice(&read[..count]);
        }
        let body = bytes[header_end..header_end + content_length].to_vec();
        requests.lock().await.push(CapturedRequest {
            method: method.to_owned(),
            path: path.to_owned(),
            headers,
            body: body.clone(),
        });
        received.notify_one();

        if body == b"hold-before" {
            let mut probe = [0_u8; 1];
            tokio::select! {
                _ = stop.notified() => {}
                result = stream.read(&mut probe) => {
                    let _ = result;
                    peer_closed.notify_one();
                }
            }
            return;
        }
        if body == b"fail" {
            return;
        }
        let (status, response_body, hold_after) = match (path, body.as_slice()) {
            ("/connector-info", _) => (204, Vec::new(), false),
            ("/mcp", b"stream") => (200, b"abc".to_vec(), false),
            ("/mcp", b"hold-after") => (200, b"late".to_vec(), true),
            ("/mcp", b"oversize") => (200, vec![b'x'; MAX_RESPONSE_BYTES + 1], false),
            ("/mcp", _) => (204, Vec::new(), false),
            _ => (404, Vec::new(), false),
        };
        let reason = if status == 200 { "OK" } else { "No Content" };
        let response = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nX-Private: hidden\r\nConnection: close\r\n\r\n",
            response_body.len()
        );
        if stream.write_all(response.as_bytes()).await.is_err()
            || stream.write_all(&response_body).await.is_err()
        {
            return;
        }
        if hold_after {
            stop.notified().await;
        }
    }

    fn request(id: &str, path: &str, method: &str, headers: &[(&str, &str)], body: &[u8]) -> Frame {
        Frame::Request {
            id: id.to_owned(),
            path: path.to_owned(),
            method: method.to_owned(),
            headers: headers
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect(),
            body: encode_body(body).expect("test body is bounded"),
        }
    }

    fn protected_request(id: &str, path: &str, method: &str, body: &[u8]) -> Frame {
        let authorization = format!("Bearer {TOKEN}");
        request(id, path, method, &[("authorization", &authorization)], body)
    }

    async fn peer(
        port: u16,
    ) -> (
        mpsc::Sender<Frame>,
        mpsc::Receiver<Frame>,
        JoinHandle<Result<(), RelayError>>,
    ) {
        routed_peer(port, WriteRoute::default()).await
    }

    async fn routed_peer(
        port: u16,
        write_route: WriteRoute,
    ) -> (
        mpsc::Sender<Frame>,
        mpsc::Receiver<Frame>,
        JoinHandle<Result<(), RelayError>>,
    ) {
        let (incoming, input) = mpsc::channel(32);
        let (output, outgoing) = mpsc::channel(32);
        let task = tokio::spawn(run_peer(port, write_route, TOKEN.to_owned(), input, output));
        (incoming, outgoing, task)
    }

    fn write_request(id: &str) -> Frame {
        let authorization = format!("Bearer {TOKEN}");
        request(
            id,
            "/mcp",
            "POST",
            &[("authorization", &authorization), (ACCESS_HEADER, "write")],
            b"tool-call",
        )
    }

    /// Send one frame and wait until it has been answered and ended.
    async fn complete(rx: &mut mpsc::Receiver<Frame>, id: &str) {
        assert!(matches!(next_frame(rx).await, Frame::Response { .. }));
        assert_eq!(next_frame(rx).await, Frame::End { id: id.to_owned() });
    }

    #[test]
    fn routing_picks_the_write_port_only_for_the_exact_word_and_always_strips_the_header() {
        let headers = |value: Option<&str>| {
            let mut headers = HeaderMap::new();
            headers.insert("accept".into(), "application/json".into());
            if let Some(value) = value {
                headers.insert(ACCESS_HEADER.into(), value.into());
            }
            headers
        };
        for (value, write_port, expected) in [
            (Some("write"), Some(18081), 18081),
            (Some("write"), None, 18080),
            (Some("query"), Some(18081), 18080),
            (Some("WRITE"), Some(18081), 18080),
            (Some(""), Some(18081), 18080),
            (None, Some(18081), 18080),
            (None, None, 18080),
        ] {
            let mut map = headers(value);
            assert_eq!(route(&mut map, 18080, write_port), expected, "{value:?}");
            assert!(!map.contains_key(ACCESS_HEADER), "{value:?}");
            assert_eq!(
                map.get("accept").map(String::as_str),
                Some("application/json")
            );
        }
    }

    #[test]
    fn write_route_is_shared_between_clones_and_empty_by_default() {
        let route = WriteRoute::default();
        let seen_by_connection = route.clone();
        assert_eq!(seen_by_connection.get(), None);
        route.set(Some(18081));
        assert_eq!(seen_by_connection.get(), Some(18081));
        route.set(None);
        assert_eq!(seen_by_connection.get(), None);
    }

    #[tokio::test]
    async fn a_write_request_goes_to_the_write_server_without_the_routing_header() {
        let query = test_server().await;
        let write = test_server().await;
        let route = WriteRoute::default();
        route.set(Some(write.port));
        let (tx, mut rx, task) = routed_peer(query.port, route).await;

        tx.send(write_request("write")).await.unwrap();
        complete(&mut rx, "write").await;

        assert!(query.requests.lock().await.is_empty());
        let received = write.requests.lock().await.clone();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].path, "/mcp");
        assert_eq!(received[0].body, b"tool-call");
        assert_eq!(
            received[0].headers.get("authorization"),
            Some(&format!("Bearer {TOKEN}"))
        );
        assert!(!received[0].headers.contains_key(ACCESS_HEADER));

        drop(tx);
        assert_eq!(peer_result(task).await, Ok(()));
        query.stop().await;
        write.stop().await;
    }

    #[tokio::test]
    async fn a_write_request_falls_back_to_the_query_server_when_no_write_server_runs() {
        let query = test_server().await;
        let (tx, mut rx, task) = routed_peer(query.port, WriteRoute::default()).await;

        tx.send(write_request("write")).await.unwrap();
        complete(&mut rx, "write").await;

        let received = query.requests.lock().await.clone();
        assert_eq!(received.len(), 1);
        assert!(!received[0].headers.contains_key(ACCESS_HEADER));

        drop(tx);
        assert_eq!(peer_result(task).await, Ok(()));
        query.stop().await;
    }

    #[tokio::test]
    async fn a_request_without_the_header_always_goes_to_the_query_server() {
        let query = test_server().await;
        let write = test_server().await;
        let route = WriteRoute::default();
        route.set(Some(write.port));
        let (tx, mut rx, task) = routed_peer(query.port, route.clone()).await;

        tx.send(protected_request("search", "/mcp", "POST", b"search"))
            .await
            .unwrap();
        complete(&mut rx, "search").await;

        assert_eq!(query.requests.lock().await.len(), 1);
        assert!(write.requests.lock().await.is_empty());

        // The write server stopping mid connection sends the next write request
        // to the query server, which refuses it, without reopening anything.
        route.set(None);
        tx.send(write_request("after-stop")).await.unwrap();
        complete(&mut rx, "after-stop").await;
        assert_eq!(query.requests.lock().await.len(), 2);
        assert!(write.requests.lock().await.is_empty());

        drop(tx);
        assert_eq!(peer_result(task).await, Ok(()));
        query.stop().await;
        write.stop().await;
    }

    async fn next_frame(receiver: &mut mpsc::Receiver<Frame>) -> Frame {
        timeout(Duration::from_secs(2), receiver.recv())
            .await
            .expect("frame deadline")
            .expect("peer output closed")
    }

    async fn peer_result(task: JoinHandle<Result<(), RelayError>>) -> Result<(), RelayError> {
        timeout(Duration::from_secs(2), task)
            .await
            .expect("peer shutdown deadline")
            .expect("peer task panicked")
    }

    #[test]
    fn tombstones_remain_bounded() {
        let mut tombstones = Tombstones::new();
        for index in 0..=TOMBSTONE_LIMIT {
            tombstones.insert(format!("id-{index}"));
        }
        assert!(!tombstones.contains("id-0"));
        assert!(tombstones.contains("id-1"));
        assert!(tombstones.contains(&format!("id-{TOMBSTONE_LIMIT}")));
    }

    #[tokio::test]
    async fn validates_peer_inputs_before_starting_any_http_task() {
        let (_tx, input) = mpsc::channel(1);
        let (output, _rx) = mpsc::channel(1);
        assert_eq!(
            run_peer(0, WriteRoute::default(), TOKEN.to_owned(), input, output).await,
            Err(RelayError::InvalidInput)
        );

        let (_tx, input) = mpsc::channel(1);
        let (output, _rx) = mpsc::channel(1);
        assert_eq!(
            run_peer(1, WriteRoute::default(), "short".to_owned(), input, output).await,
            Err(RelayError::InvalidInput)
        );
    }

    #[tokio::test]
    async fn forwards_anonymous_probe_protected_auth_and_body_without_private_headers() {
        let server = test_server().await;
        let (tx, mut rx, task) = peer(server.port).await;

        tx.send(request("anonymous", "/connector-info", "GET", &[], &[]))
            .await
            .unwrap();
        let response = next_frame(&mut rx).await;
        assert!(matches!(response, Frame::Response { status: 204, .. }));
        assert_eq!(
            next_frame(&mut rx).await,
            Frame::End {
                id: "anonymous".into()
            }
        );

        let body = b"passthrough-body";
        let authorization = format!("Bearer {TOKEN}");
        tx.send(request(
            "protected",
            "/mcp",
            "POST",
            &[
                ("authorization", &authorization),
                ("content-type", "application/json"),
            ],
            body,
        ))
        .await
        .unwrap();
        let response = next_frame(&mut rx).await;
        let Frame::Response {
            headers, status, ..
        } = response
        else {
            panic!("expected response")
        };
        assert_eq!(status, 204);
        assert_eq!(
            headers.get("content-type"),
            Some(&"application/json".to_owned())
        );
        assert!(!headers.contains_key("x-private"));
        assert_eq!(
            next_frame(&mut rx).await,
            Frame::End {
                id: "protected".into()
            }
        );

        let requests = server.requests.lock().await.clone();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].method, "GET");
        assert_eq!(requests[0].path, "/connector-info");
        assert!(!requests[0].headers.contains_key("authorization"));
        assert_eq!(
            requests[1].headers.get("authorization"),
            Some(&authorization)
        );
        assert_eq!(requests[1].body, body);

        drop(tx);
        assert_eq!(peer_result(task).await, Ok(()));
        server.stop().await;
    }

    #[tokio::test]
    async fn immediate_credits_drive_incremental_chunks_and_final_end() {
        let server = test_server().await;
        let (tx, mut rx, task) = peer(server.port).await;
        tx.send(protected_request("stream", "/mcp", "POST", b"stream"))
            .await
            .unwrap();
        assert!(matches!(
            next_frame(&mut rx).await,
            Frame::Response { status: 200, .. }
        ));

        // The first credit is sent immediately after observing the header.
        tx.send(Frame::Credit {
            id: "stream".into(),
            seq: 0,
        })
        .await
        .unwrap();
        let Frame::Chunk { id, seq, body } = next_frame(&mut rx).await else {
            panic!("expected first chunk")
        };
        assert_eq!((id, seq), ("stream".to_owned(), 0));
        assert_eq!(decode_body(&body).unwrap(), b"abc");

        // This credit follows the accepted chunk send without a scheduler
        // sleep, proving that a valid next credit is not rejected as a race.
        tx.send(Frame::Credit {
            id: "stream".into(),
            seq: 1,
        })
        .await
        .unwrap();
        assert_eq!(
            next_frame(&mut rx).await,
            Frame::End {
                id: "stream".into()
            }
        );
        drop(tx);
        assert_eq!(peer_result(task).await, Ok(()));
        server.stop().await;
    }

    #[tokio::test]
    async fn rejects_early_and_duplicate_credit_as_peer_protocol_errors() {
        let server = test_server().await;
        let (tx, _rx, task) = peer(server.port).await;
        tx.send(Frame::Credit {
            id: "unknown".into(),
            seq: 0,
        })
        .await
        .unwrap();
        assert_eq!(peer_result(task).await, Err(RelayError::InvalidInput));
        server.stop().await;

        let server = test_server().await;
        let (tx, mut rx, task) = peer(server.port).await;
        tx.send(protected_request("duplicate", "/mcp", "POST", b"stream"))
            .await
            .unwrap();
        assert!(matches!(next_frame(&mut rx).await, Frame::Response { .. }));
        tx.send(Frame::Credit {
            id: "duplicate".into(),
            seq: 0,
        })
        .await
        .unwrap();
        tx.send(Frame::Credit {
            id: "duplicate".into(),
            seq: 0,
        })
        .await
        .unwrap();
        assert_eq!(peer_result(task).await, Err(RelayError::InvalidInput));
        server.stop().await;
    }

    #[tokio::test]
    async fn cancellation_before_and_after_headers_does_not_echo_or_leak_output() {
        let server = test_server().await;
        let (tx, mut rx, task) = peer(server.port).await;
        tx.send(protected_request("before", "/mcp", "POST", b"hold-before"))
            .await
            .unwrap();
        tx.send(Frame::Cancel {
            id: "before".into(),
        })
        .await
        .unwrap();
        assert!(timeout(Duration::from_millis(100), rx.recv())
            .await
            .is_err());
        drop(tx);
        assert_eq!(peer_result(task).await, Ok(()));
        server.stop().await;

        let server = test_server().await;
        let (tx, mut rx, task) = peer(server.port).await;
        tx.send(protected_request("after", "/mcp", "POST", b"hold-after"))
            .await
            .unwrap();
        assert!(matches!(next_frame(&mut rx).await, Frame::Response { .. }));
        tx.send(Frame::Cancel { id: "after".into() }).await.unwrap();
        assert!(timeout(Duration::from_millis(100), rx.recv())
            .await
            .is_err());
        drop(tx);
        assert_eq!(peer_result(task).await, Ok(()));
        server.stop().await;
    }

    #[tokio::test]
    async fn local_failure_cancels_only_that_request() {
        let server = test_server().await;
        let (tx, mut rx, task) = peer(server.port).await;
        tx.send(protected_request("failure", "/mcp", "POST", b"fail"))
            .await
            .unwrap();
        tx.send(protected_request("survivor", "/mcp", "POST", b"stream"))
            .await
            .unwrap();

        let mut saw_failure = false;
        let mut saw_survivor = false;
        for _ in 0..2 {
            match next_frame(&mut rx).await {
                Frame::Cancel { id } if id == "failure" => saw_failure = true,
                Frame::Response { id, status, .. } if id == "survivor" && status == 200 => {
                    saw_survivor = true
                }
                frame => panic!("unexpected frame: {frame:?}"),
            }
        }
        assert!(saw_failure && saw_survivor);
        tx.send(Frame::Credit {
            id: "survivor".into(),
            seq: 0,
        })
        .await
        .unwrap();
        assert!(
            matches!(next_frame(&mut rx).await, Frame::Chunk { id, seq: 0, .. } if id == "survivor")
        );
        tx.send(Frame::Credit {
            id: "survivor".into(),
            seq: 1,
        })
        .await
        .unwrap();
        assert_eq!(
            next_frame(&mut rx).await,
            Frame::End {
                id: "survivor".into()
            }
        );
        drop(tx);
        assert_eq!(peer_result(task).await, Ok(()));
        server.stop().await;
    }

    #[tokio::test]
    async fn caps_pending_requests_without_starting_a_ninth_http_task() {
        let server = test_server().await;
        let (tx, mut rx, task) = peer(server.port).await;
        for index in 0..MAX_PENDING {
            tx.send(protected_request(
                &format!("pending-{index}"),
                "/mcp",
                "POST",
                b"hold-before",
            ))
            .await
            .unwrap();
        }
        server.wait_for_requests(MAX_PENDING).await;
        tx.send(protected_request(
            "overflow",
            "/mcp",
            "POST",
            b"hold-before",
        ))
        .await
        .unwrap();
        assert_eq!(
            next_frame(&mut rx).await,
            Frame::Cancel {
                id: "overflow".into()
            }
        );
        assert_eq!(server.requests.lock().await.len(), MAX_PENDING);
        drop(tx);
        assert_eq!(peer_result(task).await, Ok(()));
        server.stop().await;
    }

    #[tokio::test]
    async fn caps_each_response_at_two_mib_and_sends_generic_cancel() {
        let server = test_server().await;
        let (tx, mut rx, task) = peer(server.port).await;
        tx.send(protected_request("oversize", "/mcp", "POST", b"oversize"))
            .await
            .unwrap();
        assert!(matches!(
            next_frame(&mut rx).await,
            Frame::Response { status: 200, .. }
        ));

        let mut total = 0_usize;
        let mut chunks = 0_usize;
        let mut cancelled = false;
        for sequence in 0..=MAX_RESPONSE_CHUNKS as u64 {
            tx.send(Frame::Credit {
                id: "oversize".into(),
                seq: sequence,
            })
            .await
            .unwrap();
            match next_frame(&mut rx).await {
                Frame::Chunk { body, .. } => {
                    let bytes = decode_body(&body).unwrap();
                    total += bytes.len();
                    chunks += 1;
                    assert!(total <= MAX_RESPONSE_BYTES);
                    assert!(chunks <= MAX_RESPONSE_CHUNKS);
                }
                Frame::Cancel { id } => {
                    assert_eq!(id, "oversize");
                    cancelled = true;
                    break;
                }
                frame => panic!("unexpected frame: {frame:?}"),
            }
        }
        assert!(cancelled);
        drop(tx);
        assert_eq!(peer_result(task).await, Ok(()));
        server.stop().await;
    }

    #[tokio::test]
    async fn forbidden_route_or_credential_never_reaches_loopback() {
        let server = test_server().await;
        let (tx, _rx, task) = peer(server.port).await;
        tx.send(Frame::Request {
            id: "route".into(),
            path: "http://attacker.invalid/mcp".into(),
            method: "GET".into(),
            headers: HeaderMap::new(),
            body: String::new(),
        })
        .await
        .unwrap();
        assert_eq!(peer_result(task).await, Err(RelayError::InvalidInput));
        assert!(server.requests.lock().await.is_empty());
        server.stop().await;

        let server = test_server().await;
        let (tx, _rx, task) = peer(server.port).await;
        tx.send(request("auth", "/mcp", "POST", &[], b"body"))
            .await
            .unwrap();
        assert_eq!(peer_result(task).await, Err(RelayError::Unauthorized));
        assert!(server.requests.lock().await.is_empty());
        server.stop().await;
    }

    #[tokio::test]
    async fn input_shutdown_aborts_owned_http_tasks() {
        let server = test_server().await;
        let (tx, _rx, task) = peer(server.port).await;
        tx.send(protected_request(
            "shutdown",
            "/mcp",
            "POST",
            b"hold-before",
        ))
        .await
        .unwrap();
        server.wait_for_requests(1).await;
        drop(tx);
        assert_eq!(peer_result(task).await, Ok(()));
        server.wait_for_peer_close().await;
        server.stop().await;
    }
}
