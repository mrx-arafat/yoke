//! Original, deliberately bounded HTTP/1.1 transport for the loopback console.
//! Only GET/POST origin-form requests and connection-close responses are supported.

use std::{
    io::{self, Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const HEADER_CAP: usize = 16 * 1024;
const BODY_CAP: usize = 64 * 1024;
const RESPONSE_CAP: usize = 16 * 1024 * 1024;
const STREAM_CAP: usize = 1024 * 1024;
const CONNECTION_CAP: usize = 16;
const IO_TIME: Duration = Duration::from_secs(3);
const STREAM_TIME: Duration = Duration::from_secs(60);

pub(super) struct Server {
    address: SocketAddr,
    requests: mpsc::Receiver<Request>,
    stopped: Arc<AtomicBool>,
    acceptor: Option<JoinHandle<()>>,
}

impl Server {
    pub(super) fn bind_loopback() -> io::Result<Self> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        let address = listener.local_addr()?;
        listener.set_nonblocking(true)?;
        let (sender, requests) = mpsc::sync_channel(CONNECTION_CAP);
        let stopped = Arc::new(AtomicBool::new(false));
        let stopping = stopped.clone();
        let active = Arc::new(AtomicUsize::new(0));
        let acceptor = thread::Builder::new().name("yoke-http-accept".into()).spawn(move || {
            while !stopping.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        // Accepted sockets must use timeout-governed blocking I/O on all platforms.
                        if stream.set_nonblocking(false).is_err() {
                            continue;
                        }
                        let Some(slot) = Slot::acquire(&active) else {
                            let _ = stream.shutdown(Shutdown::Both);
                            continue;
                        };
                        let sender = sender.clone();
                        let deadline = Instant::now() + IO_TIME;
                        // A permit covers parsing, queueing, handling and streaming.
                        // Failed thread creation drops the closure and its permit.
                        let _ = thread::Builder::new().name("yoke-http-read".into()).spawn(move || {
                            let mut connection = Connection { stream, _slot: slot };
                            match parse(&mut connection.stream, deadline) {
                                Ok(parsed) => {
                                    let _ = sender.try_send(Request { connection, parsed });
                                }
                                Err(error) => {
                                    let status = if matches!(error.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock) { 408 } else { 400 };
                                    let message = format!("HTTP/1.1 {status} Error\r\nConnection: close\r\nContent-Length: 0\r\n\r\n");
                                    let _ = write_until(&mut connection.stream, message.as_bytes(), Instant::now() + IO_TIME);
                                }
                            }
                        });
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::park_timeout(Duration::from_millis(10)),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        })?;
        Ok(Self {
            address,
            requests,
            stopped,
            acceptor: Some(acceptor),
        })
    }

    pub(super) fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.address)
    }

    pub(super) fn recv(&self) -> io::Result<Request> {
        self.requests
            .recv()
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "console listener stopped"))
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(acceptor) = self.acceptor.take() {
            acceptor.thread().unpark();
            let _ = acceptor.join();
        }
    }
}

struct Slot(Arc<AtomicUsize>);
impl Slot {
    fn acquire(active: &Arc<AtomicUsize>) -> Option<Self> {
        active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < CONNECTION_CAP).then_some(n + 1)
            })
            .ok()?;
        Some(Self(active.clone()))
    }
}
impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

struct Connection {
    stream: TcpStream,
    _slot: Slot,
}
impl Drop for Connection {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

struct Parsed {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

pub(super) struct Request {
    connection: Connection,
    parsed: Parsed,
}
impl Request {
    pub(super) fn method(&self) -> &str {
        &self.parsed.method
    }
    pub(super) fn url(&self) -> &str {
        &self.parsed.url
    }
    pub(super) fn header(&self, name: &str) -> Option<&str> {
        self.parsed
            .headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
    pub(super) fn body(&self) -> &[u8] {
        &self.parsed.body
    }

    pub(super) fn respond(
        mut self,
        status: u16,
        headers: Vec<(String, String)>,
        bytes: &[u8],
    ) -> io::Result<()> {
        if bytes.len() > RESPONSE_CAP {
            return Err(invalid("response exceeds 16 MiB"));
        }
        let head = response_head(status, headers, Some(bytes.len()))?;
        let deadline = Instant::now() + IO_TIME;
        write_until(&mut self.connection.stream, &head, deadline)?;
        write_until(&mut self.connection.stream, bytes, deadline)
    }

    /// Call only after authentication; the console separately limits streams to four.
    pub(super) fn into_stream(
        mut self,
        status: u16,
        headers: Vec<(String, String)>,
    ) -> io::Result<EventStream> {
        let head = response_head(status, headers, None)?;
        let deadline = Instant::now() + STREAM_TIME;
        write_until(&mut self.connection.stream, &head, Instant::now() + IO_TIME)?;
        Ok(EventStream {
            connection: self.connection,
            deadline,
            remaining: STREAM_CAP,
        })
    }
}

pub(super) struct EventStream {
    connection: Connection,
    deadline: Instant,
    remaining: usize,
}
impl Write for EventStream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.remaining {
            return Err(invalid("event stream exceeds 1 MiB"));
        }
        let deadline = self.deadline.min(Instant::now() + IO_TIME);
        write_until(&mut self.connection.stream, bytes, deadline)?;
        self.remaining -= bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        remaining(self.deadline).map(|_| ())
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|v| !v.is_zero())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "absolute console I/O deadline elapsed",
            )
        })
}
fn read_until(stream: &mut TcpStream, bytes: &mut [u8], deadline: Instant) -> io::Result<usize> {
    loop {
        stream.set_read_timeout(Some(remaining(deadline)?))?;
        match stream.read(bytes) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => return result,
        }
    }
}
fn write_until(stream: &mut TcpStream, mut bytes: &[u8], deadline: Instant) -> io::Result<()> {
    remaining(deadline)?;
    while !bytes.is_empty() {
        stream.set_write_timeout(Some(remaining(deadline)?))?;
        match stream.write(bytes) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "console socket closed",
                ))
            }
            Ok(count) => bytes = &bytes[count..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    remaining(deadline).map(|_| ())
}

fn token(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}
fn field_value(value: &str) -> bool {
    value.bytes().all(|b| b == b'\t' || (32..=126).contains(&b))
}

fn parse(stream: &mut TcpStream, deadline: Instant) -> io::Result<Parsed> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    let end = loop {
        if let Some(at) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
            if at + 4 > HEADER_CAP {
                return Err(invalid("headers exceed 16 KiB"));
            }
            break at + 4;
        }
        if bytes.len() >= HEADER_CAP {
            return Err(invalid("headers exceed 16 KiB"));
        }
        let count = read_until(stream, &mut chunk, deadline)?;
        if count == 0 {
            return Err(invalid("incomplete request headers"));
        }
        bytes.extend_from_slice(&chunk[..count]);
    };
    let head =
        std::str::from_utf8(&bytes[..end - 4]).map_err(|_| invalid("headers must be ASCII"))?;
    let mut lines = head.split("\r\n");
    let request_line: Vec<_> = lines.next().unwrap_or("").split(' ').collect();
    if request_line.len() != 3
        || !matches!(request_line[0], "GET" | "POST")
        || request_line[2] != "HTTP/1.1"
        || !request_line[1].starts_with('/')
        || request_line[1].contains('#')
        || !request_line[1].bytes().all(|b| (33..=126).contains(&b))
    {
        return Err(invalid("unsupported request line"));
    }
    let method = request_line[0].to_owned();
    let url = request_line[1].to_owned();
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut length = 0usize;
    for line in lines {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| invalid("invalid header"))?;
        if !token(name) || !field_value(value) {
            return Err(invalid("invalid header syntax"));
        }
        let name = name.to_ascii_lowercase();
        // Authentication headers must not have ambiguous duplicate interpretations either.
        if headers.iter().any(|(key, _)| key == &name) {
            return Err(invalid("duplicate header"));
        }
        let value = value.trim_matches([' ', '\t']);
        if name == "transfer-encoding" {
            return Err(invalid("transfer encoding is unsupported"));
        }
        if name == "expect" {
            return Err(invalid("expect is unsupported"));
        }
        if name == "content-length" {
            if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                return Err(invalid("invalid content length"));
            }
            length = value
                .parse()
                .map_err(|_| invalid("invalid content length"))?;
            if length > BODY_CAP {
                return Err(invalid("body exceeds 64 KiB"));
            }
        }
        headers.push((name, value.to_owned()));
    }
    if !headers
        .iter()
        .any(|(name, value)| name == "host" && !value.is_empty())
    {
        return Err(invalid("host required"));
    }
    let mut body = bytes[end..].to_vec();
    if body.len() > length {
        return Err(invalid("unexpected bytes after request"));
    }
    while body.len() < length {
        let wanted = (length - body.len()).min(chunk.len());
        let count = read_until(stream, &mut chunk[..wanted], deadline)?;
        if count == 0 {
            return Err(invalid("incomplete request body"));
        }
        body.extend_from_slice(&chunk[..count]);
    }
    remaining(deadline)?;
    Ok(Parsed {
        method,
        url,
        headers,
        body,
    })
}

fn response_head(
    status: u16,
    headers: Vec<(String, String)>,
    length: Option<usize>,
) -> io::Result<Vec<u8>> {
    if !(200..=599).contains(&status) {
        return Err(invalid("invalid response status"));
    }
    let mut head = format!("HTTP/1.1 {status} Response\r\nConnection: close\r\n");
    if let Some(length) = length {
        head.push_str(&format!("Content-Length: {length}\r\n"));
    }
    for (name, value) in headers {
        if !token(&name)
            || !field_value(&value)
            || ["content-length", "transfer-encoding", "connection"]
                .iter()
                .any(|s| name.eq_ignore_ascii_case(s))
        {
            return Err(invalid("invalid response header"));
        }
        if head.len() + name.len() + value.len() + 6 > HEADER_CAP {
            return Err(invalid("response headers exceed 16 KiB"));
        }
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    Ok(head.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        (client, server)
    }

    #[test]
    fn rejects_ambiguous_framing_and_oversized_input() {
        for raw in [
            "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nContent-Length: 0\r\n\r\n"
                .to_owned(),
            "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\ncontent-length: 1\r\n\r\n"
                .into(),
            "POST / HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n".into(),
            "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 65537\r\n\r\n".into(),
            format!(
                "GET / HTTP/1.1\r\nHost: localhost\r\nX: {}\r\n\r\n",
                "a".repeat(HEADER_CAP)
            ),
            "GET / HTTP/1.1\r\nHost: localhost\r\n folded: bad\r\n\r\n".into(),
        ] {
            let (mut client, mut server) = pair();
            client.write_all(raw.as_bytes()).unwrap();
            assert!(parse(&mut server, Instant::now() + IO_TIME).is_err());
        }
    }

    #[test]
    fn slow_trickle_cannot_extend_absolute_read_or_write_deadline() {
        let (mut client, mut server) = pair();
        let sender = thread::spawn(move || {
            for byte in b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n" {
                if client.write_all(&[*byte]).is_err() {
                    break;
                }
                // Deliberate real timing: traffic arrives faster than the timeout.
                thread::sleep(Duration::from_millis(10));
            }
        });
        let start = Instant::now();
        assert!(parse(&mut server, start + Duration::from_millis(70)).is_err());
        assert!(start.elapsed() < Duration::from_secs(1));
        drop(server);
        sender.join().unwrap();
        let (_client, mut server) = pair();
        let start = Instant::now();
        assert!(write_until(
            &mut server,
            &vec![0; RESPONSE_CAP],
            start + Duration::from_millis(70)
        )
        .is_err());
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn stream_and_slow_client_leave_routes_available_and_connections_bounded() {
        let server = Server::bind_loopback().unwrap();
        let address = server.local_addr().unwrap();
        assert!(address.ip().is_loopback());
        let _slow = TcpStream::connect(address).unwrap();
        let mut events = TcpStream::connect(address).unwrap();
        events
            .write_all(b"GET /events HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let request = server.recv().unwrap();
        assert_eq!(request.url(), "/events");
        let mut stream = request
            .into_stream(
                200,
                vec![("Content-Type".into(), "text/event-stream".into())],
            )
            .unwrap();
        stream.write_all(b"data: ready\n\n").unwrap();
        let mut client = TcpStream::connect(address).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        client
            .write_all(b"POST /action HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2\r\n\r\n{}")
            .unwrap();
        let request = server.recv().unwrap();
        assert_eq!(request.method(), "POST");
        assert_eq!(request.header("HOST"), Some("localhost"));
        assert_eq!(request.body(), b"{}");
        request.respond(200, vec![], b"ok").unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        assert!(response.contains("Connection: close\r\nContent-Length: 2\r\n"));
        assert!(response.ends_with("\r\n\r\nok"));
        assert!(stream.write(&vec![0; STREAM_CAP + 1]).is_err());
        stream.deadline = Instant::now();
        assert!(stream.write(b"late").is_err());
        let active = Arc::new(AtomicUsize::new(0));
        let permits: Vec<_> = (0..CONNECTION_CAP)
            .map(|_| Slot::acquire(&active).unwrap())
            .collect();
        assert!(Slot::acquire(&active).is_none());
        drop(permits);
        assert!(Slot::acquire(&active).is_some());
    }

    #[test]
    fn loopback_port_is_exclusive_until_drop_and_only_get_and_post_parse() {
        let server = Server::bind_loopback().unwrap();
        let address = server.local_addr().unwrap();
        assert_eq!(
            address.ip(),
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        );
        assert!(
            TcpListener::bind(address).is_err(),
            "a second listener took the console port"
        );
        for raw in [
            "HEAD / HTTP/1.1\r\nHost: localhost\r\n\r\n",
            "OPTIONS * HTTP/1.1\r\nHost: localhost\r\n\r\n",
            "PUT / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n",
            "DELETE / HTTP/1.1\r\nHost: localhost\r\n\r\n",
            "GET / HTTP/1.1\r\n\r\n",
            "GET / HTTP/1.0\r\nHost: localhost\r\n\r\n",
            "GET http://localhost/ HTTP/1.1\r\nHost: localhost\r\n\r\n",
        ] {
            let mut client = TcpStream::connect(address).unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            client.write_all(raw.as_bytes()).unwrap();
            let mut response = String::new();
            client.read_to_string(&mut response).unwrap();
            assert!(response.starts_with("HTTP/1.1 400 "), "{raw:?}: {response}");
        }
        drop(server);
        assert!(
            TcpListener::bind(address).is_ok(),
            "the console port stayed bound after shutdown"
        );
    }
}
