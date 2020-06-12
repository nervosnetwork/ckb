use ckb_app_config::{ExitCode, PeerIDArgs};

pub fn peer_id(args: PeerIDArgs) -> Result<(), ExitCode> {
    println!("peer_id: {}", args.peer_id.to_base58());
    Ok(())
}
