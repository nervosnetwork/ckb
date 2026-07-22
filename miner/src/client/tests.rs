use super::*;
use ckb_async_runtime::new_background_runtime;
use ckb_channel::bounded;
use ckb_jsonrpc_types::BlockTemplate;
use http_body_util::Full;
use hyper::body::Bytes;

fn test_client(auth_token: Option<String>) -> (Client, ckb_channel::Receiver<Works>) {
    let runtime = new_background_runtime();
    let (new_work_tx, new_work_rx) = bounded::<Works>(16);
    let config = MinerClientConfig {
        rpc_url: "http://127.0.0.1:8114/".to_string(),
        poll_interval: 1000,
        block_on_submit: false,
        listen: None,
        auth_token,
    };
    let client = Client::new(new_work_tx, config, runtime);
    (client, new_work_rx)
}

fn notify_request(method: &str, auth_header: Option<&str>, body: Bytes) -> Request<Full<Bytes>> {
    let mut builder = Request::builder()
        .method(method)
        .uri("http://127.0.0.1:8888/");
    if let Some(header) = auth_header {
        builder = builder.header(hyper::header::AUTHORIZATION, header);
    }
    builder.body(Full::new(body)).unwrap()
}

#[tokio::test]
async fn notify_rejects_non_post_requests() {
    let (client, _rx) = test_client(None);
    let req = notify_request("GET", None, Bytes::new());
    let resp = handle(client, req).await.unwrap();
    assert_eq!(resp.status(), hyper::StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(resp.headers().get(hyper::header::ALLOW).unwrap(), "POST");
}

#[tokio::test]
async fn notify_accepts_unauthenticated_requests_when_no_token_configured() {
    let (client, _rx) = test_client(None);
    let req = notify_request("POST", None, Bytes::new());
    let resp = handle(client, req).await.unwrap();
    assert_eq!(resp.status(), hyper::StatusCode::OK);
}

#[tokio::test]
async fn notify_rejects_missing_token_when_auth_configured() {
    let (client, _rx) = test_client(Some("secret".to_string()));
    let req = notify_request("POST", None, Bytes::new());
    let resp = handle(client, req).await.unwrap();
    assert_eq!(resp.status(), hyper::StatusCode::UNAUTHORIZED);
    assert_eq!(
        resp.headers().get(hyper::header::WWW_AUTHENTICATE).unwrap(),
        "Bearer"
    );
}

#[tokio::test]
async fn notify_rejects_wrong_token_when_auth_configured() {
    let (client, _rx) = test_client(Some("secret".to_string()));
    let req = notify_request("POST", Some("Bearer wrong"), Bytes::new());
    let resp = handle(client, req).await.unwrap();
    assert_eq!(resp.status(), hyper::StatusCode::UNAUTHORIZED);
    assert_eq!(
        resp.headers().get(hyper::header::WWW_AUTHENTICATE).unwrap(),
        "Bearer"
    );
}

#[tokio::test]
async fn notify_accepts_correct_token_and_updates_work() {
    let (client, rx) = test_client(Some("secret".to_string()));
    let template = BlockTemplate {
        work_id: 42.into(),
        ..Default::default()
    };
    let body = serde_json::to_vec(&template).unwrap();
    let req = notify_request("POST", Some("Bearer secret"), Bytes::from(body));
    let resp = handle(client.clone(), req).await.unwrap();
    assert_eq!(resp.status(), hyper::StatusCode::OK);
    assert_eq!(client.current_work_id.load(Ordering::SeqCst), 42);

    let work = rx.recv();
    assert!(
        matches!(work, Ok(Works::New(_))),
        "expected new work notification"
    );
}

#[tokio::test]
async fn notify_accepts_correct_token_with_leading_whitespace() {
    let (client, _rx) = test_client(Some("secret".to_string()));
    let req = notify_request("POST", Some("Bearer  secret"), Bytes::new());
    let resp = handle(client, req).await.unwrap();
    assert_eq!(resp.status(), hyper::StatusCode::OK);
}
