//! The connection state machine, the wrk.c port.
//!
//! One connection owns its response framer and write position and
//! drives the script callbacks through the thread engine. The
//! handlers return events the event loop interprets: interest
//! changes, delay timers, reconnects, and fatal script errors.

use std::io;
use std::sync::Arc;
use std::time::Instant;

use mio::Interest;
use wrkrs_engine::{ErrorCounts, ScriptEngine};

use crate::parser::{FrameError, Response, ResponseFramer};
use crate::stats::Histogram;

/// The socket operations the state machine needs, the seam where the
/// TLS transport lands.
pub trait Socket {
    /// Writes bytes and returns how many landed.
    fn write(&mut self, buf: &[u8]) -> io::Result<usize>;
    /// Reads bytes, zero means end of stream.
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;
}

/// The per thread counters, merged into the run totals after the
/// join.
#[derive(Debug, Default)]
pub struct Counters {
    /// Completed requests.
    pub complete: u64,
    /// Requests inside the current rate window.
    pub requests: u64,
    /// Bytes read.
    pub bytes: u64,
    /// Socket error counters.
    pub errors: ErrorCounts,
}

/// Everything the handlers need from their thread.
pub struct ThreadCtx<'a> {
    /// The shared latency histogram.
    pub latency: &'a Histogram,
    /// The thread counters.
    pub counters: &'a mut Counters,
    /// Requests per burst.
    pub pipeline: u64,
    /// Whether request() runs per burst.
    pub dynamic: bool,
    /// Whether the script supplies delays.
    pub has_delay: bool,
    /// Whether the script wants responses.
    pub wants_response: bool,
    /// The thread engine.
    pub engine: &'a mut dyn ScriptEngine,
}

/// What the event loop does next.
pub enum Event {
    /// Register this interest, readable always included.
    Interest(Interest),
    /// Close and reconnect the socket.
    Reconnect,
    /// Wait the delay in milliseconds, then go writable.
    Delay(u64),
    /// A script callback failed, the run stops.
    Fatal(String),
}

/// The request bytes of a connection.
enum Request {
    /// The shared buffer of a static script.
    Shared(Arc<Vec<u8>>),
    /// The per burst buffer of a dynamic script.
    Owned(Vec<u8>),
}

impl Request {
    fn bytes(&self) -> &[u8] {
        match self {
            Request::Shared(bytes) => bytes,
            Request::Owned(bytes) => bytes,
        }
    }
}

/// The buffer size of the C read path.
const RECVBUF: usize = 8192;

/// One benchmark connection.
pub struct Connection {
    framer: ResponseFramer,
    request: Request,
    written: usize,
    pending: u64,
    delayed: bool,
    started: Option<Instant>,
}

impl Connection {
    /// Creates a connection sharing the static request buffer.
    pub fn new(static_request: Arc<Vec<u8>>, wants_response: bool, delayed: bool) -> Connection {
        Connection {
            framer: ResponseFramer::new(wants_response),
            request: Request::Shared(static_request),
            written: 0,
            pending: 0,
            delayed,
            started: None,
        }
    }

    /// socket_connected: the socket established, the burst may start.
    pub fn connected(&mut self) -> Event {
        self.written = 0;
        Event::Interest(Interest::READABLE | Interest::WRITABLE)
    }

    /// socket_writeable: delay first, then the request burst.
    pub fn writable(&mut self, ctx: &mut ThreadCtx<'_>, socket: &mut dyn Socket) -> Event {
        if self.delayed {
            return Event::Delay(ctx.engine.delay());
        }

        if self.written == 0 {
            // A burst starts: dynamic scripts generate now, the
            // latency clock arms, the pipeline counts.
            if ctx.dynamic {
                match ctx.engine.request() {
                    Ok(bytes) => self.request = Request::Owned(bytes),
                    Err(error) => return Event::Fatal(error.raw_message().to_owned()),
                }
            }
            self.started = Some(Instant::now());
            self.pending = ctx.pipeline;
        }

        let bytes = self.request.bytes();
        match socket.write(&bytes[self.written..]) {
            Ok(written) => {
                self.written += written;
                if self.written == bytes.len() {
                    // The burst went out whole, drop writable.
                    self.written = 0;
                    Event::Interest(Interest::READABLE)
                } else {
                    Event::Interest(Interest::READABLE | Interest::WRITABLE)
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                Event::Interest(Interest::READABLE | Interest::WRITABLE)
            }
            Err(_) => {
                ctx.counters.errors.write += 1;
                Event::Reconnect
            }
        }
    }

    /// socket_readable: read into the 8 KiB buffer and drain.
    pub fn readable(&mut self, ctx: &mut ThreadCtx<'_>, socket: &mut dyn Socket) -> Event {
        let mut buffer = [0u8; RECVBUF];
        loop {
            match socket.read(&mut buffer) {
                Ok(0) => {
                    // End of stream completes an until-close message,
                    // anything else is a read error.
                    match self.framer.closed() {
                        Ok(responses) => {
                            if let Some(event) = self.responses(ctx, responses) {
                                return event;
                            }
                            ctx.counters.errors.read += 1;
                            return Event::Reconnect;
                        }
                        Err(_) => {
                            ctx.counters.errors.read += 1;
                            return Event::Reconnect;
                        }
                    }
                }
                Ok(read) => {
                    ctx.counters.bytes += read as u64;
                    match self.framer.feed(&buffer[..read]) {
                        Ok(responses) => {
                            if let Some(event) = self.responses(ctx, responses) {
                                return event;
                            }
                        }
                        Err(FrameError::BadResponse) => {
                            ctx.counters.errors.read += 1;
                            return Event::Reconnect;
                        }
                        Err(_) => {
                            ctx.counters.errors.read += 1;
                            return Event::Reconnect;
                        }
                    }
                    // The drain loop keeps reading while the buffer
                    // fills completely.
                    if read == RECVBUF {
                        continue;
                    }
                    return Event::Interest(Interest::READABLE);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    return Event::Interest(Interest::READABLE);
                }
                Err(_) => {
                    ctx.counters.errors.read += 1;
                    return Event::Reconnect;
                }
            }
        }
    }

    /// The delay timer fired, the burst may write.
    pub fn delay_fired(&mut self) -> Event {
        self.delayed = false;
        Event::Interest(Interest::READABLE | Interest::WRITABLE)
    }

    /// response_complete for every framed message.
    fn responses(&mut self, ctx: &mut ThreadCtx<'_>, responses: Vec<Response>) -> Option<Event> {
        for response in responses {
            ctx.counters.complete += 1;
            ctx.counters.requests += 1;
            if response.status > 399 {
                ctx.counters.errors.status += 1;
            }
            if ctx.wants_response
                && let Err(error) =
                    ctx.engine
                        .response(response.status, &response.headers, &response.body)
            {
                return Some(Event::Fatal(error.raw_message().to_owned()));
            }

            // The pending counter wraps like the C unsigned decrement.
            self.pending = self.pending.wrapping_sub(1);
            if self.pending == 0 {
                let latency = self
                    .started
                    .map(|started| started.elapsed().as_micros() as u64)
                    .unwrap_or_default();
                if !ctx.latency.record(latency) {
                    ctx.counters.errors.timeout += 1;
                }
                self.delayed = ctx.has_delay;
                return Some(self.rearm_or_close(&response));
            }
            if !response.keep_alive {
                return Some(Event::Reconnect);
            }
        }
        None
    }

    /// Re-arms writable after a drained pipeline or reconnects when
    /// the message closed the connection.
    fn rearm_or_close(&mut self, response: &Response) -> Event {
        if response.keep_alive {
            Event::Interest(Interest::READABLE | Interest::WRITABLE)
        } else {
            Event::Reconnect
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::{Connection, Counters, Event, Socket, ThreadCtx};
    use crate::stats::Histogram;
    use wrkrs_engine::{
        Capabilities, EngineError, ResolveApi, ScriptEngine, ScriptSpec, StatsView, Summary,
        ThreadApi, Value,
    };

    /// A minimal engine with canned answers and a response log.
    struct TestEngine {
        request: Vec<u8>,
        responses: Vec<LoggedResponse>,
    }

    /// One captured response callback.
    type LoggedResponse = (u16, Vec<(String, String)>, Vec<u8>);

    impl ScriptEngine for TestEngine {
        fn create(_spec: &ScriptSpec) -> Result<Self, EngineError>
        where
            Self: Sized,
        {
            unreachable!("tests construct the engine directly")
        }

        fn resolve(
            &mut self,
            _host: &str,
            _service: &str,
            _resolver: std::sync::Arc<dyn ResolveApi>,
        ) -> Result<Vec<std::net::SocketAddr>, EngineError> {
            Ok(Vec::new())
        }

        fn setup(&mut self, _thread: std::sync::Arc<dyn ThreadApi>) -> Result<(), EngineError> {
            Ok(())
        }

        fn init(
            &mut self,
            _thread: std::sync::Arc<dyn ThreadApi>,
            _args: &[String],
        ) -> Result<(), EngineError> {
            Ok(())
        }

        fn delay(&mut self) -> u64 {
            42
        }

        fn request(&mut self) -> Result<Vec<u8>, EngineError> {
            Ok(self.request.clone())
        }

        fn response(
            &mut self,
            status: u16,
            headers: &[(String, String)],
            body: &[u8],
        ) -> Result<(), EngineError> {
            self.responses
                .push((status, headers.to_vec(), body.to_vec()));
            Ok(())
        }

        fn done(
            &mut self,
            _summary: &Summary,
            _latency: std::sync::Arc<dyn StatsView>,
            _requests: std::sync::Arc<dyn StatsView>,
        ) -> Result<(), EngineError> {
            Ok(())
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }

        fn get_global(&self, _name: &str) -> Result<Value, EngineError> {
            Ok(Value::Null)
        }

        fn set_global(&mut self, _name: &str, _value: &Value) -> Result<(), EngineError> {
            Ok(())
        }
    }

    /// A socket with queued reads and a write log. An empty queued
    /// entry reads as end of stream, an exhausted queue blocks.
    struct FakeSocket {
        reads: Vec<Vec<u8>>,
        written: Vec<u8>,
    }

    impl FakeSocket {
        fn with(reads: &[&[u8]]) -> FakeSocket {
            FakeSocket {
                reads: reads.iter().map(|read| read.to_vec()).collect(),
                written: Vec::new(),
            }
        }
    }

    impl Socket for FakeSocket {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.written.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            match self.reads.first_mut() {
                Some(next) if !next.is_empty() => {
                    let take = next.len().min(buf.len());
                    buf[..take].copy_from_slice(&next[..take]);
                    next.drain(..take);
                    Ok(take)
                }
                Some(_) => Ok(0),
                None => Err(io::Error::new(io::ErrorKind::WouldBlock, "drained")),
            }
        }
    }

    struct Rig {
        latency: Histogram,
        counters: Counters,
        engine: TestEngine,
    }

    impl Rig {
        fn new() -> Rig {
            Rig {
                latency: Histogram::new(100_000),
                counters: Counters::default(),
                engine: TestEngine {
                    request: b"GET / HTTP/1.1\r\nHost: h\r\n\r\n".to_vec(),
                    responses: Vec::new(),
                },
            }
        }

        fn ctx(&mut self, pipeline: u64, dynamic: bool, wants_response: bool) -> ThreadCtx<'_> {
            ThreadCtx {
                latency: &self.latency,
                counters: &mut self.counters,
                pipeline,
                dynamic,
                has_delay: false,
                wants_response,
                engine: &mut self.engine,
            }
        }
    }

    fn static_request() -> std::sync::Arc<Vec<u8>> {
        std::sync::Arc::new(b"GET / HTTP/1.1\r\nHost: h\r\n\r\n".to_vec())
    }

    fn ok_response(body: &str) -> String {
        format!(
            "HTTP/1.1 200 X\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
    }

    fn writable(
        conn: &mut Connection,
        rig: &mut Rig,
        socket: &mut FakeSocket,
        pipeline: u64,
    ) -> Event {
        conn.writable(&mut rig.ctx(pipeline, false, false), socket)
    }

    fn readable(
        conn: &mut Connection,
        rig: &mut Rig,
        socket: &mut FakeSocket,
        pipeline: u64,
    ) -> Event {
        conn.readable(&mut rig.ctx(pipeline, false, false), socket)
    }

    #[test]
    fn writes_the_burst_and_waits_readable() {
        let mut conn = Connection::new(static_request(), false, false);
        let mut rig = Rig::new();
        let mut socket = FakeSocket::with(&[]);

        assert!(matches!(
            conn.connected(),
            Event::Interest(interest) if interest.is_writable()
        ));
        let event = writable(&mut conn, &mut rig, &mut socket, 1);
        assert!(matches!(event, Event::Interest(interest) if !interest.is_writable()));
        assert_eq!(socket.written, static_request().as_slice());
    }

    #[test]
    fn delays_come_before_the_burst() {
        let mut conn = Connection::new(static_request(), false, true);
        let mut rig = Rig::new();
        let mut socket = FakeSocket::with(&[]);
        let event = conn.writable(&mut rig.ctx(1, false, false), &mut socket);
        assert!(matches!(event, Event::Delay(42)));
        assert!(socket.written.is_empty());

        let event = conn.delay_fired();
        assert!(matches!(event, Event::Interest(interest) if interest.is_writable()));
    }

    #[test]
    fn responses_count_and_record_latency() {
        let mut conn = Connection::new(static_request(), false, false);
        let mut rig = Rig::new();
        let mut socket = FakeSocket::with(&[ok_response("hi").as_bytes()]);

        writable(&mut conn, &mut rig, &mut socket, 1);
        let event = readable(&mut conn, &mut rig, &mut socket, 1);
        assert!(matches!(event, Event::Interest(interest) if interest.is_writable()));
        assert_eq!(rig.counters.complete, 1);
        assert_eq!(rig.counters.requests, 1);
        assert_eq!(rig.latency.count(), 1);
    }

    #[test]
    fn pipelines_record_once_per_drain() {
        let mut conn = Connection::new(static_request(), false, false);
        let mut rig = Rig::new();
        let wire = (ok_response("a") + &ok_response("b")).into_bytes();
        let mut socket = FakeSocket::with(&[wire.as_slice()]);

        writable(&mut conn, &mut rig, &mut socket, 2);
        let event = readable(&mut conn, &mut rig, &mut socket, 2);
        assert!(matches!(event, Event::Interest(interest) if interest.is_writable()));
        assert_eq!(rig.counters.complete, 2);
        assert_eq!(rig.latency.count(), 1);
    }

    #[test]
    fn error_statuses_count() {
        let mut conn = Connection::new(static_request(), false, false);
        let mut rig = Rig::new();
        let wire = "HTTP/1.1 404 X\r\nContent-Length: 0\r\n\r\n"
            .as_bytes()
            .to_vec();
        let mut socket = FakeSocket::with(&[wire.as_slice()]);

        writable(&mut conn, &mut rig, &mut socket, 1);
        readable(&mut conn, &mut rig, &mut socket, 1);
        assert_eq!(rig.counters.errors.status, 1);
        assert_eq!(rig.counters.complete, 1);
    }

    #[test]
    fn wanted_responses_reach_the_engine() {
        let mut conn = Connection::new(static_request(), true, false);
        let mut rig = Rig::new();
        let mut socket = FakeSocket::with(&[ok_response("hi").as_bytes()]);

        conn.writable(&mut rig.ctx(1, false, true), &mut socket);
        conn.readable(&mut rig.ctx(1, false, true), &mut socket);
        assert_eq!(rig.engine.responses.len(), 1);
        assert_eq!(rig.engine.responses[0].0, 200);
        assert_eq!(rig.engine.responses[0].2, b"hi".to_vec());
    }

    #[test]
    fn close_headers_reconnect_after_the_drain() {
        let mut conn = Connection::new(static_request(), false, false);
        let mut rig = Rig::new();
        let wire = b"HTTP/1.1 200 X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let mut socket = FakeSocket::with(&[wire]);

        writable(&mut conn, &mut rig, &mut socket, 1);
        let event = readable(&mut conn, &mut rig, &mut socket, 1);
        assert!(matches!(event, Event::Reconnect));
        assert_eq!(rig.counters.complete, 1);
    }

    #[test]
    fn eof_mid_message_reconnects_with_a_read_error() {
        let mut conn = Connection::new(static_request(), false, false);
        let mut rig = Rig::new();
        let wire = b"HTTP/1.1 200 X\r\nContent-Length: 9\r\n\r\nshort";
        let end = b"";
        let mut socket = FakeSocket::with(&[wire, end]);

        writable(&mut conn, &mut rig, &mut socket, 1);
        // The short read returns without touching the end of stream,
        // the next readable event sees it.
        let _ = readable(&mut conn, &mut rig, &mut socket, 1);
        let event = readable(&mut conn, &mut rig, &mut socket, 1);
        assert!(matches!(event, Event::Reconnect));
        assert_eq!(rig.counters.errors.read, 1);
        assert_eq!(rig.counters.complete, 0);
    }

    #[test]
    fn write_errors_reconnect_with_a_write_error() {
        struct BrokenSocket;

        impl Socket for BrokenSocket {
            fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("gone"))
            }

            fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::new(io::ErrorKind::WouldBlock, "drained"))
            }
        }

        let mut conn = Connection::new(static_request(), false, false);
        let mut rig = Rig::new();
        let mut socket = BrokenSocket;
        let event = conn.writable(&mut rig.ctx(1, false, false), &mut socket);
        assert!(matches!(event, Event::Reconnect));
        assert_eq!(rig.counters.errors.write, 1);
    }

    #[test]
    fn dynamic_scripts_generate_per_burst() {
        let mut conn = Connection::new(static_request(), false, false);
        let mut rig = Rig::new();
        let mut socket = FakeSocket::with(&[ok_response("").as_bytes()]);

        conn.writable(&mut rig.ctx(1, true, false), &mut socket);
        conn.readable(&mut rig.ctx(1, true, false), &mut socket);
        assert_eq!(socket.written, rig.engine.request);
    }
}
