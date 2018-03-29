use core::block::Header;
use nervos_protocol;
use protobuf::Message as ProtobufMessage;
use protobuf::RepeatedField;

pub fn new_headers_payload(headers: &[Header]) -> Vec<u8> {
    let mut payload = nervos_protocol::Payload::new();
    let mut headers_proto = nervos_protocol::Headers::new();
    let headers = headers.iter().map(Into::into).collect();
    headers_proto.set_headers(RepeatedField::from_vec(headers));
    payload.set_headers(headers_proto);
    payload.write_to_bytes().unwrap()
}
