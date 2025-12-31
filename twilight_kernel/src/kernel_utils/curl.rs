use crate::driver::disk::ata::FileIO;
use crate::println;
use crate::sys::net::socket::tcp::TcpSocket;
use crate::sys::net::socket::udp::UdpSocket;
use crate::sys::rng;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::{format, vec};
use bit_field::BitField;
use core::str::FromStr;
use smoltcp::wire::{IpAddress, Ipv4Address};

pub fn main(args: &[&str]) {
    if args.len() < 2 {
        println!("Usage: curl [url]");
        return;
    }
    let host = args[1]
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    let url_path = format!("http://{}", host);

    let Some(url) = URL::parse(&url_path) else {
        return;
    };
    let port = url.port;

    let addr = if url.host.ends_with(char::is_numeric) {
        match IpAddress::from_str(url.path.as_str()) {
            Ok(addr) => addr,
            Err(_) => {
                println!("Invalid address format!");
                return;
            }
        }
    } else {
        match resolve(&url.host) {
            Ok(ip_addr) => ip_addr,
            Err(e) => {
                println!("Could not resolve host: {:?}", e);
                return;
            }
        }
    };

    let mut code = None;
    let mut tcp_socket = TcpSocket::new();
    let buf_len = TcpSocket::size();

    if tcp_socket.connect(addr, port).is_err() {
        println!("Could not connect to {}:{}", addr, port);
        return;
    }
    let req = vec![
        format!("GET {} HTTP/1.1\r\n", url.path),
        format!("Host: {}\r\n", url.host),
        format!("User-Agent: TwilightOS/{}\r\n", env!("CARGO_PKG_VERSION")),
        "Connection: close\r\n".to_string(),
        "\r\n".to_string(),
    ];
    let req = req.join("");
    if let Err(()) = tcp_socket.write(req.as_bytes()) {
        println!("Could not write to socket");
        return;
    };

    let mut state = ResponseState::Headers;
    loop {
        let mut data = vec![0; buf_len];
        if let Ok(n) = tcp_socket.read(&mut data) {
            if n == 0 {
                break;
            }
            data.resize(n, 0);
            let mut i = 0;
            while i < n {
                match state {
                    ResponseState::Headers => {
                        let mut j = i;
                        while j < n {
                            if data[j] == b'\n' {
                                break;
                            }
                            j += 1;
                        }
                        // TODO: check i == j
                        let line = String::from_utf8_lossy(&data[i..j]);
                        if i == 0 {
                            code = line.split(" ").nth(1).map(|word| word.to_string());
                        }
                        if line.trim().is_empty() {
                            state = ResponseState::Body;
                        }
                        i = j + 1;
                    }
                    ResponseState::Body => {
                        // NOTE: The buffer may not be convertible to a
                        // UTF-8 string so we write it to STDOUT directly
                        // instead of using print.
                        println!("{}", String::from_utf8_lossy(&data[i..n]));
                        break;
                    }
                }
            }
        } else {
            println!("Could not read from {}:{}", addr, port);
            return;
        }
    }
    if let Some(s) = code {
        if let Ok(n) = s.parse::<usize>() {
            if n < 400 {
                return;
            }
        }
    }
}

#[repr(u16)]
enum QueryType {
    A = 1,
    // NS = 2,
    // MD = 3,
    // MF = 4,
    // CNAME = 5,
    // SOA = 6,
    // MX = 15,
    // TXT = 16,
}

#[repr(u16)]
enum QueryClass {
    IN = 1,
}

struct Message {
    pub datagram: Vec<u8>,
}

#[derive(Debug)]
#[repr(u16)]
pub enum ResponseCode {
    NoError = 0,
    FormatError = 1,
    ServerFailure = 2,
    NameError = 3,
    NotImplemented = 4,
    Refused = 5,

    UnknownError,
    NetworkError,
}

const FLAG_RD: u16 = 0x0100; // Recursion desired

impl Message {
    pub fn from(datagram: &[u8]) -> Self {
        Self {
            datagram: Vec::from(datagram),
        }
    }

    pub fn query(qname: &str, qtype: QueryType, qclass: QueryClass) -> Self {
        let mut datagram = Vec::new();

        let id = rng::get_u16();
        for b in id.to_be_bytes().iter() {
            datagram.push(*b); // Transaction ID
        }
        for b in FLAG_RD.to_be_bytes().iter() {
            datagram.push(*b); // Flags
        }
        for b in (1 as u16).to_be_bytes().iter() {
            datagram.push(*b); // Questions
        }
        for _ in 0..6 {
            datagram.push(0); // Answer + Authority + Additional
        }
        for label in qname.split('.') {
            datagram.push(label.len() as u8); // QNAME label length
            for b in label.bytes() {
                datagram.push(b); // QNAME label bytes
            }
        }
        datagram.push(0); // Root null label
        for b in (qtype as u16).to_be_bytes().iter() {
            datagram.push(*b); // QTYPE
        }
        for b in (qclass as u16).to_be_bytes().iter() {
            datagram.push(*b); // QCLASS
        }

        Self { datagram }
    }

    pub fn id(&self) -> u16 {
        u16::from_be_bytes(self.datagram[0..2].try_into().unwrap())
    }

    pub fn header(&self) -> u16 {
        u16::from_be_bytes(self.datagram[2..4].try_into().unwrap())
    }

    pub fn is_response(&self) -> bool {
        self.header().get_bit(15)
    }

    pub fn code(&self) -> ResponseCode {
        match self.header().get_bits(11..15) {
            0 => ResponseCode::NoError,
            1 => ResponseCode::FormatError,
            2 => ResponseCode::ServerFailure,
            3 => ResponseCode::NameError,
            4 => ResponseCode::NotImplemented,
            5 => ResponseCode::Refused,
            _ => ResponseCode::UnknownError,
        }
    }
}

pub fn resolve(name: &str) -> Result<IpAddress, ResponseCode> {
    let addr = IpAddress::v4(8, 8, 8, 8);
    let port = 53;
    let query = Message::query(name, QueryType::A, QueryClass::IN);

    let mut udp_socket = UdpSocket::new();

    let buf_len = UdpSocket::size();

    if udp_socket.connect(addr, port).is_err() {
        return Err(ResponseCode::NetworkError);
    }
    if udp_socket.write(&query.datagram).is_err() {
        return Err(ResponseCode::NetworkError);
    }
    loop {
        let mut data = vec![0; buf_len];
        if let Ok(bytes) = udp_socket.read(&mut data) {
            if bytes < 28 {
                break;
            }
            data.resize(bytes, 0);
            let message = Message::from(&data);
            if message.id() == query.id() && message.is_response() {
                //usr::hex::print_hex(&message.datagram);
                return match message.code() {
                    ResponseCode::NoError => {
                        // TODO: Parse the datagram instead of extracting
                        // the last 4 bytes
                        let n = message.datagram.len();
                        let data = &message.datagram[(n - 4)..];
                        if let Ok(data) = data.try_into() {
                            let ipv4 = Ipv4Address::from_octets(data);
                            if ipv4.is_unspecified() {
                                Err(ResponseCode::NameError) // FIXME
                            } else {
                                Ok(IpAddress::from(ipv4))
                            }
                        } else {
                            Err(ResponseCode::NameError) // FIXME
                        }
                    }
                    code => Err(code),
                };
            }
        } else {
            break;
        }
    }
    println!("Could not read from {}", name);
    Err(ResponseCode::NetworkError)
}

#[derive(Debug)]
struct URL {
    pub host: String,
    pub port: u16,
    pub path: String,
}

enum ResponseState {
    Headers,
    Body,
}
impl URL {
    pub fn parse(url: &str) -> Option<Self> {
        let (scheme, rest) = if let Some(r) = url.strip_prefix("https://") {
            ("https", r)
        } else if let Some(r) = url.strip_prefix("http://") {
            ("http", r)
        } else {
            return None;
        };

        let (server, path) = match rest.find('/') {
            Some(i) => rest.split_at(i),
            None => (rest, "/"),
        };

        let (host, port_str) = match server.find(':') {
            Some(i) => {
                let (h, p) = server.split_at(i);
                (h, p.strip_prefix(':').unwrap_or(""))
            }
            None => (server, if scheme == "https" { "443" } else { "80" }),
        };

        let port = port_str
            .parse()
            .unwrap_or(if scheme == "https" { 443 } else { 80 });

        Some(Self {
            host: host.into(),
            port,
            path: path.into(),
        })
    }
}
