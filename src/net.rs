use std::io;
use std::net::SocketAddr;

use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;

pub const UDP_SOCKET_BUFFER_BYTES: usize = 8 * 1024 * 1024;

pub fn bind_udp_socket(addr: SocketAddr) -> io::Result<UdpSocket> {
    let domain = match addr {
        SocketAddr::V4(_) => Domain::IPV4,
        SocketAddr::V6(_) => Domain::IPV6,
    };

    let socket = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_recv_buffer_size(UDP_SOCKET_BUFFER_BYTES)?;
    socket.set_send_buffer_size(UDP_SOCKET_BUFFER_BYTES)?;
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;

    let std_socket: std::net::UdpSocket = socket.into();
    UdpSocket::from_std(std_socket)
}
