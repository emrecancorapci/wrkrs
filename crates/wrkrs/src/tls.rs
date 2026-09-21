//! The TLS transport over rustls, the ssl.c port.
//!
//! Certificate verification is disabled and the SNI carries the URL
//! host, the ssl_init and SSL_set_tlsext_host_name behavior. The
//! handshake is driven one step per readiness event, the RETRY shape
//! of ssl_connect, and the reads and writes follow the SSL_read and
//! SSL_write retry semantics.

use std::io::{self, ErrorKind, Read, Write};
use std::sync::Arc;

use mio::net::TcpStream;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::ring;
use rustls::pki_types::{ServerName, UnixTime};
use rustls::{
    ClientConfig, ClientConnection, DigitallySignedStruct, Error as TlsError, SignatureScheme,
};

use crate::connection::Socket;

/// The verifier that accepts every certificate, SSL_VERIFY_NONE with
/// a verify depth of zero.
#[derive(Debug)]
struct NoVerify;

impl ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// The client side setup shared by every connection of a run.
pub struct TlsSetup {
    config: Arc<ClientConfig>,
    name: ServerName<'static>,
}

impl TlsSetup {
    /// Builds the setup for one host: no verification, SNI from the
    /// host. An IP host becomes an address name and no SNI is sent.
    pub fn new(host: &str) -> TlsSetup {
        let config = ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerify))
            .with_no_client_auth();
        let name = ServerName::try_from(host.to_owned())
            .unwrap_or_else(|_| ServerName::try_from("localhost".to_owned()).expect("valid"));
        TlsSetup {
            config: Arc::new(config),
            name,
        }
    }

    /// Wraps a connected stream with a fresh session.
    pub fn stream(&self, sock: TcpStream) -> io::Result<TlsStream> {
        let conn = ClientConnection::new(self.config.clone(), self.name.clone())
            .map_err(|error| io::Error::other(error.to_string()))?;
        Ok(TlsStream {
            conn,
            sock,
            sending: 0,
        })
    }
}

/// One TLS session over the evented stream.
pub struct TlsStream {
    conn: ClientConnection,
    sock: TcpStream,
    /// Plaintext bytes queued whose records are not on the wire yet.
    sending: usize,
}

/// A self signed server config for the tests, any valid certificate
/// serves because the client verifies nothing.
#[cfg(test)]
pub fn test_server_config() -> std::sync::Arc<rustls::ServerConfig> {
    let key = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).expect("cert");
    let private = rustls::pki_types::PrivateKeyDer::Pkcs8(
        rustls::pki_types::PrivatePkcs8KeyDer::from(key.signing_key.serialize_der()),
    );
    std::sync::Arc::new(
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![key.cert.der().clone()], private)
            .expect("valid cert"),
    )
}

impl TlsStream {
    /// Drives the handshake one step, ssl_connect. True once the
    /// session is established. A WouldBlock is a retry step, the
    /// WANT_READ and WANT_WRITE of the C flow, anything else is a
    /// fatal error.
    pub fn handshake(&mut self) -> io::Result<bool> {
        if self.conn.is_handshaking() {
            let retry = match self.handshake_step() {
                Ok(()) => false,
                Err(error) if error.kind() == ErrorKind::WouldBlock => true,
                Err(error) => return Err(error),
            };
            if retry {
                return Ok(false);
            }
        }
        Ok(!self.conn.is_handshaking())
    }

    /// One exchange of pending handshake bytes.
    fn handshake_step(&mut self) -> io::Result<()> {
        if self.conn.wants_write() {
            write_tls(&mut self.conn, &mut self.sock)?;
        }
        if self.conn.wants_read() {
            self.conn.read_tls(&mut self.sock)?;
            self.conn
                .process_new_packets()
                .map_err(|error| io::Error::other(error.to_string()))?;
        }
        Ok(())
    }

    /// Flushes pending TLS records to the socket.
    fn flush_tls(&mut self) -> io::Result<()> {
        while self.conn.wants_write() {
            write_tls(&mut self.conn, &mut self.sock)?;
        }
        Ok(())
    }

    /// Sends the close notify, ssl_close, best effort.
    pub fn shutdown(&mut self) {
        if !self.conn.is_handshaking() {
            self.conn.send_close_notify();
            let _ = self.flush_tls();
        }
    }

    /// The underlying stream, for event registration.
    pub fn raw(&mut self) -> &mut TcpStream {
        &mut self.sock
    }
}

/// One write_tls step, a clean zero read stops the loop.
fn write_tls(conn: &mut ClientConnection, sock: &mut TcpStream) -> io::Result<()> {
    match conn.write_tls(sock) {
        Ok(0) => Err(io::Error::new(ErrorKind::WriteZero, "tls write stalled")),
        Ok(_) => Ok(()),
        Err(error) => Err(error),
    }
}

impl Socket for TlsStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // Finish the previous chunk before queueing new plaintext,
        // the retry after a WouldBlock reports its completion so the
        // caller advances before sending more.
        self.flush_tls()?;
        if self.sending > 0 {
            let done = self.sending;
            self.sending = 0;
            return Ok(done);
        }
        let queued = self.conn.writer().write(buf)?;
        self.sending = queued;
        match self.flush_tls() {
            Ok(()) => {
                let done = self.sending;
                self.sending = 0;
                Ok(done)
            }
            Err(error) => Err(error),
        }
    }

    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // Server traffic can require a reply record before more
        // plaintext arrives.
        self.flush_tls()?;
        match self.conn.reader().read(buf) {
            Ok(read) => Ok(read),
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                self.conn.read_tls(&mut self.sock)?;
                self.conn
                    .process_new_packets()
                    .map_err(|error| io::Error::other(error.to_string()))?;
                self.conn.reader().read(buf)
            }
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    use rustls::ServerConnection;

    use super::{Socket, TlsSetup, TlsStream};

    /// A self signed server config, any valid certificate serves
    /// because the client verifies nothing.
    fn server_config() -> Arc<rustls::ServerConfig> {
        super::test_server_config()
    }

    /// Drives a server session over a blocking stream until the
    /// handshake completes, then serves one request and response.
    fn serve_one(stream: std::net::TcpStream, seen: &mut Vec<String>, name: &mut Vec<String>) {
        let mut conn = ServerConnection::new(server_config()).expect("server");
        let mut stream = stream;
        while conn.is_handshaking() {
            conn.complete_io(&mut stream).expect("server handshake");
        }
        if let Some(server_name) = conn.server_name() {
            name.push(server_name.to_owned());
        }
        let mut buf = [0u8; 4096];
        let mut text = String::new();
        loop {
            conn.complete_io(&mut stream).expect("io");
            match conn.reader().read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    text.push_str(&String::from_utf8_lossy(&buf[..read]));
                    if text.contains("\r\n\r\n") {
                        break;
                    }
                }
            }
        }
        seen.push(text);
        conn.writer()
            .write_all(b"HTTP/1.1 200 X\r\nContent-Length: 2\r\n\r\nok")
            .expect("write");
        conn.write_tls(&mut stream).expect("send");
        conn.send_close_notify();
        let _ = conn.write_tls(&mut stream);
        let _ = stream.shutdown(std::net::Shutdown::Both);
    }

    /// Drives the client session one event step at a time, retrying
    /// on WouldBlock the way the event loop keeps firing.
    fn drive<F: FnMut() -> bool>(mut step: F) {
        while !step() {
            thread::sleep(Duration::from_micros(200));
        }
    }

    /// Handshake steps until done, WouldBlock means wait for the next
    /// readiness event, anything else is fatal.
    fn finish_handshake(client: &mut TlsStream) {
        drive(|| match client.handshake() {
            Ok(done) => done,
            Err(ref error) if error.kind() == std::io::ErrorKind::WouldBlock => false,
            Err(error) => panic!("handshake failed: {error}"),
        });
    }

    fn tls_stream(host: &str, port: u16) -> TlsStream {
        let std_stream = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
        std_stream.set_nonblocking(true).expect("nonblocking");
        let setup = TlsSetup::new(host);
        setup
            .stream(mio::net::TcpStream::from_std(std_stream))
            .expect("session")
    }

    #[test]
    fn handshakes_and_roundtrips_plaintext() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("address").port();
        let mut seen = Vec::new();
        let mut names = Vec::new();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            serve_one(stream, &mut seen, &mut names);
            (seen, names)
        });

        let mut client = tls_stream("localhost", port);
        finish_handshake(&mut client);
        // Write until the whole request is reported as sent.
        let request = b"GET / HTTP/1.1\r\nHost: h\r\n\r\n";
        loop {
            match client.write(request) {
                Ok(0) => unreachable!("writer reports whole chunks"),
                Ok(_) => break,
                Err(ref error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_micros(200));
                }
                Err(error) => panic!("write failed: {error}"),
            }
        }

        let mut received = Vec::new();
        loop {
            let mut buf = [0u8; 512];
            match client.read(&mut buf) {
                Ok(0) => break,
                Ok(read) => received.extend_from_slice(&buf[..read]),
                Err(ref error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_micros(200));
                }
                Err(error) => panic!("read failed: {error}"),
            }
            if received.ends_with(b"ok") {
                break;
            }
        }
        assert!(received.starts_with(b"HTTP/1.1 200"));
        assert!(received.ends_with(b"ok"));

        client.shutdown();
        let (seen, names) = server.join().expect("server");
        assert_eq!(seen.len(), 1);
        assert!(seen[0].starts_with("GET / HTTP/1.1"));
        // The SNI carried the host.
        assert_eq!(names, vec!["localhost".to_owned()]);
    }

    #[test]
    fn ip_hosts_send_no_sni() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("address").port();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            let mut seen = Vec::new();
            let mut names = Vec::new();
            serve_one(stream, &mut seen, &mut names);
            names
        });

        let mut client = tls_stream("127.0.0.1", port);
        finish_handshake(&mut client);
        let _ = client.write(b"GET / HTTP/1.1\r\nHost: h\r\n\r\n");
        client.shutdown();
        let names = server.join().expect("server");
        assert!(names.is_empty(), "no SNI for an IP host: {names:?}");
    }
}
