mod tcp_socket;
mod tcp_stream;
mod tcp_stream_factory;

use std::sync::atomic::{AtomicU16, Ordering};
pub use tcp_socket::*;
pub use tcp_stream::TcpStream;
pub use tcp_stream_factory::TcpStreamFactory;

static START_PORT: AtomicU16 = AtomicU16::new(1025);

pub fn get_available_port() -> u16 {
    // Probe through the non-privileged port range; wrap safely on overflow.
    for _ in 0..u16::MAX {
        let offset = START_PORT.fetch_add(1, Ordering::SeqCst);
        let port = 1025 + (offset % (u16::MAX - 1025));
        if is_port_available(port) {
            return port;
        }
    }
    panic!("Could not find an available port");
}

fn is_port_available(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}
