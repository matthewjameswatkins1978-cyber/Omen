use omen_adapters::RustAnalyzerProvider;
use omen_mcp::protocol::*;
use omen_mcp::{McpServer, validate_workspace};
use omen_semantic::SemanticProviderRegistry;
use omen_test_fixtures::{INTEGRATION_TIMEOUT, UNIT_TIMEOUT, run_with_test_timeout};
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::tempdir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[test]
fn test_workspace_validation_rejects_missing_and_files() {
    let temp = tempdir().unwrap();
    let valid = validate_workspace(temp.path()).unwrap();
    assert!(valid.is_absolute());

    let file = temp.path().join("not-a-workspace");
    fs::write(&file, "file").unwrap();
    let file_error = validate_workspace(&file).unwrap_err();
    assert!(file_error.starts_with("WORKSPACE_NOT_FOUND"));

    let missing = temp.path().join("missing");
    let missing_error = validate_workspace(&missing).unwrap_err();
    assert!(missing_error.starts_with("WORKSPACE_NOT_FOUND"));
}

#[tokio::test]
async fn test_history_reports_unjournalled_local_execution_truthfully() {
    let temp = tempdir().unwrap();
    fs::create_dir_all(
        omen_knowledge::local_execution_status_path(temp.path())
            .parent()
            .unwrap(),
    )
    .unwrap();
    fs::write(
        omen_knowledge::local_execution_status_path(temp.path()),
        r#"{"history_status":"UNJOURNALED_LOCAL_EXECUTION","command":["cargo","check"]}"#,
    )
    .unwrap();
    let server = McpServer::new(temp.path().to_path_buf(), None);
    let result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(1)),
            method: "tools/call".into(),
            params: Some(json!({"name":"omen_history_query","arguments":{}})),
        })
        .await;
    let call: CallToolResult = serde_json::from_value(result.result.unwrap()).unwrap();
    let value: Value = serde_json::from_str(&call.content[0].text).unwrap();
    assert_eq!(value["history_status"], "UNJOURNALED_LOCAL_EXECUTION");
    assert_eq!(value["history"]["entries"].as_array().unwrap().len(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_mcp_initialize_and_protocol_version() {
    run_with_test_timeout(
        "test_mcp_initialize_and_protocol_version",
        UNIT_TIMEOUT,
        |_ctx| async move {
            let temp = tempdir().unwrap();
            let server = McpServer::new(temp.path().to_path_buf(), None);

            // 1. Valid initialization
            let req = JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: Some(json!(1)),
                method: "initialize".into(),
                params: Some(json!({
                    "protocolVersion": "2024-11-05",
                    "clientInfo": { "name": "test-agent", "version": "1.0.0" }
                })),
            };

            let resp = server.handle_request(req).await;
            assert!(resp.error.is_none(), "Valid initialize must succeed");
            let result: InitializeResult = serde_json::from_value(resp.result.unwrap()).unwrap();
            assert_eq!(result.protocol_version, "2024-11-05");
            assert_eq!(result.server_info.name, "omen-mcp");

            // 2. Malformed protocol version is distinct from negotiation.
            let bad_req = JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: Some(json!(2)),
                method: "initialize".into(),
                params: Some(json!({
                    "protocolVersion": "not-a-version"
                })),
            };
            let bad_resp = server.handle_request(bad_req).await;
            assert!(
                bad_resp.error.is_some(),
                "Incompatible version must be rejected"
            );
            let err = bad_resp.error.unwrap();
            assert_eq!(err.code, -32602);
            assert!(err.message.contains("Invalid MCP protocol version"));

            // 3. A valid but unsupported revision is counter-offered, not
            // classified as malformed input.
            let future_req = JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: Some(json!(3)),
                method: "initialize".into(),
                params: Some(json!({"protocolVersion": "2027-01-01"})),
            };
            let future_resp = server.handle_request(future_req).await;
            assert!(future_resp.error.is_none());
            let future_result: InitializeResult =
                serde_json::from_value(future_resp.result.unwrap()).unwrap();
            assert_eq!(future_result.protocol_version, LATEST_MCP_PROTOCOL_VERSION);
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_mcp_tools_list() {
    run_with_test_timeout("test_mcp_tools_list", UNIT_TIMEOUT, |_ctx| async move {
        let temp = tempdir().unwrap();
        let server = McpServer::new(temp.path().to_path_buf(), None);

        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(1)),
            method: "tools/list".into(),
            params: None,
        };

        let resp = server.handle_request(req).await;
        assert!(resp.error.is_none());
        let val = resp.result.unwrap();
        let tools = val["tools"].as_array().unwrap();

        let tool_names: Vec<String> = tools
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        assert!(tool_names.contains(&"omen_workspace_status".to_string()));
        assert!(tool_names.contains(&"omen_facts_query".to_string()));
        assert!(tool_names.contains(&"omen_execute".to_string()));
        assert!(tool_names.contains(&"omen_execution_status".to_string()));
        assert!(tool_names.contains(&"omen_history_query".to_string()));
        assert!(tool_names.contains(&"omen_services_list".to_string()));
        assert!(tool_names.contains(&"omen_services_control".to_string()));
        assert!(tool_names.contains(&"omen_capabilities_discover".to_string()));
        for name in [
            "omen_orient",
            "omen_capabilities",
            "omen_describe",
            "omen_recipe",
            "omen_context",
            "omen_action_list",
            "omen_action_show",
            "omen_action_plan",
        ] {
            assert!(tool_names.contains(&name.to_string()), "missing {name}");
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_mcp_canonical_discovery_and_action_projection() {
    run_with_test_timeout(
        "test_mcp_canonical_discovery_and_action_projection",
        UNIT_TIMEOUT,
        |_ctx| async move {
            let temp = tempdir().unwrap();
            fs::write(
                temp.path().join("Omen.toml"),
                r#"schema_version = 1

[actions.inspect]
description = "Inspect a symbol"
[[actions.inspect.steps]]
id = "definition"
capability = "semantic.definition"
input = { symbol = { kind = "literal", value = "SessionToken" } }
"#,
            )
            .unwrap();
            let server = McpServer::new(temp.path().to_path_buf(), None);

            async fn call(server: &McpServer, name: &str, arguments: Value) -> Value {
                let response = server
                    .handle_request(JsonRpcRequest {
                        jsonrpc: "2.0".into(),
                        id: Some(json!(name)),
                        method: "tools/call".into(),
                        params: Some(json!({"name": name, "arguments": arguments})),
                    })
                    .await;
                assert!(response.error.is_none(), "{name} returned RPC error");
                let result: CallToolResult =
                    serde_json::from_value(response.result.unwrap()).unwrap();
                assert_ne!(result.is_error, Some(true), "{name} returned tool error");
                serde_json::from_str(&result.content[0].text).unwrap()
            }

            let orient = call(&server, "omen_orient", json!({})).await;
            assert_eq!(orient["contract_version"], "0.8");
            assert!(
                orient["contract_digest"]
                    .as_str()
                    .unwrap()
                    .starts_with("sha256:")
            );
            assert!(
                orient["next_actions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|action| { action["cli"].is_string() && action["mcp_tool"].is_string() })
            );

            let capabilities =
                call(&server, "omen_capabilities", json!({"group":"mutation"})).await;
            assert_eq!(capabilities["capabilities"].as_array().unwrap().len(), 1);
            assert_eq!(capabilities["capabilities"][0]["id"], "mutation.threadmoth");
            assert!(capabilities["capabilities"][0]["describe_ref"].is_string());
            assert!(capabilities["capabilities"][0]["summary"].is_string());
            assert!(capabilities["capabilities"][0]["input_schema"].is_null());

            let described = call(
                &server,
                "omen_describe",
                json!({"capability_id":"mutation.threadmoth"}),
            )
            .await;
            assert_eq!(described["definition"]["authority"], "Tethers admission");
            assert_eq!(described["describe_ref"], "mutation.threadmoth");
            assert!(described["status"]["availability"].is_string());

            let recipe = call(&server, "omen_recipe", json!({"recipe_id":"safe-mutation"})).await;
            assert_eq!(recipe["id"], "safe-mutation");
            assert!(
                recipe["steps"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|step| { step["capability_id"] == "mutation.threadmoth" })
            );

            let context = call(&server, "omen_context", json!({})).await;
            assert_eq!(context["delta"], "CURRENT_SNAPSHOT");

            let actions = call(&server, "omen_action_list", json!({})).await;
            assert_eq!(actions["config_present"], true);
            assert_eq!(actions["actions"][0]["action_id"], "inspect");

            let plan = call(&server, "omen_action_plan", json!({"action_id":"inspect"})).await;
            assert_eq!(plan["planning_valid"], true);
            assert_eq!(plan["admission_snapshot_only"], true);
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_mcp_capabilities_discover() {
    run_with_test_timeout(
        "test_mcp_capabilities_discover",
        UNIT_TIMEOUT,
        |_ctx| async move {
            let temp = tempdir().unwrap();
            let server = McpServer::new(temp.path().to_path_buf(), None);

            let req = JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: Some(json!(10)),
                method: "tools/call".into(),
                params: Some(json!({
                    "name": "omen_capabilities_discover",
                    "arguments": {}
                })),
            };

            let resp = server.handle_request(req).await;
            assert!(resp.error.is_none());
            let result: CallToolResult = serde_json::from_value(resp.result.unwrap()).unwrap();
            assert_ne!(result.is_error, Some(true));

            let content_text = &result.content[0].text;
            let val: Value = serde_json::from_str(content_text).unwrap();
            assert_eq!(val["doctrine"], "substrate, not sovereign");
            assert!(val["boundaries"]["lantern"].is_string());
            assert!(val["boundaries"]["resolve"].is_string());
            assert!(val["boundaries"]["tethers"].is_string());
            assert!(val["boundaries"]["omen"].is_string());
            assert!(val["boundaries"]["threadmoth"].is_string());
            assert!(val["tools"].as_array().unwrap().len() >= 8);
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_mcp_workspace_status() {
    run_with_test_timeout(
        "test_mcp_workspace_status",
        UNIT_TIMEOUT,
        |_ctx| async move {
            let temp = tempdir().unwrap();
            let ws_path = temp.path().to_path_buf();
            fs::write(ws_path.join("Cargo.toml"), "[package]\nname = \"mcp_ws\"\n").unwrap();

            let server = McpServer::new(ws_path.clone(), None);

            let req = JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: Some(json!(20)),
                method: "tools/call".into(),
                params: Some(json!({
                    "name": "omen_workspace_status",
                    "arguments": {}
                })),
            };

            let resp = server.handle_request(req).await;
            assert!(resp.error.is_none());
            let result: CallToolResult = serde_json::from_value(resp.result.unwrap()).unwrap();
            assert_ne!(result.is_error, Some(true));

            let text = &result.content[0].text;
            let status: Value = serde_json::from_str(text).unwrap();
            assert!(status["workspace_root"].is_string());
            assert_eq!(status["mode"], "standalone");
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_mcp_execute_and_read_cas_artifact_proof_d() {
    run_with_test_timeout(
        "test_mcp_execute_and_read_cas_artifact_proof_d",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_WORKSPACE");
            let temp = tempdir().unwrap();
            let ws_path = temp.path().to_path_buf();

            let daemon = std::sync::Arc::new(omen_daemon::DaemonServer::new(Some(
                "duplex://proof_d_daemon".into(),
            )));
            let (client_stream, daemon_stream) = omen_ipc::PlatformStream::duplex_pair(65536);
            let instance_id = daemon.instance_id().to_string();
            let registry = daemon.registry();
            let shutdown_rx = daemon.subscribe_shutdown();

            tokio::spawn(async move {
                let _ = omen_daemon::DaemonServer::handle_connection(
                    daemon_stream,
                    instance_id,
                    registry,
                    shutdown_rx,
                )
                .await;
            });

            let client = omen_client::OmenClient::from_stream(
                client_stream,
                Some("memory://proof_d".into()),
                Some("sess-proof-d".into()),
            )
            .await
            .unwrap();

            client
                .attach_workspace(ws_path.to_str().unwrap())
                .await
                .unwrap();

            let server = McpServer::new(ws_path.clone(), Some(client));

            ctx.phase("EXECUTE_COMMAND");
            #[cfg(windows)]
            let argv = vec![
                "cmd".to_string(),
                "/C".to_string(),
                "echo proof_d_token_hello".to_string(),
            ];
            #[cfg(not(windows))]
            let argv = vec!["echo".to_string(), "proof_d_token_hello".to_string()];

            let exec_req = JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: Some(json!(30)),
                method: "tools/call".into(),
                params: Some(json!({
                    "name": "omen_execute",
                    "arguments": {
                        "argv": argv,
                        "cwd": ws_path.to_string_lossy().to_string()
                    }
                })),
            };

            let exec_resp = server.handle_request(exec_req).await;
            assert!(exec_resp.error.is_none(), "omen_execute call must succeed");

            let exec_call_res: CallToolResult =
                serde_json::from_value(exec_resp.result.unwrap()).unwrap();
            assert_ne!(exec_call_res.is_error, Some(true));

            let exec_val: Value = serde_json::from_str(&exec_call_res.content[0].text).unwrap();
            assert_eq!(exec_val["exit_code"], 0);

            let stdout_art = exec_val["stdout_artifact"].as_str();
            assert!(
                stdout_art.is_some(),
                "Execution must produce a stdout CAS artifact"
            );
            let art_uri = stdout_art.unwrap();
            assert!(
                art_uri.starts_with("artifact://sha256/"),
                "Artifact URI must be artifact://sha256/..."
            );

            ctx.phase("READ_CAS_ARTIFACT_RESOURCE");
            let read_req = JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: Some(json!(31)),
                method: "resources/read".into(),
                params: Some(json!({
                    "uri": art_uri,
                    "offset": 0,
                    "length": 1024
                })),
            };

            let read_resp = server.handle_request(read_req).await;
            assert!(read_resp.error.is_none(), "resources/read must succeed");

            let read_val = read_resp.result.unwrap();
            let contents = read_val["contents"].as_array().unwrap();
            assert_eq!(contents.len(), 1);
            assert_eq!(contents[0]["uri"], art_uri);

            let content_text = contents[0]["text"].as_str().unwrap();
            assert!(
                content_text.contains("proof_d_token_hello"),
                "Artifact resource read must contain the executed command output"
            );
        },
    )
    .await;
}

#[tokio::test]
async fn test_mcp_execute_preserves_timeout_and_nullable_exit_code() {
    let temp = tempdir().unwrap();
    let server = McpServer::new(temp.path().to_path_buf(), None);
    let request = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: Some(json!(91)),
        method: "tools/call".into(),
        params: Some(json!({
            "name": "omen_execute",
            "arguments": {
                "argv": [semantic_gremlin_exe(), "--sleep-ms", "1000"],
                "cwd": temp.path().to_string_lossy(),
                "timeout_ms": 20
            }
        })),
    };

    let response = server.handle_request(request).await;
    assert!(response.error.is_none());
    let result: CallToolResult = serde_json::from_value(response.result.unwrap()).unwrap();
    assert_ne!(result.is_error, Some(true));
    let value: Value = serde_json::from_str(&result.content[0].text).unwrap();
    assert_eq!(value["runtime_status"], "TIMED_OUT");
    assert!(value["exit_code"].is_null());
}

#[tokio::test]
async fn test_mcp_execution_result_preserves_nonzero_and_spawn_failure_identity() {
    let temp = tempdir().unwrap();
    let server = McpServer::new(temp.path().to_path_buf(), None);
    let cwd = temp.path().to_string_lossy().to_string();
    let server_ref = &server;

    let call = |argv: Value| {
        let cwd = cwd.clone();
        async move {
            let response = server_ref
                .handle_request(JsonRpcRequest {
                    jsonrpc: "2.0".into(),
                    id: Some(json!(92)),
                    method: "tools/call".into(),
                    params: Some(json!({
                        "name": "omen_execute",
                        "arguments": {"argv": argv, "cwd": cwd}
                    })),
                })
                .await;
            serde_json::from_value::<CallToolResult>(response.result.unwrap()).unwrap()
        }
    };

    let nonzero_result = call(json!([semantic_gremlin_exe(), "--exit", "7"])).await;
    let nonzero: Value = serde_json::from_str(&nonzero_result.content[0].text).unwrap();
    assert_eq!(nonzero["runtime_status"], "COMPLETED");
    assert_eq!(nonzero["exit_code"], 7);

    let failed = call(json!(["definitely-not-an-omen-executable"])).await;
    assert_eq!(failed.is_error, Some(true));
    assert!(failed.content[0].text.contains("Local execution error"));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_mcp_persistent_notification_does_not_emit_response() {
    run_with_test_timeout(
        "test_mcp_persistent_notification_does_not_emit_response",
        UNIT_TIMEOUT,
        |_ctx| async move {
            let temp = tempdir().unwrap();
            let server = McpServer::new(temp.path().to_path_buf(), None);
            let (client_read, server_write) = tokio::io::duplex(65536);
            let (server_read, mut client_write) = tokio::io::duplex(65536);

            tokio::spawn(async move {
                let _ = server.run_stream(server_read, server_write).await;
            });
            let mut client_lines = BufReader::new(client_read).lines();

            let initialize = serde_json::to_string(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2024-11-05",
                    "clientInfo": {"name": "persistent-test", "version": "1"}
                }
            }))
            .unwrap();
            client_write.write_all(initialize.as_bytes()).await.unwrap();
            client_write.write_all(b"\n").await.unwrap();
            client_write.flush().await.unwrap();
            let initialize_response = client_lines.next_line().await.unwrap().unwrap();
            let initialize_value: Value = serde_json::from_str(&initialize_response).unwrap();
            assert_eq!(initialize_value["id"], 1);

            client_write
                .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\",\"params\":{}}\n")
                .await
                .unwrap();
            client_write.flush().await.unwrap();
            assert!(
                tokio::time::timeout(
                    std::time::Duration::from_millis(100),
                    client_lines.next_line()
                )
                .await
                .is_err(),
                "notifications must not produce a response"
            );

            client_write
                .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{}}\n")
                .await
                .unwrap();
            client_write.flush().await.unwrap();
            let tools_response = client_lines.next_line().await.unwrap().unwrap();
            let tools_value: Value = serde_json::from_str(&tools_response).unwrap();
            assert_eq!(tools_value["id"], 2);
            let names: Vec<&str> = tools_value["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|tool| tool["name"].as_str())
                .collect();
            for name in [
                "omen_orient",
                "omen_symbol_search",
                "omen_symbol_definition",
                "omen_symbol_references",
            ] {
                assert!(names.contains(&name), "missing {name}");
            }
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_mcp_persistent_handshake_supported_versions() {
    run_with_test_timeout(
        "test_mcp_persistent_handshake_supported_versions",
        UNIT_TIMEOUT,
        |_ctx| async move {
            for version in SUPPORTED_MCP_HANDSHAKE_VERSIONS {
                let temp = tempdir().unwrap();
                let server = McpServer::new(temp.path().to_path_buf(), None);
                let (client_read, server_write) = tokio::io::duplex(65536);
                let (server_read, mut client_write) = tokio::io::duplex(65536);

                tokio::spawn(async move {
                    let _ = server.run_stream(server_read, server_write).await;
                });
                let mut client_lines = BufReader::new(client_read).lines();

                client_write
                    .write_all(
                        serde_json::to_string(&json!({
                            "jsonrpc": "2.0",
                            "id": 1,
                            "method": "initialize",
                            "params": {"protocolVersion": version}
                        }))
                        .unwrap()
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
                client_write.write_all(b"\n").await.unwrap();
                client_write.flush().await.unwrap();

                let initialize = client_lines.next_line().await.unwrap().unwrap();
                let initialize_value: Value = serde_json::from_str(&initialize).unwrap();
                assert_eq!(initialize_value["id"], 1);
                assert_eq!(initialize_value["result"]["protocolVersion"], *version);

                client_write
                    .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
                    .await
                    .unwrap();
                client_write.flush().await.unwrap();
                assert!(
                    tokio::time::timeout(
                        std::time::Duration::from_millis(100),
                        client_lines.next_line()
                    )
                    .await
                    .is_err(),
                    "notifications must not produce a response for {version}"
                );

                client_write
                    .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n")
                    .await
                    .unwrap();
                client_write.flush().await.unwrap();
                let tools = client_lines.next_line().await.unwrap().unwrap();
                let tools_value: Value = serde_json::from_str(&tools).unwrap();
                assert_eq!(tools_value["id"], 2);
                for name in [
                    "omen_orient",
                    "omen_symbol_search",
                    "omen_symbol_definition",
                    "omen_symbol_references",
                ] {
                    assert!(
                        tools_value["result"]["tools"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|tool| tool["name"] == name),
                        "missing {name} for {version}"
                    );
                }
            }
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_mcp_malformed_json_rpc_handling() {
    run_with_test_timeout(
        "test_mcp_malformed_json_rpc_handling",
        UNIT_TIMEOUT,
        |_ctx| async move {
            let temp = tempdir().unwrap();
            let server = McpServer::new(temp.path().to_path_buf(), None);

            // 1. Invalid JSON string
            let resp_str = server.dispatch_message("{malformed json").await;
            assert!(resp_str.is_some());
            let val: Value = serde_json::from_str(&resp_str.unwrap()).unwrap();
            assert_eq!(val["error"]["code"], -32700);

            // 2. Unknown method
            let unknown_req = JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: Some(json!(99)),
                method: "nonexistent/action".into(),
                params: None,
            };
            let unknown_resp = server.handle_request(unknown_req).await;
            assert!(unknown_resp.error.is_some());
            assert_eq!(unknown_resp.error.unwrap().code, -32601);

            // 3. Unknown tool
            let bad_tool = JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: Some(json!(100)),
                method: "tools/call".into(),
                params: Some(json!({
                    "name": "ghost_tool",
                    "arguments": {}
                })),
            };
            let bad_tool_resp = server.handle_request(bad_tool).await;
            let call_res: CallToolResult =
                serde_json::from_value(bad_tool_resp.result.unwrap()).unwrap();
            assert_eq!(call_res.is_error, Some(true));
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_mcp_duplex_stream_transport() {
    run_with_test_timeout(
        "test_mcp_duplex_stream_transport",
        UNIT_TIMEOUT,
        |_ctx| async move {
            let temp = tempdir().unwrap();
            let server = McpServer::new(temp.path().to_path_buf(), None);

            let (client_read, server_write) = tokio::io::duplex(65536);
            let (server_read, mut client_write) = tokio::io::duplex(65536);

            tokio::spawn(async move {
                let _ = server.run_stream(server_read, server_write).await;
            });

            let mut client_lines = BufReader::new(client_read).lines();

            // Send ping
            let ping_json = serde_json::to_string(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "ping"
            }))
            .unwrap();

            client_write.write_all(ping_json.as_bytes()).await.unwrap();
            client_write.write_all(b"\n").await.unwrap();
            client_write.flush().await.unwrap();

            let line = client_lines.next_line().await.unwrap().unwrap();
            let resp_val: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(resp_val["id"], 1);
            assert!(resp_val["result"].is_object());
        },
    )
    .await;
}
fn semantic_gremlin_exe() -> PathBuf {
    let mut path = std::env::current_exe().expect("failed to get current_exe");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    let name = if cfg!(windows) {
        "omen-gremlin.exe"
    } else {
        "omen-gremlin"
    };
    let exe = path.join(name);
    if exe.exists() {
        return exe;
    }

    let fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("target")
        .join("debug")
        .join(name);
    if fallback.exists() {
        return fallback;
    }

    let status = std::process::Command::new("cargo")
        .args(["build", "-p", "omen-test-fixtures", "--bin", "omen-gremlin"])
        .status()
        .expect("failed to invoke cargo for omen-gremlin");
    assert!(
        status.success(),
        "failed to build omen-gremlin test fixture"
    );
    assert!(fallback.exists(), "omen-gremlin fixture was not produced");
    fallback
}

async fn call_semantic_mcp_tool(server: &McpServer, name: &str, arguments: Value) -> Value {
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(name)),
            method: "tools/call".into(),
            params: Some(json!({"name": name, "arguments": arguments})),
        })
        .await;
    assert!(response.error.is_none(), "{name} returned RPC error");
    let result: CallToolResult = serde_json::from_value(response.result.unwrap()).unwrap();
    assert_ne!(result.is_error, Some(true), "{name} returned tool error");
    serde_json::from_str(&result.content[0].text).unwrap()
}

async fn call_semantic_mcp_tool_result(
    server: &McpServer,
    name: &str,
    arguments: Value,
) -> CallToolResult {
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(name)),
            method: "tools/call".into(),
            params: Some(json!({"name": name, "arguments": arguments})),
        })
        .await;
    assert!(response.error.is_none(), "{name} returned RPC error");
    serde_json::from_value(response.result.unwrap()).unwrap()
}
#[tokio::test(flavor = "multi_thread")]
async fn test_mcp_public_semantic_surface_finds_rust_function_through_adapter() {
    run_with_test_timeout(
        "test_mcp_public_semantic_surface_finds_rust_function_through_adapter",
        INTEGRATION_TIMEOUT,
        |_ctx| async move {
            let temp = tempdir().unwrap();
            fs::create_dir_all(temp.path().join("src")).unwrap();
            fs::write(
                temp.path().join("Cargo.toml"),
                "[package]\nname = \"omen-mcp-semantic-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
            )
            .unwrap();
            fs::write(
                temp.path().join("src/lib.rs"),
                "pub fn refresh_token() -> &'static str { \"token\" }\n\npub fn caller() -> &'static str { refresh_token() }\n",
            )
            .unwrap();

            let mut registry = SemanticProviderRegistry::new(temp.path().to_path_buf());
            registry.register(Arc::new(RustAnalyzerProvider::with_binary_args(
                temp.path().to_path_buf(),
                semantic_gremlin_exe(),
                vec!["--lsp-mode".into(), "semantic-filtered".into()],
            )));
            let server = McpServer::with_semantic_registry(
                temp.path().to_path_buf(),
                None,
                Arc::new(registry),
            );

            let search = call_semantic_mcp_tool(
                &server,
                "omen_symbol_search",
                json!({"query":"refresh_token"}),
            )
            .await;
            assert_eq!(search["operation"], "symbol_search");
            assert_eq!(search["outcome"], "FOUND");
            let search = search["data"]["matches"]
                .as_array()
                .expect("symbol search must return typed matches");
            assert!(
                search.iter().any(|symbol| {
                    symbol["name"] == "refresh_token" && symbol["kind"] == "function"
                }),
                "public MCP search must expose refresh_token as a function"
            );

            let definition = call_semantic_mcp_tool(
                &server,
                "omen_symbol_definition",
                json!({"symbol":"refresh_token"}),
            )
            .await;
            assert_eq!(definition["operation"], "definition");
            assert_eq!(definition["outcome"], "FOUND");
            assert_eq!(definition["data"]["resolved"]["file"], "src/lib.rs");
            assert_eq!(definition["data"]["resolved"]["provider"], "rust-analyzer");

            let equivalent_hints = vec![
                "src/lib.rs".to_string(),
                "src\\lib.rs".to_string(),
                "./src/lib.rs".to_string(),
            ];
            #[cfg(windows)]
            let equivalent_hints = {
                let mut equivalent_hints = equivalent_hints;
                let absolute = temp.path().join("src/lib.rs");
                let absolute = absolute.to_string_lossy().replace('\\', "/");
                equivalent_hints.push(absolute.clone());
                equivalent_hints.push(format!("\\\\?\\{}", absolute));
                equivalent_hints.push(format!("file:///{}", absolute));
                equivalent_hints
            };
            for file in equivalent_hints {
                let resolved = call_semantic_mcp_tool(
                    &server,
                    "omen_symbol_definition",
                    json!({"symbol":"refresh_token","file":file,"line":0,"col":7}),
                )
                .await;
                assert_eq!(resolved["data"]["resolved"]["file"], "src/lib.rs", "hint={file}");
            }

            let references = call_semantic_mcp_tool(
                &server,
                "omen_symbol_references",
                json!({"symbol":"refresh_token"}),
            )
            .await;
            let references = references["data"]["references"]
                .as_array()
                .expect("public references must resolve");
            assert!(
                references.iter().any(|reference| {
                    reference["location"]["range"]["start_line"] == 2
                }),
                "public references must include the known fixture call site"
            );

            let false_definition = call_semantic_mcp_tool(
                &server,
                "omen_symbol_definition",
                json!({"symbol":"nonexistent_fake_symbol","file":"src/lib.rs","line":0,"col":7}),
            )
            .await;
            assert_eq!(false_definition["outcome"], "NOT_FOUND");
            assert!(false_definition["data"]["resolved"].is_null());

            let false_references = call_semantic_mcp_tool_result(
                &server,
                "omen_symbol_references",
                json!({"symbol":"refresh_token","file":"src/lib.rs","line":4,"col":7}),
            )
            .await;
            assert_eq!(false_references.is_error, Some(true));
            assert_eq!(
                false_references.error.as_ref().unwrap().code,
                omen_core::ErrorCode::SemanticHintMismatch
            );
            assert!(false_references.content[0].text.contains("SEMANTIC_HINT_MISMATCH"));

            for file in [
                "src/lib.rs".to_string(),
                "src\\lib.rs".to_string(),
                "./src/lib.rs".to_string(),
            ] {
                let mismatch = call_semantic_mcp_tool_result(
                    &server,
                    "omen_symbol_references",
                    json!({"symbol":"refresh_token","file":file,"line":4,"col":7}),
                )
                .await;
                assert_eq!(mismatch.is_error, Some(true));
                assert_eq!(
                    mismatch.error.as_ref().unwrap().code,
                    omen_core::ErrorCode::SemanticHintMismatch
                );
                assert!(mismatch.content[0].text.contains("SEMANTIC_HINT_MISMATCH"));
            }

            let partial_hint = call_semantic_mcp_tool_result(
                &server,
                "omen_symbol_definition",
                json!({"symbol":"refresh_token","file":"src/lib.rs","line":0}),
            )
            .await;
            assert_eq!(partial_hint.is_error, Some(true));
            assert!(partial_hint.content[0].text.contains("SEMANTIC_HINT_INCOMPLETE"));
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_mcp_semantic_coverage_preserves_unsupported_targets() {
    run_with_test_timeout(
        "test_mcp_semantic_coverage_preserves_unsupported_targets",
        INTEGRATION_TIMEOUT,
        |_ctx| async move {
            let supported = tempdir().unwrap();
            fs::create_dir_all(supported.path().join("src")).unwrap();
            fs::write(
                supported.path().join("Cargo.toml"),
                "[package]\nname = \"omen-mcp-supported\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
            )
            .unwrap();
            fs::write(
                supported.path().join("src/lib.rs"),
                "pub fn supported_symbol() -> &'static str { \"ok\" }\n",
            )
            .unwrap();

            let mut supported_registry = SemanticProviderRegistry::new(supported.path().into());
            supported_registry.register(Arc::new(RustAnalyzerProvider::with_binary_args(
                supported.path().into(),
                semantic_gremlin_exe(),
                vec!["--lsp-mode".into(), "semantic-filtered".into()],
            )));
            let supported_server = McpServer::with_semantic_registry(
                supported.path().into(),
                None,
                Arc::new(supported_registry),
            );
            let absent = call_semantic_mcp_tool(
                &supported_server,
                "omen_symbol_search",
                json!({"query":"definitely_absent_symbol"}),
            )
            .await;
            assert_eq!(absent["outcome"], "NOT_FOUND");
            assert_eq!(absent["data"]["matches"], json!([]));

            let mixed = tempdir().unwrap();
            fs::create_dir_all(mixed.path().join("src")).unwrap();
            fs::write(
                mixed.path().join("Cargo.toml"),
                "[package]\nname = \"omen-mcp-mixed\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
            )
            .unwrap();
            fs::write(mixed.path().join("src/lib.rs"), "pub fn supported_symbol() {}\n").unwrap();
            fs::write(
                mixed.path().join("src/unsupported.xyz"),
                "DEFINE FUNCTION xyz_function() { return 42; }\n",
            )
            .unwrap();

            let mut mixed_registry = SemanticProviderRegistry::new(mixed.path().into());
            mixed_registry.register(Arc::new(RustAnalyzerProvider::with_binary_args(
                mixed.path().into(),
                semantic_gremlin_exe(),
                vec!["--lsp-mode".into(), "semantic-filtered".into()],
            )));
            let mixed_server = McpServer::with_semantic_registry(
                mixed.path().into(),
                None,
                Arc::new(mixed_registry),
            );

            let search = call_semantic_mcp_tool(
                &mixed_server,
                "omen_symbol_search",
                json!({"query":"xyz_function"}),
            )
            .await;
            assert_eq!(search["outcome"], "NOT_FOUND");
            assert_eq!(search["data"]["matches"], json!([]));
            assert_eq!(search["coverage"], "PARTIAL");

            for tool in ["omen_symbol_definition", "omen_symbol_references"] {
                let result = call_semantic_mcp_tool_result(
                    &mixed_server,
                    tool,
                    json!({"symbol":"xyz_function","file":"src/unsupported.xyz","line":0,"col":0}),
                )
                .await;
                assert_eq!(result.error.unwrap().code, omen_core::ErrorCode::Unsupported);
            }
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_mcp_provider_failure_does_not_become_not_found_and_recovers() {
    run_with_test_timeout(
        "test_mcp_provider_failure_does_not_become_not_found_and_recovers",
        INTEGRATION_TIMEOUT,
        |_ctx| async move {
            let temp = tempdir().unwrap();
            fs::create_dir_all(temp.path().join("src")).unwrap();
            fs::write(
                temp.path().join("Cargo.toml"),
                "[package]\nname = \"omen-mcp-provider-failure\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
            )
            .unwrap();
            fs::write(
                temp.path().join("src/lib.rs"),
                "pub fn refresh_token() -> &'static str { \"token\" }\n",
            )
            .unwrap();

            let mut failed_registry = SemanticProviderRegistry::new(temp.path().into());
            failed_registry.register(Arc::new(RustAnalyzerProvider::with_binary_args(
                temp.path().into(),
                semantic_gremlin_exe(),
                vec!["--lsp-mode".into(), "exit-before-ready".into()],
            )));
            let failed_server = McpServer::with_semantic_registry(
                temp.path().into(),
                None,
                Arc::new(failed_registry),
            );
            let failure = call_semantic_mcp_tool_result(
                &failed_server,
                "omen_symbol_definition",
                json!({"symbol":"refresh_token"}),
            )
            .await;
            assert_eq!(failure.is_error, Some(true));
            assert_eq!(
                failure.error.as_ref().unwrap().code,
                omen_core::ErrorCode::ProviderFailure
            );
            assert!(failure.content[0].text.contains("provider"));
            assert!(!failure.content[0].text.contains("not_found"));

            let mut recovered_registry = SemanticProviderRegistry::new(temp.path().into());
            recovered_registry.register(Arc::new(RustAnalyzerProvider::with_binary_args(
                temp.path().into(),
                semantic_gremlin_exe(),
                vec!["--lsp-mode".into(), "semantic-filtered".into()],
            )));
            let recovered_server = McpServer::with_semantic_registry(
                temp.path().into(),
                None,
                Arc::new(recovered_registry),
            );
            let recovered = call_semantic_mcp_tool(
                &recovered_server,
                "omen_symbol_definition",
                json!({"symbol":"refresh_token"}),
            )
            .await;
            assert_eq!(recovered["data"]["resolved"]["file"], "src/lib.rs");
        },
    )
    .await;
}
