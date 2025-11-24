mod tcp_socket;
mod tcp_stream;
mod tcp_stream_factory;

use std::net::TcpListener;
pub use tcp_socket::*;
pub use tcp_stream::TcpStream;
pub use tcp_stream_factory::TcpStreamFactory;

pub fn get_available_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("unable to bind ephemeral port")
        .local_addr()
        .expect("unable to read ephemeral port")
        .port()
}
