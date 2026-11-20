use super::protocol::*;

#[test]
fn base64_round_trips_go_encoding() {
    let cases: [&[u8]; 6] = [b"", b"f", b"fo", b"foo", b"foob", b"fooba"];
    for input in cases {
        let encoded = base64_encode(input);
        assert_eq!(base64_decode(&encoded).expect("decode"), input);
    }
    assert_eq!(base64_encode(b"foo"), "Zm9v");
    assert_eq!(base64_encode(b"fo"), "Zm8=");
    assert_eq!(base64_encode(b"f"), "Zg==");
    assert_eq!(base64_decode("Zm9v").expect("decode"), b"foo");
    assert!(base64_decode("Zm9").is_err());
    assert!(base64_decode("Zm=9").is_err());
    // The NUL-delimited lsof fixture from the Go tests must survive.
    let lsof = b"p4242\x00cnode\x00u501\x00Ldev\x00n127.0.0.1:443\x00";
    assert_eq!(base64_decode(&base64_encode(lsof)).expect("decode"), lsof);
}

#[test]
fn status_response_bytes_match_go() {
    let response = StatusResponse {
        protocol_version: 1,
        owner_uid: 501,
    };
    assert_eq!(
        String::from_utf8(encode_server_json(&response).expect("encode")).expect("utf8"),
        r#"{"protocolVersion":1,"ownerUid":501}"#
    );
}

#[test]
fn request_json_uses_base64_and_omits_empty_fields() {
    let request = ProxySocketInstallRequest {
        plist: b"<plist/>".to_vec(),
        job_path: "/Library/LaunchDaemons/xyz.jyeu.devbase.dev.1234.proxy-sockets.plist"
            .to_string(),
        target: "system/xyz.jyeu.devbase.dev.1234.proxy-sockets".to_string(),
    };
    let encoded = String::from_utf8(encode_request_json(&request).expect("encode")).expect("utf8");
    assert_eq!(
        encoded,
        r#"{"plist":"PHBsaXN0Lz4=","jobPath":"/Library/LaunchDaemons/xyz.jyeu.devbase.dev.1234.proxy-sockets.plist","target":"system/xyz.jyeu.devbase.dev.1234.proxy-sockets"}"#
    );
    let decoded: ProxySocketInstallRequest =
        serde_json::from_str(&encoded).expect("decode our own request");
    assert_eq!(decoded, request);

    let save = LaunchdSaveRequest {
        plist: Vec::new(),
        path: "/Library/LaunchDaemons/com.example.plist".to_string(),
        domain: "system".to_string(),
        target: "system/com.example".to_string(),
        existing_path: String::new(),
        load: false,
    };
    assert_eq!(
        String::from_utf8(encode_request_json(&save).expect("encode")).expect("utf8"),
        r#"{"plist":"","path":"/Library/LaunchDaemons/com.example.plist","domain":"system","target":"system/com.example","load":false}"#
    );

    let with_existing = LaunchdSaveRequest {
        existing_path: "/Library/LaunchDaemons/com.example.plist".to_string(),
        load: true,
        ..save
    };
    let encoded =
        String::from_utf8(encode_request_json(&with_existing).expect("encode")).expect("utf8");
    assert!(encoded.contains(r#""existingPath":"/Library/LaunchDaemons/com.example.plist""#));
    assert!(encoded.contains(r#""load":true"#));
}

#[test]
fn port_inspect_response_omits_empty_output() {
    let empty = PortInspectResponse { output: Vec::new() };
    assert_eq!(
        String::from_utf8(encode_server_json(&empty).expect("encode")).expect("utf8"),
        "{}"
    );
    let output = PortInspectResponse {
        output: b"p1\x00".to_vec(),
    };
    assert_eq!(
        String::from_utf8(encode_server_json(&output).expect("encode")).expect("utf8"),
        r#"{"output":"cDEA"}"#
    );
}

#[test]
fn launchd_save_response_omits_empty_error() {
    assert_eq!(
        String::from_utf8(
            encode_server_json(&LaunchdSaveResponse {
                saved: true,
                error: String::new(),
            })
            .expect("encode")
        )
        .expect("utf8"),
        r#"{"saved":true}"#
    );
    assert_eq!(
        String::from_utf8(
            encode_server_json(&LaunchdSaveResponse {
                saved: true,
                error: "Bootstrap failed: 5".to_string(),
            })
            .expect("encode")
        )
        .expect("utf8"),
        r#"{"saved":true,"error":"Bootstrap failed: 5"}"#
    );
}

#[test]
fn server_json_escapes_html_like_go_marshal() {
    let response = ErrorResponse::new("a<b>c&d\u{2028}e\u{2029}f");
    assert_eq!(
        String::from_utf8(encode_server_json(&response).expect("encode")).expect("utf8"),
        r#"{"error":"a\u003cb\u003ec\u0026d\u2028e\u2029f"}"#
    );
}

#[test]
fn decoding_tolerates_missing_and_unknown_fields() {
    let request: PortInspectRequest =
        serde_json::from_str(r#"{"port":443,"extra":true}"#).expect("decode");
    assert_eq!(request.port, 443);
    let empty: LaunchdControlRequest = serde_json::from_str("{}").expect("decode");
    assert!(empty.commands.is_empty());
    let status: StatusResponse = serde_json::from_str("{}").expect("decode");
    assert_eq!(status.protocol_version, 0);
}
