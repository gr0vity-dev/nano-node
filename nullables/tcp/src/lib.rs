mod tcp_socket;
mod tcp_stream;
mod tcp_stream_factory;

use std::sync::atomic::{AtomicU16, Ordering};
pub use tcp_socket::*;
pub use tcp_stream::TcpStream;
pub use tcp_stream_factory::TcpStreamFactory;

static START_PORT: AtomicU16 = AtomicU16::new(30000);

pub fn get_available_port() -> u16 {
    let start = START_PORT.fetch_add(1, Ordering::SeqCst);
    (start..65535)
        .find(|port| is_port_available(*port))
        .expect("Could not find an available port")
}

fn is_port_available(port: u16) -> bool {
    std::net::TcpListener::bind(("0.0.0.0", port)).is_ok()
}
