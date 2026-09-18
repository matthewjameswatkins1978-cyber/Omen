use omen_ipc::*;
use tokio::io::duplex;

#[tokio::test]
async fn test_handshake_version_negotiation_success() {
    let client_hello = ClientHello::new("cli-test-1".into(), vec!["events".into()]);
    assert_eq!(client_hello.protocol_version_family, PROTOCOL_FAMILY);
    assert_eq!(client_hello.supported_versions, vec![1]);

    let version = negotiate_protocol_version(
        &client_hello.protocol_version_family,
        &client_hello.supported_versions,
    )
    .expect("Negotiation should succeed");

    assert_eq!(version, 1);

    let daemon_hello = DaemonHello::new("dmn-test-1".into(), version);
    assert_eq!(daemon_hello.selected_protocol_version, 1);
    assert_eq!(daemon_hello.max_frame_size, MAX_FRAME_SIZE);
}

#[tokio::test]
async fn test_handshake_unsupported_family() {
    let res = negotiate_protocol_version("alien.protocol", &[1]);
    match res {
        Err(LocalIpcError::ProtocolVersionUnsupported(msg)) => {
            assert!(msg.contains("Unsupported protocol family"));
        }
        other => panic!("Expected ProtocolVersionUnsupported, got {other:?}"),
    }
}

#[tokio::test]
async fn test_handshake_unsupported_version() {
    let res = negotiate_protocol_version(PROTOCOL_FAMILY, &[99, 100]);
    match res {
        Err(LocalIpcError::ProtocolVersionUnsupported(msg)) => {
            assert!(msg.contains("No mutually supported protocol version"));
        }
        other => panic!("Expected ProtocolVersionUnsupported, got {other:?}"),
    }
}

#[tokio::test]
async fn test_codec_frame_roundtrip() {
    let (mut client, mut server) = duplex(1024);

    let request = IpcRequest::new(
        "req-001",
        Some("sess-001".into()),
        Some("ws-001".into()),
        RequestPayload::Ping {
            timestamp_ms: 123456789,
        },
    );

    // Client writes request
    write_json_frame(&mut client, &request)
        .await
        .expect("write_json_frame should succeed");

    // Server reads request
    let received: Option<IpcRequest> = read_json_frame(&mut server)
        .await
        .expect("read_json_frame should succeed");

    assert_eq!(received, Some(request.clone()));

    // Server writes response
    let response = IpcResponse::ok(
        "req-001",
        ResponsePayload::Pong {
            timestamp_ms: 123456790,
        },
    );

    write_json_frame(&mut server, &response)
        .await
        .expect("write_json_frame should succeed");

    // Client reads response
    let res_received: Option<IpcResponse> = read_json_frame(&mut client)
        .await
        .expect("read_json_frame should succeed");

    assert_eq!(res_received, Some(response));
}

#[tokio::test]
async fn test_codec_frame_too_large_rejection() {
    let (mut client, mut server) = duplex(1024);

    let huge_payload = vec![b'A'; MAX_FRAME_SIZE + 1];

    let write_res = write_frame(&mut client, &huge_payload).await;
    match write_res {
        Err(LocalIpcError::FrameTooLarge {
            declared_bytes,
            max_bytes,
        }) => {
            assert_eq!(declared_bytes, MAX_FRAME_SIZE + 1);
            assert_eq!(max_bytes, MAX_FRAME_SIZE);
        }
        other => panic!("Expected FrameTooLarge error, got {other:?}"),
    }

    // Now test server reading an oversized header directly
    let oversized_len: u32 = (MAX_FRAME_SIZE + 500) as u32;
    tokio::io::AsyncWriteExt::write_all(&mut client, &oversized_len.to_be_bytes())
        .await
        .unwrap();

    let read_res = read_frame(&mut server).await;
    match read_res {
        Err(LocalIpcError::FrameTooLarge {
            declared_bytes,
            max_bytes,
        }) => {
            assert_eq!(declared_bytes, (MAX_FRAME_SIZE + 500));
            assert_eq!(max_bytes, MAX_FRAME_SIZE);
        }
        other => panic!("Expected FrameTooLarge error on read, got {other:?}"),
    }
}

#[tokio::test]
async fn test_codec_malformed_json_rejection() {
    let (mut client, mut server) = duplex(1024);

    let malformed_json = b"{ not valid json ...";
    write_frame(&mut client, malformed_json).await.unwrap();

    let res: Result<Option<IpcRequest>, _> = read_json_frame(&mut server).await;
    match res {
        Err(LocalIpcError::MalformedRequest(msg)) => {
            assert!(msg.contains("Failed to parse JSON"));
        }
        other => panic!("Expected MalformedRequest, got {other:?}"),
    }
}

#[tokio::test]
async fn test_event_envelope_roundtrip() {
    let (mut sender, mut receiver) = duplex(1024);

    let event = IpcEvent::new(
        "ws-main",
        1,
        42,
        EventPayload::FactInvalidated {
            fact_id: "fact://test:suite".into(),
            resource_uri: "fact://test/status".into(),
            previous_validity: "CURRENT".into(),
            new_validity: "DIRTY".into(),
            cause: "fs:workspace mutation".into(),
        },
    );

    write_json_frame(&mut sender, &event).await.unwrap();
    let received: Option<IpcEvent> = read_json_frame(&mut receiver).await.unwrap();
    assert_eq!(received, Some(event));
}

#[tokio::test]
async fn test_daemon_message_untagged_deserialization() {
    let (mut sender, mut receiver) = omen_ipc::PlatformStream::duplex_pair(2048);

    // 1. Send IpcResponse as DaemonMessage
    let response = IpcResponse::ok(
        "req-1",
        ResponsePayload::Pong {
            timestamp_ms: 1234567,
        },
    );
    write_json_frame(&mut sender, &DaemonMessage::Response(response.clone()))
        .await
        .unwrap();

    let msg1: Option<DaemonMessage> = read_json_frame(&mut receiver).await.unwrap();
    match msg1 {
        Some(DaemonMessage::Response(r)) => assert_eq!(r, response),
        other => panic!("Expected DaemonMessage::Response, got {other:?}"),
    }

    // 2. Send IpcEvent as DaemonMessage
    let event = IpcEvent::new(
        "ws-1",
        2,
        100,
        EventPayload::ResyncRequired {
            reason: "gap".into(),
        },
    );
    write_json_frame(&mut sender, &DaemonMessage::Event(event.clone()))
        .await
        .unwrap();

    let msg2: Option<DaemonMessage> = read_json_frame(&mut receiver).await.unwrap();
    match msg2 {
        Some(DaemonMessage::Event(e)) => assert_eq!(e, event),
        other => panic!("Expected DaemonMessage::Event, got {other:?}"),
    }
}
