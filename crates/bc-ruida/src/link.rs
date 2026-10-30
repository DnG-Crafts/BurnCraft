//! UDP transport to a Ruida controller over Ethernet.
//!
//! Commands go to the controller's UDP port 50200; it replies to port 40200.
//! Every packet must be acknowledged (ACK) before the next one is sent; a NAK
//! means the checksum failed and the packet is resent.

use crate::codec::{decode35, udp_packet, Swizzle, ACK, ENQ, ERR, NAK};
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

pub const CONTROLLER_PORT: u16 = 50200;
pub const REPLY_PORT: u16 = 40200;

/// Payload budget per packet. The protocol allows more, but many firmwares cut near 1000 bytes.
pub const MAX_PAYLOAD: usize = 1000;

#[derive(Debug)]
pub enum LinkError {
    Io(io::Error),
    /// No reply within the timeout.
    Timeout,
    /// The controller rejected the packet repeatedly or reported an error.
    Rejected(&'static str),
    /// A reply that could not be parsed.
    BadReply(Vec<u8>),
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkError::Io(e) if e.kind() == io::ErrorKind::AddrInUse => {
                write!(f, "UDP port {REPLY_PORT} is in use; close other laser software (e.g. LightBurn, RDWorks) and try again")
            }
            LinkError::Io(e) => write!(f, "network error: {e}"),
            LinkError::Timeout => write!(f, "the controller did not reply; check the IP address and cable"),
            LinkError::Rejected(why) => write!(f, "the controller rejected the data: {why}"),
            LinkError::BadReply(b) => write!(f, "unexpected reply from controller: {b:02X?}"),
        }
    }
}

impl std::error::Error for LinkError {}

impl From<io::Error> for LinkError {
    fn from(e: io::Error) -> Self {
        match e.kind() {
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => LinkError::Timeout,
            _ => LinkError::Io(e),
        }
    }
}

pub type Result<T> = std::result::Result<T, LinkError>;

/// A way of talking to the controller: Ethernet (UDP) or USB (serial).
pub trait Transport: Send {
    /// Sends one packet; returns any data reply (for parameter reads).
    fn send_packet(&self, payload: &[u8]) -> Result<Option<Vec<u8>>>;
    /// Sends a stream of commands, reporting (bytes sent, total).
    fn send_stream(&self, commands: &[Vec<u8>], progress: &mut dyn FnMut(usize, usize)) -> Result<()>;
    /// Reads a controller parameter (memory address) as a signed 35-bit value.
    fn read_param(&self, address: [u8; 2]) -> Result<i64> {
        let reply = self.send_packet(&[0xDA, 0x00, address[0], address[1]])?.ok_or(LinkError::Timeout)?;
        // Reply: DA 01 <addr:2> <value:5> [<value:5>]
        if reply.len() < 9 || reply[2..4] != address {
            return Err(LinkError::BadReply(reply));
        }
        Ok(decode35(reply[4..9].try_into().expect("5 bytes")))
    }
}

impl Transport for UdpLink {
    fn send_packet(&self, payload: &[u8]) -> Result<Option<Vec<u8>>> {
        UdpLink::send_packet(self, payload)
    }

    fn send_stream(&self, commands: &[Vec<u8>], progress: &mut dyn FnMut(usize, usize)) -> Result<()> {
        self.send_commands(commands, progress)
    }
}

/// USB connection: the controller's FTDI chip appears as a serial port. Data is scrambled as on
/// Ethernet, but there is no checksum header and no per-packet acknowledgement.
pub struct SerialLink {
    port: std::sync::Mutex<Box<dyn serialport::SerialPort>>,
    swizzle: Swizzle,
}

impl SerialLink {
    pub fn open(port: &str, magic: u8) -> Result<Self> {
        let serial = serialport::new(port, 115_200)
            .flow_control(serialport::FlowControl::Hardware)
            .timeout(Duration::from_millis(250))
            .open()
            .map_err(|e| LinkError::Io(e.into()))?;
        Ok(Self { port: std::sync::Mutex::new(serial), swizzle: Swizzle::new(magic) })
    }
}

impl Transport for SerialLink {
    fn send_packet(&self, payload: &[u8]) -> Result<Option<Vec<u8>>> {
        let mut port = self.port.lock().expect("serial port lock");
        let _ = port.clear(serialport::ClearBuffer::Input);
        port.write_all(&self.swizzle.encode(payload))?;
        port.flush()?;
        if !payload.starts_with(&[0xDA, 0x00]) {
            return Ok(None);
        }
        // Parameter reads answer DA 01 <addr:2> <value:5>. A late answer to an earlier question
        // (another address) is skipped.
        let deadline = Instant::now() + Duration::from_millis(1500);
        loop {
            let mut reply = [0u8; 9];
            let mut got = 0;
            while got < reply.len() {
                if Instant::now() > deadline {
                    return Err(LinkError::Timeout);
                }
                match port.read(&mut reply[got..]) {
                    // Some USB-serial drivers return at once with nothing instead of waiting for
                    // the timeout; pause so this loop doesn't use a whole processor core.
                    Ok(0) => std::thread::sleep(Duration::from_millis(2)),
                    Ok(n) => got += n,
                    Err(e) if e.kind() == io::ErrorKind::TimedOut => std::thread::sleep(Duration::from_millis(2)),
                    Err(e) => return Err(e.into()),
                }
            }
            let reply = self.swizzle.decode(&reply);
            if reply.get(2..4) == payload.get(2..4) {
                return Ok(Some(reply));
            }
        }
    }

    fn send_stream(&self, commands: &[Vec<u8>], progress: &mut dyn FnMut(usize, usize)) -> Result<()> {
        let total: usize = commands.iter().map(Vec::len).sum();
        let mut sent = 0;
        let mut chunk: Vec<u8> = Vec::with_capacity(MAX_PAYLOAD);
        let mut port = self.port.lock().expect("serial port lock");
        for cmd in commands {
            if !chunk.is_empty() && chunk.len() + cmd.len() > MAX_PAYLOAD {
                port.write_all(&self.swizzle.encode(&chunk))?;
                sent += chunk.len();
                progress(sent, total);
                chunk.clear();
            }
            chunk.extend_from_slice(cmd);
        }
        if !chunk.is_empty() {
            port.write_all(&self.swizzle.encode(&chunk))?;
        }
        port.flush()?;
        progress(total, total);
        Ok(())
    }
}

pub struct UdpLink {
    socket: UdpSocket,
    swizzle: Swizzle,
    ack_timeout: Duration,
    retries: u32,
}

impl UdpLink {
    /// Opens the link to a controller at `ip` using the standard ports.
    pub fn connect(ip: Ipv4Addr, magic: u8) -> Result<Self> {
        Self::connect_with(SocketAddr::from((Ipv4Addr::UNSPECIFIED, REPLY_PORT)), SocketAddr::from((ip, CONTROLLER_PORT)), magic)
    }

    /// Opens the link with explicit local and remote addresses (used by tests).
    pub fn connect_with(local: SocketAddr, remote: SocketAddr, magic: u8) -> Result<Self> {
        let socket = UdpSocket::bind(local)?;
        socket.connect(remote)?;
        Ok(Self { socket, swizzle: Swizzle::new(magic), ack_timeout: Duration::from_millis(1500), retries: 3 })
    }

    pub fn set_timeout(&mut self, timeout: Duration) {
        self.ack_timeout = timeout;
    }

    fn recv(&self, deadline: Instant) -> Result<Vec<u8>> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(LinkError::Timeout);
        }
        self.socket.set_read_timeout(Some(remaining))?;
        let mut buf = [0u8; 2048];
        let n = self.socket.recv(&mut buf)?;
        Ok(self.swizzle.decode(&buf[..n]))
    }

    /// Throws away replies still waiting from earlier packets. The controller answers slowly while
    /// the head moves, so after a timeout and resend the first answer can arrive late; read then,
    /// it would be taken as the answer to the next question.
    fn drain_stale_replies(&self) {
        if self.socket.set_nonblocking(true).is_err() {
            return;
        }
        let mut buf = [0u8; 2048];
        while self.socket.recv(&mut buf).is_ok() {}
        let _ = self.socket.set_nonblocking(false);
    }

    /// Sends one packet and waits for its ACK. Any data reply that arrives
    /// alongside the ACK (for parameter reads) is returned.
    pub fn send_packet(&self, payload: &[u8]) -> Result<Option<Vec<u8>>> {
        let packet = udp_packet(&self.swizzle, payload);
        let expects_data = payload.starts_with(&[0xDA, 0x00]);
        self.drain_stale_replies();
        for _ in 0..=self.retries {
            self.socket.send(&packet)?;
            let deadline = Instant::now() + self.ack_timeout;
            let mut acked = false;
            let mut data = None;
            loop {
                let reply = match self.recv(deadline) {
                    Ok(r) => r,
                    Err(LinkError::Timeout) => break,
                    Err(e) => return Err(e),
                };
                match reply.as_slice() {
                    [ACK] => acked = true,
                    [NAK] => break, // checksum failed: resend
                    [ERR] => return Err(LinkError::Rejected("error reply (0xCD)")),
                    [ENQ] => {}     // keep-alive
                    // Only the answer to this question: a late answer to an earlier one (another
                    // address) is ignored.
                    [0xDA, 0x01, a, b, ..] if expects_data && payload.get(2..4) == Some(&[*a, *b][..]) => data = Some(reply),
                    _ => {}         // stray traffic
                }
                if acked && (!expects_data || data.is_some()) {
                    return Ok(data);
                }
            }
            if acked {
                // Acknowledged but the data reply never came.
                return Err(LinkError::Timeout);
            }
        }
        Err(LinkError::Timeout)
    }

    /// Sends a stream of commands, packing whole commands into packets of at most [`MAX_PAYLOAD`] bytes.
    pub fn send_commands(&self, commands: &[Vec<u8>], mut progress: impl FnMut(usize, usize)) -> Result<()> {
        let total: usize = commands.iter().map(Vec::len).sum();
        let mut sent = 0;
        let mut packet: Vec<u8> = Vec::with_capacity(MAX_PAYLOAD);
        for cmd in commands {
            if !packet.is_empty() && packet.len() + cmd.len() > MAX_PAYLOAD {
                self.send_packet(&packet)?;
                sent += packet.len();
                progress(sent, total);
                packet.clear();
            }
            packet.extend_from_slice(cmd);
        }
        if !packet.is_empty() {
            self.send_packet(&packet)?;
            progress(total, total);
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::codec::{encode35, MAGIC_644X};
    use std::sync::{Arc, Mutex};
    use std::thread;

    /// A minimal fake controller on localhost: checks checksums, ACKs, and answers parameter reads.
    pub(crate) struct FakeController {
        pub addr: SocketAddr,
        pub received: Arc<Mutex<Vec<Vec<u8>>>>,
    }

    impl FakeController {
        pub(crate) fn start(nak_first: bool) -> Self {
            Self::start_with(nak_first, false)
        }

        /// With `stale_first`, every parameter read is answered first with a late reply to a
        /// different address, as a busy controller does after a resend.
        pub(crate) fn start_with(nak_first: bool, stale_first: bool) -> Self {
            let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
            let addr = sock.local_addr().unwrap();
            let received = Arc::new(Mutex::new(Vec::new()));
            let log = received.clone();
            thread::spawn(move || {
                let sw = Swizzle::new(MAGIC_644X);
                let mut buf = [0u8; 2048];
                let mut naked = !nak_first;
                while let Ok((n, from)) = sock.recv_from(&mut buf) {
                    let (sum, data) = (u16::from_be_bytes([buf[0], buf[1]]), &buf[2..n]);
                    let ok = data.iter().fold(0u32, |a, &b| a + b as u32) as u16 == sum;
                    if !ok || !naked {
                        naked = true;
                        sock.send_to(&sw.encode(&[NAK]), from).unwrap();
                        continue;
                    }
                    let payload = sw.decode(data);
                    // Record before acknowledging, so a test that checks right after its send sees it.
                    log.lock().unwrap().push(payload.clone());
                    sock.send_to(&sw.encode(&[ACK]), from).unwrap();
                    if payload.starts_with(&[0xDA, 0x00]) {
                        if stale_first {
                            let mut stale = vec![0xDA, 0x01, 0x05, 0x7E];
                            stale.extend_from_slice(&encode35(999));
                            sock.send_to(&sw.encode(&stale), from).unwrap();
                        }
                        let value: i64 = if payload[2..4] == [0x04, 0x21] { 123_456 } else { 7 };
                        let mut reply = vec![0xDA, 0x01, payload[2], payload[3]];
                        reply.extend_from_slice(&encode35(value));
                        reply.extend_from_slice(&encode35(value));
                        sock.send_to(&sw.encode(&reply), from).unwrap();
                    }
                }
            });
            Self { addr, received }
        }

        pub(crate) fn link(&self) -> UdpLink {
            let mut link = UdpLink::connect_with("127.0.0.1:0".parse().unwrap(), self.addr, MAGIC_644X).unwrap();
            link.set_timeout(Duration::from_millis(500));
            link
        }
    }

    #[test]
    fn packet_is_acked() {
        let fake = FakeController::start(false);
        fake.link().send_packet(&[0xD8, 0x2A]).unwrap();
        assert_eq!(fake.received.lock().unwrap().as_slice(), &[vec![0xD8, 0x2A]]);
    }

    #[test]
    fn nak_causes_resend() {
        let fake = FakeController::start(true);
        fake.link().send_packet(&[0xD8, 0x01]).unwrap();
        assert_eq!(fake.received.lock().unwrap().len(), 1);
    }

    #[test]
    fn read_param_returns_value() {
        let fake = FakeController::start(false);
        assert_eq!(fake.link().read_param([0x04, 0x21]).unwrap(), 123_456);
    }

    #[test]
    fn late_answers_to_other_questions_are_ignored() {
        let fake = FakeController::start_with(false, true);
        let link = fake.link();
        assert_eq!(link.read_param([0x04, 0x21]).unwrap(), 123_456);
        assert_eq!(link.read_param([0x04, 0x31]).unwrap(), 7);
    }

    #[test]
    fn leftover_replies_are_cleared_before_asking() {
        let fake = FakeController::start(false);
        let link = fake.link();
        // A reply nobody asked for is waiting on the socket.
        let sw = Swizzle::new(MAGIC_644X);
        let stray = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut stale = vec![0xDA, 0x01, 0x04, 0x21];
        stale.extend_from_slice(&encode35(1));
        stray.send_to(&sw.encode(&stale), link.socket.local_addr().unwrap()).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(link.read_param([0x04, 0x21]).unwrap(), 123_456);
    }

    #[test]
    fn commands_are_packed_on_boundaries() {
        let fake = FakeController::start(false);
        let cmds: Vec<Vec<u8>> = (0..300).map(|_| vec![0xA9, 1, 2, 3, 4]).collect(); // 1500 bytes
        let mut last = (0, 0);
        fake.link().send_commands(&cmds, |s, t| last = (s, t)).unwrap();
        let got = fake.received.lock().unwrap();
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|p| p.len() <= MAX_PAYLOAD && p.len() % 5 == 0));
        assert_eq!(got.concat(), cmds.concat());
        assert_eq!(last, (1500, 1500));
    }

    #[test]
    fn no_controller_times_out() {
        let silent = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut link = UdpLink::connect_with("127.0.0.1:0".parse().unwrap(), silent.local_addr().unwrap(), MAGIC_644X).unwrap();
        link.set_timeout(Duration::from_millis(100));
        assert!(matches!(link.send_packet(&[0xD8, 0x01]), Err(LinkError::Timeout)));
    }
}
