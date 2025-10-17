#![allow(dead_code)]
use alloc::collections::{BTreeMap, VecDeque};
use alloc::{format, vec};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;
use smoltcp::iface::SocketSet;
use smoltcp::phy::Device;
use smoltcp::socket::tcp;
use smoltcp::time::Instant;
use smoltcp::wire::IpAddress;
use crate::driver::nic::NET;
use crate::driver::timer::cmos::CMOS;
use crate::{println};
use crate::task::executor::sleep;

use time::{Duration, OffsetDateTime, UtcOffset};
use crate::sys::fs::vfs::VFS;

const MAX_CONNECTIONS: usize = 32; // TODO: Add dynamic pooling
const POLL_DELAY_DIV: usize = 128;
const INDEX: [&str; 4] = ["", "/index.html", "/index.htm", "/index.txt"];

pub const DATE_TIME_ZONE: &str = "%Y-%m-%d %H:%M:%S %z";

pub fn now_utc() -> OffsetDateTime {
    let mut cmos = CMOS::new();
    let s = cmos.unix_time(); // Since Unix Epoch
    let ns = Duration::nanoseconds(
        libm::floor(1e9 * (s as f64 - libm::floor(s as f64))) as i64
    );
    OffsetDateTime::from_unix_timestamp(s as i64) + ns
}


pub fn now() -> OffsetDateTime {
    now_utc().to_offset(offset())
}


fn offset() -> UtcOffset {
    UtcOffset::UTC
}


#[derive(Clone)]
struct Request {
    addr: IpAddress,
    verb: String,
    path: String,
    body: Vec<u8>,
    headers: BTreeMap<String, String>,
}


impl Request {
    pub fn new(addr: IpAddress) -> Self {
        Self {
            addr,
            verb: String::new(),
            path: String::new(),
            body: Vec::new(),
            headers: BTreeMap::new(),
        }
    }

    pub fn from(addr: IpAddress, buf: &[u8]) -> Option<Self> {
        let msg = String::from_utf8_lossy(buf);
        if !msg.is_empty() {
            let mut req = Request::new(addr);
            let mut is_header = true;
            for (i, line) in msg.lines().enumerate() {
                if i == 0 {
                    // Request line
                    let fields: Vec<_> = line.split(' ').collect();
                    if fields.len() >= 2 {
                        req.verb = fields[0].to_string();
                        req.path = fields[1].to_string();
                    }
                } else if is_header {
                    // Message header
                    if let Some((key, val)) = line.split_once(':') {
                        let k = key.trim().to_string();
                        let v = val.trim().to_string();
                        req.headers.insert(k, v);
                    } else if line.is_empty() {
                        is_header = false;
                    }
                } else if !is_header {
                    // Message body
                    let s = format!("{}\n", line);
                    req.body.extend_from_slice(s.as_bytes());
                }
            }
            Some(req)
        } else {
            None
        }
    }
}


#[derive(Clone)]
struct Response {
    req: Request,
    buf: Vec<u8>,
    mime: String,
    time: String,
    code: usize,
    size: usize,
    body: Vec<u8>,
    headers: BTreeMap<String, String>,
    real_path: String,
}

impl Response {
    pub fn new(req: Request) -> Self {
        let mut headers = BTreeMap::new();
        headers.insert(
            "Date".to_string(),
            now_utc().format("%a, %d %b %Y %H:%M:%S GMT"),
        );
        headers.insert(
            "Server".to_string(),
            format!("MOROS/{}", env!("CARGO_PKG_VERSION")),
        );
        Self {
            req,
            buf: Vec::new(),
            mime: String::new(),
            time: now().format(DATE_TIME_ZONE),
            code: 0,
            size: 0,
            body: Vec::new(),
            headers,
            real_path: String::new(),
        }
    }

    pub fn end(&mut self) {
        self.size = self.body.len();
        self.headers.insert(
            "Content-Length".to_string(),
            self.size.to_string()
        );
        self.headers.insert(
            "Connection".to_string(),
            if self.is_persistent() {
                "keep-alive".to_string()
            } else {
                "close".to_string()
            }
        );
        self.headers.insert(
            "Content-Type".to_string(),
            if self.mime.starts_with("text/") {
                format!("{}; charset=utf-8", self.mime)
            } else {
                format!("{}", self.mime)
            }
        );
        self.write();
    }

    fn write(&mut self) {
        self.buf.clear();
        self.buf.extend_from_slice(
            format!("{}\r\n", self.status()).as_bytes()
        );
        for (key, val) in &self.headers {
            self.buf.extend_from_slice(
                format!("{}: {}\r\n", key, val).as_bytes()
            );
        }
        self.buf.extend_from_slice(b"\r\n");
        self.buf.extend_from_slice(&self.body);
    }

    fn status(&self) -> String {
        let msg = match self.code {
            200 => "OK",
            301 => "Moved Permanently",
            400 => "Bad Request",
            403 => "Forbidden",
            404 => "Not Found",
            500 => "Internal Server Error",
            _ => "Unknown Error",
        };
        format!("HTTP/1.1 {} {}", self.code, msg)
    }

    fn is_persistent(&self) -> bool {
        if let Some(value) = self.req.headers.get("Connection") {
            if value == "close" {
                return false;
            }
        }
        true
    }
}

impl fmt::Display for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let csi_blue = "\x1b[34m";   // Blue
        let csi_cyan = "\x1b[36m";   // Cyan
        let csi_pink = "\x1b[35m";   // Magenta
        let csi_reset = "\x1b[0m";

        write!(
            f,
            "{}{} - -{} [{}] {}\"{} {}\"{} {} {}",
            csi_cyan,
            self.req.addr,
            csi_pink,
            self.time,
            csi_blue,
            self.req.verb,
            self.req.path,
            csi_reset,
            self.code,
            self.size
        )
    }
}


fn get(req: &Request, res: &mut Response) {
    res.mime = "text/html".to_string();
    let path = req.path.as_str();
    let mut file = if path.is_empty() || path == "/" {
        "index.html"
    } else {
        path
    };

    file = file.trim_start_matches("/");
    #[allow(static_mut_refs)]
    let vfs = unsafe { VFS.get_mut() };
    let file_path = format!("/var/www/{}", file);
    println!("file_path: {}", file_path);
    if let Ok(mut inode) = vfs.open(file_path.as_str()) {
        res.code = 200;
        let Ok(buf) = inode.read() else {
            res.body.extend_from_slice(b"<h1>Hello World From Twilight OS!</h1>\n");
            return;
        };

        res.body.extend_from_slice(buf.as_slice());
    } else {
        res.code = 404;
        res.body.extend_from_slice(b"<h1>404 Not Found</h1>\n");
    }
}

pub fn main(_args: &[&str]) {
    if let Some((ref mut iface, ref mut device)) = *NET.lock() {
        let mut sockets = SocketSet::new(vec![]);

        let mtu = device.capabilities().max_transmission_unit;
        let buf_len = mtu - 14 - 20 - 20; // ETH+TCP+IP headers
        let mut connections = Vec::new();

        for _ in 0..MAX_CONNECTIONS {
            let tcp_rx_buffer = tcp::SocketBuffer::new(vec![0; buf_len]);
            let tcp_tx_buffer = tcp::SocketBuffer::new(vec![0; buf_len]);
            let tcp_socket = tcp::Socket::new(tcp_rx_buffer, tcp_tx_buffer);
            let tcp_handle = sockets.add(tcp_socket);

            let send_queue: VecDeque<Vec<u8>> = VecDeque::new();
            let keep_alive = true;
            connections.push((tcp_handle, send_queue, keep_alive));
        }
        let csi_color = "\x1b[33m";
        let port = 80;
        let csi_reset = "\x1b[0m";

        println!(
            "{}HTTP Server listening on 0.0.0.0:{}{}",
            csi_color, port, csi_reset
        );
        let mut cmos = CMOS::new();
        loop {
            let ms = (cmos.unix_time() as f64 * 1000000.0) as i64;
            let time = Instant::from_micros(ms);
            iface.poll(time, device, &mut sockets);

            for (tcp_handle, send_queue, keep_alive) in &mut connections {
                let socket = sockets.get_mut::<tcp::Socket>(*tcp_handle);

                if !socket.is_open() {
                    socket.listen(port).unwrap();
                    *keep_alive = true; // Reset to default
                }

                let endpoint = match socket.remote_endpoint() {
                    Some(endpoint) => endpoint,
                    None => continue,
                };

                if socket.may_recv() {
                    // The amount of octets queued in the receive buffer may be
                    // larger than the contiguous slice returned by `recv` so
                    // we need to loop over chunks of it until it is empty.
                    let recv_queue = socket.recv_queue();
                    let mut buf = vec![];
                    let mut receiving = true;
                    while receiving {
                        let res = socket.recv(|chunk| {
                            buf.extend_from_slice(chunk);
                            if buf.len() < recv_queue {
                                return (chunk.len(), None);
                            }
                            receiving = false;

                            let addr = endpoint.addr;
                            if let Some(req) = Request::from(addr, &buf) {
                                let mut res = Response::new(req.clone());

                                match req.verb.as_str() {
                                    "GET" => {
                                        get(&req, &mut res)
                                    }
                                    _ => {
                                        let s = b"<h1>Bad Request</h1>\n";
                                        res.body.extend_from_slice(s);
                                        res.code = 400;
                                        res.mime = "text/html".to_string();
                                    }
                                }
                                res.end();
                                println!("{}", res);
                                (chunk.len(), Some(res))
                            } else {
                                (0, None) // (chunk.len(), None) // TODO?
                            }
                        });
                        if receiving {
                            continue;
                        }
                        if let Ok(Some(res)) = res {
                            *keep_alive = res.is_persistent();
                            for chunk in res.buf.chunks(buf_len) {
                                send_queue.push_back(chunk.to_vec());
                            }
                        }
                    }

                    if socket.can_send() {
                        if let Some(chunk) = send_queue.pop_front() {
                            if socket.send_slice(&chunk).is_err() {
                                // send_queue.push_front(chunk); // TODO?
                            }
                        }
                    }
                    if send_queue.is_empty() && !*keep_alive {
                        socket.close();
                    }
                } else if socket.may_send() {
                    socket.close();
                    send_queue.clear(); // TODO: Remove this?
                }
            }

            if let Some(delay) = iface.poll_delay(time, &sockets) {
                let d = delay.total_micros() / POLL_DELAY_DIV as u64;
                if d > 0 {
                    sleep((d as f64) / 1000000.0);
                }
            }
        }
    }
}