use url::Url;

pub(crate) fn check_proxy_url(proxy_url: &str) -> Result<(), String> {
    let parsed_url = Url::parse(proxy_url).map_err(|e| e.to_string())?;
    if parsed_url.host_str().is_none() {
        return Err(format!("missing host in proxy url: {}", proxy_url));
    }
    let scheme = parsed_url.scheme();
    if scheme.ne("socks5") {
        return Err(format!("CKB doesn't support proxy scheme: {}", scheme));
    }
    if parsed_url.port().is_none() {
        return Err(format!("missing port in proxy url: {}", proxy_url));
    }
    Ok(())
}

#[test]
fn parse_socks5_url() {
    let result = Url::parse("socks5://username:password@localhost:1080");
    assert!(result.is_ok());
    let parsed_url = result.unwrap();
    assert_eq!(parsed_url.scheme(), "socks5");
    // username
    assert_eq!(parsed_url.username(), "username");
    // password
    assert_eq!(parsed_url.password(), Some("password"));
    // host
    assert_eq!(parsed_url.host_str(), Some("localhost"));
    // port
    assert_eq!(parsed_url.port(), Some(1080));
}
