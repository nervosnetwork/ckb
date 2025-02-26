use ckb_network::PeerId;

#[tokio::test]
async fn test_start_onion_by_controller() {
    // let tmp_dir = tempfile::tempdir().unwrap();
    // let config = crate::OnionServiceConfig {
    //     tor_controller: "127.0.0.1:9051".to_string(),
    //     onion_private_key_path: tmp_dir
    //         .path()
    //         .join("test_tor_secret_path")
    //         .to_string_lossy()
    //         .to_string(),
    //     onion_server: "127.0.0.:9050".to_string(),
    //     tor_password: None,
    //     onion_service_target: "127.0.0.1:9051".parse().unwrap(),
    // };
    // let handle = ckb_async_runtime::new_background_runtime();
    // let node_id = PeerId::random().to_base58();
    // let (onion_service, _onion_service_addr) =
    //     crate::onion_service::OnionService::new(handle, config, node_id).unwrap();
    // onion_service.start().await.unwrap();
}
