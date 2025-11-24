mod tcp_socket;
mod tcp_stream;
mod tcp_stream_factory;

use std::sync::atomic::{AtomicU16, Ordering};
pub use tcp_socket::*;
pub use tcp_stream::TcpStream;
pub use tcp_stream_factory::TcpStreamFactory;

pub fn get_available_port() -> u16 {
    // Use a simple monotonic counter so tests do not rely on OS ephemeral port
    // allocation, which can fail under sandboxed permissions.
    static NEXT_PORT: AtomicU16 = AtomicU16::new(30_000);
    NEXT_PORT.fetch_add(1, Ordering::Relaxed)
}
