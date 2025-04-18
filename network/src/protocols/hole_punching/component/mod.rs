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
use ckb_types::{packed, prelude::*};
use p2p::{multiaddr::Multiaddr, runtime, utils::multiaddr_to_socketaddr};
#[cfg(not(target_family = "wasm"))]
use tokio::net::{TcpSocket, TcpStream};

use crate::{PeerId, protocols::hole_punching::MAX_TTL};

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

pub(crate) fn init_request(
    from: &PeerId,
    to: &PeerId,
    listen_addrs: packed::AddressVec,
) -> packed::ConnectionRequest {
    let new_route = packed::BytesVec::new_builder()
        .push(from.as_bytes().pack())
        .build();
    packed::ConnectionRequest::new_builder()
        .from(from.as_bytes().pack())
        .to(to.as_bytes().pack())
        .ttl(MAX_TTL.into())
        .listen_addrs(listen_addrs)
        .route(new_route)
        .build()
}

pub(crate) fn forward_request(
    request: packed::ConnectionRequestReader<'_>,
    current_id: &PeerId,
) -> packed::ConnectionRequest {
    let ttl: u8 = request.ttl().into();
    let message = request.to_entity();
    let new_route = message
        .route()
        .as_builder()
        .push(current_id.as_bytes().pack())
        .build();
    message
        .as_builder()
        .ttl((ttl - 1).into())
        .route(new_route)
        .build()
}

pub(crate) fn init_delivered(
    request: packed::ConnectionRequestReader<'_>,
    listen_addrs: packed::AddressVec,
) -> packed::ConnectionRequestDelivered {
    let route = request.route();
    let message = request.to_entity();
    let new_route = packed::BytesVec::new_builder()
        .extend(message.route().into_iter().take(route.len() - 1))
        .build();
    let sync_route = packed::BytesVec::new_builder()
        .extend(
            message
                .route()
                .into_iter()
                .chain(vec![message.to()])
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .take(route.len()),
        )
        .build();
    packed::ConnectionRequestDelivered::new_builder()
        .from(message.from())
        .to(message.to())
        .route(new_route)
        .sync_route(sync_route)
        .listen_addrs(listen_addrs)
        .build()
}

pub(crate) fn forward_delivered(
    delivered: packed::ConnectionRequestDeliveredReader<'_>,
) -> packed::ConnectionRequestDelivered {
    let route = delivered.route();
    let message = delivered.to_entity();
    let new_route = packed::BytesVec::new_builder()
        .extend(message.route().into_iter().take(route.len() - 1))
        .build();
    message.as_builder().route(new_route).build()
}

pub(crate) fn init_sync(
    delivered: packed::ConnectionRequestDeliveredReader<'_>,
) -> packed::ConnectionSync {
    let sync_route = delivered.sync_route();
    let message = delivered.to_entity();
    let new_route = packed::BytesVec::new_builder()
        .extend(message.sync_route().into_iter().take(sync_route.len() - 1))
        .build();
    packed::ConnectionSync::new_builder()
        .from(message.from())
        .to(message.to())
        .route(new_route)
        .build()
}

pub(crate) fn forward_sync(sync: packed::ConnectionSyncReader<'_>) -> packed::ConnectionSync {
    let route = sync.route();
    let message = sync.to_entity();
    let new_route = packed::BytesVec::new_builder()
        .extend(message.route().into_iter().take(route.len() - 1))
        .build();
    message.as_builder().route(new_route).build()
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::protocols::hole_punching::MAX_TTL;
    use ckb_types::packed;

    #[test]
    fn test_route() {
        // Simulate the entire message flow from from to to, passing through forward_a, forward_b.
        let from = PeerId::random();
        let to = PeerId::random();
        let forward_a = PeerId::random();
        let forward_b = PeerId::random();

        // empty listen addrs
        let listen_addrs = packed::AddressVec::new_builder().build();

        let init_request = init_request(&from, &to, listen_addrs.clone());

        assert_eq!(init_request.from(), from.as_bytes().pack());
        assert_eq!(init_request.to(), to.as_bytes().pack());
        assert_eq!(init_request.ttl(), MAX_TTL.into());
        // from is in the route
        assert_eq!(
            init_request.route().as_bytes(),
            packed::BytesVec::new_builder()
                .push(from.as_bytes().pack())
                .build()
                .as_bytes()
        );

        // in forward_a
        let forward_request_a = forward_request(init_request.as_reader(), &forward_a);
        assert_eq!(forward_request_a.from(), from.as_bytes().pack());
        assert_eq!(forward_request_a.to(), to.as_bytes().pack());
        assert_eq!(forward_request_a.ttl(), (MAX_TTL - 1).into());
        // forward_a is in the route
        assert_eq!(
            forward_request_a.route().as_bytes(),
            packed::BytesVec::new_builder()
                .push(from.as_bytes().pack())
                .push(forward_a.as_bytes().pack())
                .build()
                .as_bytes()
        );

        // in forward_b
        let forward_request_b = forward_request(forward_request_a.as_reader(), &forward_b);
        assert_eq!(forward_request_b.from(), from.as_bytes().pack());
        assert_eq!(forward_request_b.to(), to.as_bytes().pack());
        assert_eq!(forward_request_b.ttl(), (MAX_TTL - 2).into());
        // forward_b is in the route
        assert_eq!(
            forward_request_b.route().as_bytes(),
            packed::BytesVec::new_builder()
                .push(from.as_bytes().pack())
                .push(forward_a.as_bytes().pack())
                .push(forward_b.as_bytes().pack())
                .build()
                .as_bytes()
        );

        // in to
        let init_delivered = init_delivered(forward_request_b.as_reader(), listen_addrs);
        assert_eq!(init_delivered.from(), from.as_bytes().pack());
        assert_eq!(init_delivered.to(), to.as_bytes().pack());
        // forward_b is not in the route
        assert_eq!(
            init_delivered.route().as_bytes(),
            packed::BytesVec::new_builder()
                .push(from.as_bytes().pack())
                .push(forward_a.as_bytes().pack())
                .build()
                .as_bytes()
        );
        // sync route is to <- forward_b <- forward_a
        assert_eq!(
            init_delivered.sync_route().as_bytes(),
            packed::BytesVec::new_builder()
                .push(to.as_bytes().pack())
                .push(forward_b.as_bytes().pack())
                .push(forward_a.as_bytes().pack())
                .build()
                .as_bytes()
        );

        // now we can start to send back the delivered message to the from

        // in forward_b
        assert_eq!(
            init_delivered
                .as_reader()
                .route()
                .iter()
                .last()
                .unwrap()
                .as_slice(),
            forward_a.as_bytes().pack().as_slice()
        );
        let forward_delivered_b = forward_delivered(init_delivered.as_reader());
        assert_eq!(forward_delivered_b.from(), from.as_bytes().pack());
        assert_eq!(forward_delivered_b.to(), to.as_bytes().pack());
        assert_eq!(
            forward_delivered_b.route().as_bytes(),
            packed::BytesVec::new_builder()
                .push(from.as_bytes().pack())
                .build()
                .as_bytes()
        );
        assert_eq!(
            forward_delivered_b.sync_route().as_bytes(),
            init_delivered.sync_route().as_bytes()
        );

        // in forward_a
        assert_eq!(
            forward_delivered_b
                .as_reader()
                .route()
                .iter()
                .last()
                .unwrap()
                .as_slice(),
            from.as_bytes().pack().as_slice()
        );
        let forward_delivered_a = forward_delivered(forward_delivered_b.as_reader());
        assert_eq!(forward_delivered_a.from(), from.as_bytes().pack());
        assert_eq!(forward_delivered_a.to(), to.as_bytes().pack());
        assert_eq!(
            forward_delivered_a.route().as_bytes(),
            packed::BytesVec::new_builder().build().as_bytes()
        );
        assert_eq!(
            forward_delivered_a.sync_route().as_bytes(),
            init_delivered.sync_route().as_bytes()
        );

        // in from
        assert!(
            forward_delivered_a
                .as_reader()
                .route()
                .iter()
                .last()
                .is_none()
        );
        let init_sync = init_sync(forward_delivered_a.as_reader());
        assert_eq!(init_sync.from(), from.as_bytes().pack());
        assert_eq!(init_sync.to(), to.as_bytes().pack());
        assert_eq!(
            init_sync.route().as_bytes(),
            packed::BytesVec::new_builder()
                .push(to.as_bytes().pack())
                .push(forward_b.as_bytes().pack())
                .build()
                .as_bytes()
        );

        // now we can start to send back the sync message to the to

        // in forward_a
        assert_eq!(
            init_sync
                .as_reader()
                .route()
                .iter()
                .last()
                .unwrap()
                .as_slice(),
            forward_b.as_bytes().pack().as_slice()
        );
        let forward_sync_a = forward_sync(init_sync.as_reader());
        assert_eq!(forward_sync_a.from(), from.as_bytes().pack());
        assert_eq!(forward_sync_a.to(), to.as_bytes().pack());
        assert_eq!(
            forward_sync_a.route().as_bytes(),
            packed::BytesVec::new_builder()
                .push(to.as_bytes().pack())
                .build()
                .as_bytes()
        );

        // in forward_b
        assert_eq!(
            forward_sync_a
                .as_reader()
                .route()
                .iter()
                .last()
                .unwrap()
                .as_slice(),
            to.as_bytes().pack().as_slice()
        );
        let forward_sync_b = forward_sync(forward_sync_a.as_reader());
        assert_eq!(forward_sync_b.from(), from.as_bytes().pack());
        assert_eq!(forward_sync_b.to(), to.as_bytes().pack());
        assert_eq!(
            forward_sync_b.route().as_bytes(),
            packed::BytesVec::new_builder().build().as_bytes()
        );

        // in to
        assert!(forward_sync_b.as_reader().route().iter().last().is_none());
    }
}
