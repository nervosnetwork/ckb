use super::client::{Client, Command};
use super::executor::Executor;
use super::peers::Peers;
use super::server::{Request, Server};
use actix::prelude::*;
use futures::Future;
use futures::Stream;
use nervos_chain::chain::ChainClient;
use nervos_notify::Notify;
use nervos_protocol;
use network::protocol::Peer;
use network::Network;
use pool::TransactionPool;
use std::sync::Arc;
use std::thread;
use tokio_core::reactor::Core;
use util::RwLock;

#[derive(Clone)]
pub struct Node<C: ChainClient + 'static> {
    pub network: Arc<Network>,
    pub chain: Arc<C>,
    pub server: Addr<Syn, Server<C>>,
    pub client: Addr<Syn, Client<C>>,
}

impl<C: ChainClient + 'static> Node<C> {
    // /// When new peer connects to the node
    // pub fn on_connect(&self, peer_id: PeerId) {}

    // /// When peer disconnects
    // pub fn on_disconnect(&self, peer_id: PeerId) {}

    pub fn new(
        network: Arc<Network>,
        chain: Arc<C>,
        tx_pool: &Arc<TransactionPool<C>>,
        notify: Notify,
    ) -> Self {
        let peers = Arc::new(RwLock::new(Peers::default()));
        let executor = Arc::new(Executor::new(&network));
        let server = Server::new(&chain, &executor);
        let client = Client::new(&chain, &executor, &peers, tx_pool, notify);

        Node {
            network,
            chain,
            server,
            client,
        }
    }

    pub fn start(&self) {
        let network_clone = Arc::clone(&self.network);
        let chain_clone = Arc::clone(&self.chain);
        let server_clone = self.server.clone();
        let client_clone = self.client.clone();
        let _ = thread::Builder::new()
            .name("network".to_string())
            .spawn(move || {
                let mut core = Core::new().unwrap();
                let (network_reciver, network_future) =
                    network_clone.start(core.handle(), chain_clone);
                let server = server_clone.clone();
                let client = client_clone.clone();
                let network_reciver = network_reciver.for_each(move |msg| {
                    info!(target: "sync", "received msg from {:?}", msg.peer);
                    on_message(&server, &client, msg.payload, msg.peer);
                    Ok(())
                });
                core.run(
                    network_future
                        .select(network_reciver)
                        .map_err(|(err, _)| err)
                        .map(|((), _)| ()),
                )
            });
    }
}

fn on_message<C: ChainClient + 'static>(
    server: &Addr<Syn, Server<C>>,
    client: &Addr<Syn, Client<C>>,
    mut input: nervos_protocol::Payload,
    source: Peer,
) {
    if input.has_getheaders() {
        let getheaders = input.take_getheaders();
        let request = Request::GetHeaders(source, getheaders);
        server.do_send(request);
    } else if input.has_headers() {
        let headers = input.take_headers();
        let command = Command::OnHeaders(source, headers);
        client.do_send(command);
    } else if input.has_getdata() {
        let getdata = input.take_getdata();
        let request = Request::GetData(source, getdata);
        server.do_send(request);
    } else if input.has_transaction() {
        let transaction = input.take_transaction();
        let command = Command::OnTransaction(source, transaction);
        client.do_send(command);
    } else if input.has_block() {
        let block = input.take_block();
        let command = Command::OnBlock(source, block);
        client.do_send(command);
    }
}
