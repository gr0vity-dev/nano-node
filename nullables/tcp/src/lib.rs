mod tcp_socket;
mod tcp_stream;
mod tcp_stream_factory;

use std::net::{Ipv6Addr, TcpListener};
use std::sync::atomic::{AtomicU16, Ordering};
pub use tcp_socket::*;
pub use tcp_stream::TcpStream;
pub use tcp_stream_factory::TcpStreamFactory;

static START_PORT: AtomicU16 = AtomicU16::new(40_000);

pub fn get_available_port() -> u16 {
    let offset = START_PORT.fetch_add(1, Ordering::SeqCst);
    40_000 + (offset % 20_000)
}
