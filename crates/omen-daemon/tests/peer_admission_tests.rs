use tempfile::tempdir;

use omen_ipc::{
    ClientHello, DaemonHello, PlatformListener, PlatformStream, read_json_frame, write_json_frame,
};
use omen_test_fixtures::{INTEGRATION_TIMEOUT, run_with_test_timeout};

#[tokio::test(flavor = "multi_thread")]
async fn test_peer_admission_same_user_allowed() {
    run_with_test_timeout(
        "test_peer_admission_same_user_allowed",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_ENDPOINT");
            let _temp = tempdir().unwrap();

            #[cfg(windows)]
            let endpoint = format!(r"\\.\pipe\omen-test-peer-{}", uuid::Uuid::new_v4());
            #[cfg(not(windows))]
            let endpoint = _temp
                .path()
                .join("omen-peer.sock")
                .to_string_lossy()
                .to_string();

            ctx.phase("BIND_LISTENER");
            let mut listener = PlatformListener::bind(&endpoint)
                .await
                .expect("PlatformListener bind should succeed");
            ctx.set_daemon_status("listening");

            ctx.phase("CONNECT_CLIENT");
            let client_task = tokio::spawn({
                let ep = endpoint.clone();
                async move {
                    let mut client_stream = PlatformStream::connect(&ep)
                        .await
                        .expect("Client connect should succeed");

                    let hello = ClientHello::new("cli_test".into(), vec!["events".into()]);
                    write_json_frame(&mut client_stream, &hello).await.unwrap();
                    let daemon_hello: Option<DaemonHello> =
                        read_json_frame(&mut client_stream).await.unwrap();
                    daemon_hello.expect("Should receive DaemonHello")
                }
            });

            ctx.phase("ACCEPT_CONNECTION");
            let mut server_stream = listener
                .accept()
                .await
                .expect("Peer admission should accept same user");
            ctx.set_client_status("accepted");

            ctx.phase("READ_CLIENT_HELLO");
            let client_hello: Option<ClientHello> =
                read_json_frame(&mut server_stream).await.unwrap();
            let client_hello = client_hello.expect("Should receive ClientHello");
            assert_eq!(client_hello.client_instance_id, "cli_test");

            ctx.phase("SEND_DAEMON_HELLO");
            let daemon_hello = DaemonHello::new("daemon_test".into(), 1);
            write_json_frame(&mut server_stream, &daemon_hello)
                .await
                .unwrap();

            ctx.phase("VERIFY_CLIENT_RECEIPT");
            let received_daemon_hello = client_task.await.unwrap();
            assert_eq!(received_daemon_hello.daemon_instance_id, "daemon_test");
        },
    )
    .await;
}
