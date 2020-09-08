use std::{net::SocketAddr, sync::Arc};

use tokio::{net::TcpListener, sync::RwLock};

use ckb_logger::{debug, error};

use crate::client;

pub(crate) async fn async_run(socket_addr: SocketAddr) {
    match TcpListener::bind(&socket_addr).await {
        Ok(mut listener) => {
            debug!("bind tcp listener at {}", socket_addr);
            let has_client = Arc::new(RwLock::new(false));
            loop {
                match listener.accept().await {
                    Ok((socket, addr)) => {
                        let has_client_now = *has_client.read().await;
                        if has_client_now {
                            tokio::spawn(client::reject(socket, addr));
                        } else {
                            *has_client.write().await = true;
                            let has_client_clone = Arc::clone(&has_client);
                            tokio::spawn(client::serve(socket, addr, has_client_clone));
                        }
                    }
                    Err(error) => {
                        error!("failed to accept client since {}", error);
                    }
                }
            }
        }
        Err(error) => {
            error!(
                "failed to bind tcp listener at {} since {}",
                socket_addr, error
            );
        }
    }
}
