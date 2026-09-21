//! The per thread event loop: readiness events, deadline timers, and
//! the rate tick.
//!
//! One loop owns every connection of its thread, mirroring the ae
//! loop of wrk.c: readable fires before writable, the 100 ms tick
//! samples the rate window and checks the stop flags, delay timers
//! re-arm writable, and connect failures reconnect immediately, the
//! original storm behavior.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mio::net::TcpStream;
use mio::{Events, Interest, Poll, Token};

use crate::connection::{Connection, Counters, Event, Socket, ThreadCtx};
use crate::runner::HostThread;
use crate::stats::Histogram;
use crate::tls::{TlsSetup, TlsStream};
use wrkrs_engine::ScriptEngine;

/// The sample window of the rate histogram.
const RECORD_INTERVAL_MS: u64 = 100;

/// The global stop flag, set by the timer thread and SIGINT.
#[derive(Default)]
pub struct StopFlag(AtomicBool);

impl StopFlag {
    /// Creates a cleared flag.
    pub fn new() -> StopFlag {
        StopFlag(AtomicBool::new(false))
    }

    /// Raises the flag.
    pub fn stop(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Whether the flag is up.
    pub fn stopped(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// What the loop does with one connection.
enum Reaction {
    Interest(Interest),
    Reconnect,
    Delay(u64),
    Fatal(String),
    Nothing,
}

/// One slot of the connection table.
struct Slot {
    stream: Transport,
    connection: Connection,
    connecting: bool,
}

/// The stream of a slot: plain TCP or TLS over it.
enum Transport {
    Plain(TcpStream),
    Tls(Box<TlsStream>),
}

impl Transport {
    /// The underlying stream, for event registration and socket
    /// options.
    fn raw(&mut self) -> &mut TcpStream {
        match self {
            Transport::Plain(stream) => stream,
            Transport::Tls(tls) => tls.raw(),
        }
    }

    /// The best effort close, the ssl_close shutdown.
    fn shutdown(&mut self) {
        if let Transport::Tls(tls) = self {
            tls.shutdown();
        }
    }
}

impl Socket for Transport {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Transport::Plain(stream) => stream.write(buf),
            Transport::Tls(tls) => tls.write(buf),
        }
    }

    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Transport::Plain(stream) => stream.read(buf),
            Transport::Tls(tls) => tls.read(buf),
        }
    }
}

/// A pending deadline.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Deadline {
    at: Instant,
    /// Which connection wakes, None is unused.
    token: usize,
}

/// The thread level run configuration.
struct LoopConfig {
    address: SocketAddr,
    static_request: Option<Arc<Vec<u8>>>,
    pipeline: u64,
    dynamic: bool,
    has_delay: bool,
    wants_response: bool,
    /// The TLS setup when the target is https.
    tls: Option<Arc<TlsSetup>>,
}

impl LoopConfig {
    /// Builds a connection for a fresh socket. Reconnects start
    /// undelayed, matching the C flag handling.
    fn connection(&self, initial: bool) -> Connection {
        let request = self
            .static_request
            .clone()
            .unwrap_or_else(|| Arc::new(Vec::new()));
        Connection::new(request, self.wants_response, initial && self.has_delay)
    }
}

/// Runs the benchmark loop for one thread.
///
/// The engine moves in with the thread and parks back into its handle
/// when the loop ends. Returns the thread counters.
#[allow(clippy::too_many_arguments)]
pub fn run(
    address: SocketAddr,
    connections: usize,
    engine: Box<dyn ScriptEngine>,
    handle: Arc<HostThread>,
    latency: Arc<Histogram>,
    rate: Arc<Histogram>,
    stop: Arc<StopFlag>,
    pipeline: u64,
    dynamic: bool,
    has_delay: bool,
    wants_response: bool,
    tls: Option<Arc<TlsSetup>>,
) -> Counters {
    let mut counters = Counters::default();
    let outcome = drive(
        address,
        connections,
        engine,
        handle,
        latency,
        rate,
        stop,
        pipeline,
        dynamic,
        has_delay,
        wants_response,
        tls,
        &mut counters,
    );
    if let Err(message) = outcome {
        // The unprotected C calls abort the process here.
        eprintln!("{message}");
        std::process::exit(1);
    }
    counters
}

/// Drives the loop, parking the engine back on every exit.
#[allow(clippy::too_many_arguments)]
fn drive(
    address: SocketAddr,
    connections: usize,
    mut engine: Box<dyn ScriptEngine>,
    handle: Arc<HostThread>,
    latency: Arc<Histogram>,
    rate: Arc<Histogram>,
    stop: Arc<StopFlag>,
    pipeline: u64,
    dynamic: bool,
    has_delay: bool,
    wants_response: bool,
    tls: Option<Arc<TlsSetup>>,
    counters: &mut Counters,
) -> Result<(), String> {
    let result = loop_once(
        address,
        connections,
        &mut engine,
        &handle,
        &latency,
        &rate,
        &stop,
        pipeline,
        dynamic,
        has_delay,
        wants_response,
        tls,
        counters,
    );
    handle.park(engine);
    result
}

/// The loop body, broken out so the engine parks on every exit.
#[allow(clippy::too_many_arguments)]
fn loop_once(
    address: SocketAddr,
    connections: usize,
    engine: &mut Box<dyn ScriptEngine>,
    handle: &Arc<HostThread>,
    latency: &Arc<Histogram>,
    rate: &Arc<Histogram>,
    stop: &Arc<StopFlag>,
    pipeline: u64,
    dynamic: bool,
    has_delay: bool,
    wants_response: bool,
    tls: Option<Arc<TlsSetup>>,
    counters: &mut Counters,
) -> Result<(), String> {
    let mut poll = Poll::new().map_err(|error| error.to_string())?;
    let registry = poll.registry().try_clone().map_err(|e| e.to_string())?;
    let mut events = Events::with_capacity(1024);
    let config = LoopConfig {
        address,
        static_request: if dynamic {
            None
        } else {
            let bytes = engine
                .request()
                .map_err(|error| error.raw_message().to_owned())?;
            Some(Arc::new(bytes))
        },
        pipeline,
        dynamic,
        has_delay,
        wants_response,
        tls,
    };

    let mut slots: Vec<Option<Slot>> = Vec::with_capacity(connections);
    let mut deadlines = BinaryHeap::new();
    let mut rate_deadline = Instant::now() + Duration::from_millis(RECORD_INTERVAL_MS);
    let mut window_start = Instant::now();

    for index in 0..connections {
        slots.push(None);
        connect_slot(&registry, &mut slots, index, &config, counters);
    }

    loop {
        let timeout = next_timeout(&deadlines, rate_deadline);
        poll.poll(&mut events, timeout)
            .map_err(|error| error.to_string())?;

        for event in &events {
            let index = event.token().0;
            if index >= slots.len() {
                continue;
            }

            if slots[index].as_ref().is_some_and(|slot| slot.connecting) {
                // The connect attempt finished, a pending error
                // refused it and the storm reconnects at once.
                let refused = slots[index].as_mut().is_some_and(|slot| {
                    matches!(slot.stream.raw().take_error(), Ok(Some(_)) | Err(_))
                });
                if refused {
                    counters.errors.connect += 1;
                    close_slot(&registry, &mut slots, index);
                    connect_slot(&registry, &mut slots, index, &config, counters);
                    continue;
                }
                // The TCP connection is up, a TLS session keeps
                // handshaking one step per readiness event.
                let mut failed = false;
                let mut handshaking = false;
                if let Some(slot) = slots[index].as_mut()
                    && let Transport::Tls(tls) = &mut slot.stream
                {
                    match tls.handshake() {
                        Ok(true) => {}
                        Ok(false) => handshaking = true,
                        Err(_) => failed = true,
                    }
                }
                if failed {
                    // A fatal handshake error counts like a refused
                    // connect and storms, the ssl_connect ERROR path.
                    counters.errors.connect += 1;
                    close_slot(&registry, &mut slots, index);
                    connect_slot(&registry, &mut slots, index, &config, counters);
                    continue;
                }
                if handshaking {
                    continue;
                }
                let reaction = slots[index]
                    .as_mut()
                    .map(|slot| match slot.connection.connected() {
                        Event::Interest(interest) => Reaction::Interest(interest),
                        _ => Reaction::Nothing,
                    })
                    .unwrap_or(Reaction::Nothing);
                if let Some(slot) = slots[index].as_mut() {
                    slot.connecting = false;
                }
                apply(&registry, &mut slots, index, &config, reaction, counters);
                continue;
            }

            // Readable fires before writable, the ae order.
            if event.is_readable() {
                let reaction = fire(&mut slots, index, engine, latency, counters, &config, false);
                apply(&registry, &mut slots, index, &config, reaction, counters);
            }

            if event.is_writable() && slots[index].as_ref().is_some_and(|slot| !slot.connecting) {
                let reaction = fire(&mut slots, index, engine, latency, counters, &config, true);
                match reaction {
                    Reaction::Delay(millis) => {
                        deadlines.push(Reverse(Deadline {
                            at: Instant::now() + Duration::from_millis(millis),
                            token: index,
                        }));
                        if let Some(slot) = slots[index].as_mut() {
                            let _ = registry.reregister(
                                slot.stream.raw(),
                                Token(index),
                                Interest::READABLE,
                            );
                        }
                    }
                    other => {
                        apply(&registry, &mut slots, index, &config, other, counters);
                    }
                }
            }
        }

        // Fire the due delays, re-arming writable.
        while let Some(Reverse(deadline)) = deadlines.peek() {
            if deadline.at > Instant::now() {
                break;
            }
            let Reverse(deadline) = deadlines.pop().expect("peeked above");
            let index = deadline.token;
            if let Some(slot) = slots[index].as_mut()
                && !slot.connecting
            {
                match slot.connection.delay_fired() {
                    Event::Interest(interest) => {
                        let _ = registry.reregister(slot.stream.raw(), Token(index), interest);
                    }
                    _ => unreachable!("delay_fired only changes interest"),
                }
            }
        }

        // The rate tick samples the window and checks the flags.
        if Instant::now() >= rate_deadline {
            record_rate(counters, rate, &mut window_start);
            rate_deadline += Duration::from_millis(RECORD_INTERVAL_MS);
            if stop.stopped() || handle.stopped() {
                return Ok(());
            }
        }

        // A thread stop exits at the end of the iteration it fired
        // in, the aeStop timing: the batch and any due timers finish,
        // the loop ends without another poll.
        if handle.stopped() {
            return Ok(());
        }
    }
}

/// Runs one readable or writable handler for a connection.
#[allow(clippy::too_many_arguments)]
fn fire(
    slots: &mut [Option<Slot>],
    index: usize,
    engine: &mut Box<dyn ScriptEngine>,
    latency: &Arc<Histogram>,
    counters: &mut Counters,
    config: &LoopConfig,
    writable: bool,
) -> Reaction {
    let Some(slot) = slots[index].as_mut() else {
        return Reaction::Nothing;
    };
    let mut ctx = ThreadCtx {
        latency,
        counters,
        pipeline: config.pipeline,
        dynamic: config.dynamic,
        has_delay: config.has_delay,
        wants_response: config.wants_response,
        engine: engine.as_mut(),
    };
    let event = if writable {
        slot.connection.writable(&mut ctx, &mut slot.stream)
    } else {
        slot.connection.readable(&mut ctx, &mut slot.stream)
    };
    match event {
        Event::Interest(interest) => Reaction::Interest(interest),
        Event::Reconnect => Reaction::Reconnect,
        Event::Delay(millis) => Reaction::Delay(millis),
        Event::Fatal(message) => Reaction::Fatal(message),
    }
}

/// Applies a reaction to a connection slot.
fn apply(
    registry: &mio::Registry,
    slots: &mut [Option<Slot>],
    index: usize,
    config: &LoopConfig,
    reaction: Reaction,
    counters: &mut Counters,
) {
    match reaction {
        Reaction::Interest(interest) => {
            if let Some(slot) = slots[index].as_mut() {
                let _ = registry.reregister(slot.stream.raw(), Token(index), interest);
            }
        }
        Reaction::Reconnect => {
            close_slot(registry, slots, index);
            connect_slot(registry, slots, index, config, counters);
        }
        Reaction::Delay(_) => {
            // Delays apply in the writable arm which owns the heap.
        }
        Reaction::Fatal(message) => {
            // The unprotected C calls abort the process.
            eprintln!("{message}");
            std::process::exit(1);
        }
        Reaction::Nothing => {}
    }
}

/// Opens a non-blocking connection and registers it. A local failure
/// counts a connect error and leaves the slot empty, the C loop drops
/// the connection the same way.
fn connect_slot(
    registry: &mio::Registry,
    slots: &mut [Option<Slot>],
    index: usize,
    config: &LoopConfig,
    counters: &mut Counters,
) {
    let stream = match TcpStream::connect(config.address) {
        Ok(stream) => stream,
        Err(_) => {
            counters.errors.connect += 1;
            slots[index] = None;
            return;
        }
    };
    let _ = stream.set_nodelay(true);
    let connection = config.connection(true);
    let mut transport = match &config.tls {
        Some(setup) => match setup.stream(stream) {
            Ok(tls) => Transport::Tls(Box::new(tls)),
            Err(_) => {
                counters.errors.connect += 1;
                slots[index] = None;
                return;
            }
        },
        None => Transport::Plain(stream),
    };
    if registry
        .register(
            transport.raw(),
            Token(index),
            Interest::READABLE | Interest::WRITABLE,
        )
        .is_err()
    {
        counters.errors.connect += 1;
        slots[index] = None;
        return;
    }
    slots[index] = Some(Slot {
        stream: transport,
        connection,
        connecting: true,
    });
}

/// Drops a slot and its registration, sending the TLS close first.
fn close_slot(registry: &mio::Registry, slots: &mut [Option<Slot>], index: usize) {
    if let Some(mut slot) = slots[index].take() {
        slot.stream.shutdown();
        let _ = registry.deregister(slot.stream.raw());
    }
}

/// The rate window sample of the C tick.
fn record_rate(counters: &mut Counters, rate: &Histogram, window_start: &mut Instant) {
    if counters.requests > 0 {
        let elapsed_ms = window_start.elapsed().as_micros() as f64 / 1000.0;
        let sampled = (counters.requests as f64 / elapsed_ms * 1000.0) as u64;
        rate.record(sampled);
        counters.requests = 0;
        *window_start = Instant::now();
    }
}

/// The poll timeout until the next deadline.
fn next_timeout(
    deadlines: &BinaryHeap<Reverse<Deadline>>,
    rate_deadline: Instant,
) -> Option<Duration> {
    let next = deadlines
        .peek()
        .map(|Reverse(deadline)| deadline.at)
        .unwrap_or(rate_deadline)
        .min(rate_deadline);
    Some(next.saturating_duration_since(Instant::now()))
}

/// The socket adapter over the mio stream.
impl Socket for TcpStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        std::io::Write::write(self, buf)
    }

    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        std::io::Read::read(self, buf)
    }
}

#[cfg(test)]
mod tests {
    use std::net::{TcpListener, TcpStream as StdStream};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    use super::{StopFlag, run};
    use crate::runner::{HostThread, build_spec};
    use crate::stats::Histogram;

    /// Serves keep-alive responses on one connection.
    fn serve(mut stream: StdStream) {
        use std::io::{Read, Write};
        let mut buffer = [0u8; 4096];
        let mut pending = Vec::new();
        loop {
            let read = stream.read(&mut buffer).unwrap_or(0);
            if read == 0 {
                return;
            }
            pending.extend_from_slice(&buffer[..read]);
            while pending.windows(4).any(|window| window == b"\r\n\r\n") {
                let end = pending
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .expect("checked above")
                    + 4;
                pending.drain(..end);
                stream
                    .write_all(b"HTTP/1.1 200 X\r\nContent-Length: 2\r\n\r\nok")
                    .expect("write response");
            }
        }
    }

    #[test]
    fn a_dead_target_storms_and_counts_connect_errors() {
        let factory = crate::engines::default_engine().expect("an engine").factory;
        let spec = build_spec(&test_config("http://127.0.0.1:1/"));
        let mut engine = factory(&spec).expect("engine builds");
        engine.init(Arc::new(HostThread::new()), &[]).expect("init");

        let stop = Arc::new(StopFlag::new());
        let early = Arc::clone(&stop);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(400));
            early.stop();
        });

        let counters = run(
            "127.0.0.1:1".parse().unwrap(),
            1,
            engine,
            Arc::new(HostThread::new()),
            Arc::new(Histogram::new(2_000_000)),
            Arc::new(Histogram::new(10_000_000)),
            stop,
            1,
            false,
            false,
            false,
            None,
        );
        assert!(counters.errors.connect > 0, "the storm counts");
        assert_eq!(counters.complete, 0);
    }

    fn test_config(url: &str) -> crate::cli::Config {
        crate::cli::Config {
            threads: 1,
            connections: 1,
            duration_s: 1,
            timeout_ms: 2000,
            latency: false,
            script: None,
            headers: vec![],
            engine: None,
            url: url.to_owned(),
            parts: crate::parser::parse_url(url).expect("valid url"),
            init_args: vec![url.to_owned()],
        }
    }

    #[cfg(any(feature = "engine-luajit", feature = "engine-lua54"))]
    #[test]
    fn runs_a_live_loop_against_a_local_server() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                thread::spawn(move || serve(stream));
            }
        });

        let url = format!("http://127.0.0.1:{}/", address.port());
        let factory = crate::engines::default_engine().expect("an engine").factory;
        let spec = build_spec(&test_config(&url));
        let mut engine = factory(&spec).expect("engine builds");
        engine
            .init(Arc::new(HostThread::new()), &[url])
            .expect("init");

        let stop = Arc::new(StopFlag::new());
        let early = Arc::clone(&stop);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(500));
            early.stop();
        });

        let counters = run(
            address,
            1,
            engine,
            Arc::new(HostThread::new()),
            Arc::new(Histogram::new(2_000_000)),
            Arc::new(Histogram::new(10_000_000)),
            stop,
            1,
            false,
            false,
            false,
            None,
        );
        assert!(counters.complete > 0, "completed requests: {:?}", counters);
        assert!(counters.bytes > 0);
        assert_eq!(counters.errors.read, 0);
        assert_eq!(counters.errors.write, 0);
        assert_eq!(counters.errors.status, 0);
    }

    /// Serves keep-alive responses over TLS on one connection.
    fn serve_tls(mut stream: StdStream) {
        use std::io::{Read, Write};
        let mut conn =
            rustls::ServerConnection::new(crate::tls::test_server_config()).expect("server");
        while conn.is_handshaking() {
            conn.complete_io(&mut stream).expect("server handshake");
        }
        let mut buf = [0u8; 4096];
        let mut pending = Vec::new();
        loop {
            match conn.reader().read(&mut buf) {
                Ok(0) => return,
                Ok(read) => pending.extend_from_slice(&buf[..read]),
                Err(ref error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => return,
            }
            while pending.windows(4).any(|window| window == b"\r\n\r\n") {
                let end = pending
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .expect("checked above")
                    + 4;
                pending.drain(..end);
                conn.writer()
                    .write_all(b"HTTP/1.1 200 X\r\nContent-Length: 2\r\n\r\nok")
                    .expect("write response");
                conn.write_tls(&mut stream).expect("send");
            }
            conn.complete_io(&mut stream).expect("io");
        }
    }

    #[test]
    fn runs_a_live_loop_against_a_tls_server() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                thread::spawn(move || serve_tls(stream));
            }
        });

        let url = format!("https://127.0.0.1:{}/", address.port());
        let factory = crate::engines::default_engine().expect("an engine").factory;
        let spec = build_spec(&test_config(&url));
        let mut engine = factory(&spec).expect("engine builds");
        engine
            .init(Arc::new(HostThread::new()), &[url])
            .expect("init");

        let stop = Arc::new(StopFlag::new());
        let early = Arc::clone(&stop);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(500));
            early.stop();
        });

        let counters = run(
            address,
            1,
            engine,
            Arc::new(HostThread::new()),
            Arc::new(Histogram::new(2_000_000)),
            Arc::new(Histogram::new(10_000_000)),
            stop,
            1,
            false,
            false,
            false,
            Some(Arc::new(crate::tls::TlsSetup::new("127.0.0.1"))),
        );
        assert!(counters.complete > 0, "completed requests: {counters:?}");
        assert!(counters.bytes > 0);
        assert_eq!(counters.errors.read, 0);
        assert_eq!(counters.errors.write, 0);
        assert_eq!(counters.errors.status, 0);
    }
}
