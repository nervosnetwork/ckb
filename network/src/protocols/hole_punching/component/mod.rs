mod connection_request;
mod connection_request_delivered;
mod connection_sync;

pub(crate) use connection_request::ConnectionRequestProcess;
pub(crate) use connection_request_delivered::ConnectionRequestDeliveredProcess;
pub(crate) use connection_sync::ConnectionSyncProcess;

use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};

use ckb_logger::debug;
use ckb_systemtime::Instant;
use p2p::{multiaddr::Multiaddr, runtime, utils::multiaddr_to_socketaddr};
#[cfg(not(target_family = "wasm"))]
use tokio::net::{TcpSocket, TcpStream};

// Attempt to establish a TCP connection with NAT traversal
#[cfg(not(target_family = "wasm"))]
pub(crate) async fn try_nat_traversal(
    bind_addr: Option<SocketAddr>,
    addr: Multiaddr,
) -> Result<(TcpStream, Multiaddr), std::io::Error> {
    let net_addr = match multiaddr_to_socketaddr(&addr) {
        Some(addr) => addr,
        None => {
            debug!("Failed to convert multiaddr to socketaddr");
            return Err(std::io::ErrorKind::InvalidInput.into());
        }
    };
    let now = Instant::now();
    let mut count = 0;
    loop {
        count += 1;
        if count / 5 > 30 && now.elapsed() > Duration::from_secs(30) {
            debug!("NAT traversal timed out");
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        let socket = match bind_addr {
            Some(listen_addr) => match (listen_addr.ip(), net_addr.ip()) {
                (IpAddr::V4(_), IpAddr::V4(_)) => {
                    let socket = TcpSocket::new_v4().unwrap();
                    socket.set_reuseaddr(true).unwrap();
                    #[cfg(all(unix, not(target_os = "solaris"), not(target_os = "illumos")))]
                    socket.set_reuseport(true).unwrap();
                    socket.bind(listen_addr).unwrap();
                    socket
                }
                (IpAddr::V6(_), IpAddr::V6(_)) => {
                    let socket = TcpSocket::new_v6().unwrap();
                    socket.set_reuseaddr(true).unwrap();
                    #[cfg(all(unix, not(target_os = "solaris"), not(target_os = "illumos")))]
                    socket.set_reuseport(true).unwrap();
                    socket.bind(listen_addr).unwrap();
                    socket
                }
                (IpAddr::V4(_), IpAddr::V6(_)) => TcpSocket::new_v6().unwrap(),
                (IpAddr::V6(_), IpAddr::V4(_)) => TcpSocket::new_v4().unwrap(),
            },
            None => match net_addr.ip() {
                IpAddr::V4(_) => TcpSocket::new_v4().unwrap(),
                IpAddr::V6(_) => TcpSocket::new_v6().unwrap(),
            },
        };

        match runtime::timeout(
            std::time::Duration::from_millis(200),
            socket.connect(net_addr),
        )
        .await
        {
            Ok(Ok(stream)) => break Ok((stream, addr)),
            Err(err) => {
                debug!("Failed to connect to NAT: {}", err);
                continue;
            }
            Ok(Err(err)) => {
                if err.kind() == std::io::ErrorKind::AddrNotAvailable {
                    break Err(err);
                }
                debug!("Failed to connect to NAT: {}, {}", err.kind(), err);
                continue;
            }
        }
    }
}
