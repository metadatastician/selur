// SPDX-License-Identifier: MPL-2.0
// SPDX-FileCopyrightText: 2026 Jonathan D.A. Jewell <6759885+hyperpolymath@users.noreply.github.com>
//
//! End-to-end tests for the selur IPC pipeline.
//!
//! These tests exercise complete data-flow paths through the selur types:
//! config parsing → validation → binary encoding → decoding → verification.
//! None of these tests require a live WASM module or Vörðr instance.

use selur::{Command, StatusCode, VordrConfig};

// ---------------------------------------------------------------------------
// Binary protocol round-trip
// ---------------------------------------------------------------------------

/// Build a binary request packet in selur protocol format:
/// [command:1B][payload_len:4B LE][payload:NB]
fn build_request(cmd: Command, payload: &[u8]) -> Vec<u8> {
    let mut req = Vec::with_capacity(5 + payload.len());
    req.push(cmd as u8);
    req.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    req.extend_from_slice(payload);
    req
}

/// Parse the binary request packet back to (command_byte, payload).
fn parse_request(req: &[u8]) -> Option<(u8, &[u8])> {
    if req.len() < 5 {
        return None;
    }
    let cmd_byte = req[0];
    let payload_len = u32::from_le_bytes([req[1], req[2], req[3], req[4]]) as usize;
    if req.len() < 5 + payload_len {
        return None;
    }
    Some((cmd_byte, &req[5..5 + payload_len]))
}

/// Build a binary response packet: [status:1B][data_len:4B LE][data:NB]
fn build_response(status: StatusCode, data: &[u8]) -> Vec<u8> {
    let mut resp = Vec::with_capacity(5 + data.len());
    resp.push(status as u8);
    resp.extend_from_slice(&(data.len() as u32).to_le_bytes());
    resp.extend_from_slice(data);
    resp
}

/// Parse a binary response packet back to (status_byte, data).
fn parse_response(resp: &[u8]) -> Option<(u8, &[u8])> {
    if resp.len() < 5 {
        return None;
    }
    let status_byte = resp[0];
    let data_len = u32::from_le_bytes([resp[1], resp[2], resp[3], resp[4]]) as usize;
    if resp.len() < 5 + data_len {
        return None;
    }
    Some((status_byte, &resp[5..5 + data_len]))
}

// ---------------------------------------------------------------------------
// E2E: config parse → binary encode → decode → verify
// ---------------------------------------------------------------------------

#[test]
fn e2e_create_container_request_round_trip() {
    // Simulate: user config → binary request
    let container_spec = r#"{"name":"web","image":"nginx:1.27","ports":["80:80"]}"#;
    let payload = container_spec.as_bytes();

    let request = build_request(Command::CreateContainer, payload);

    // Round-trip decode
    let (cmd_byte, decoded_payload) = parse_request(&request)
        .expect("request parse must succeed for valid packet");

    let cmd = Command::from_u8(cmd_byte)
        .expect("command byte must map to a known Command");

    assert_eq!(cmd, Command::CreateContainer, "command must round-trip correctly");
    assert_eq!(decoded_payload, payload, "payload must round-trip correctly");
    assert_eq!(
        std::str::from_utf8(decoded_payload).expect("payload must be valid UTF-8"),
        container_spec,
        "container spec string must be preserved verbatim"
    );
}

#[test]
fn e2e_success_response_round_trip() {
    // Simulate Vörðr responding with container ID
    let response_data = r#"{"id":"abc123","name":"web","status":"created"}"#;
    let response = build_response(StatusCode::Success, response_data.as_bytes());

    let (status_byte, decoded_data) = parse_response(&response)
        .expect("response parse must succeed for valid packet");

    let status = StatusCode::from_u8(status_byte)
        .expect("status byte must map to a known StatusCode");

    assert_eq!(status, StatusCode::Success, "status must round-trip correctly");
    assert_eq!(decoded_data, response_data.as_bytes(), "response data must round-trip");
}

#[test]
fn e2e_error_response_round_trip() {
    let error_msg = "container already exists: web";
    let response = build_response(StatusCode::ContainerAlreadyExists, error_msg.as_bytes());

    let (status_byte, decoded_msg) = parse_response(&response)
        .expect("error response parse must succeed");

    let status = StatusCode::from_u8(status_byte)
        .expect("error status byte must be valid");

    assert_eq!(status, StatusCode::ContainerAlreadyExists);
    assert_eq!(
        std::str::from_utf8(decoded_msg).expect("error message must be valid UTF-8"),
        error_msg
    );
}

// ---------------------------------------------------------------------------
// E2E: multi-container compose sequence
// ---------------------------------------------------------------------------

#[test]
fn e2e_multi_container_compose_ordering() {
    // Simulate a 3-service compose: db must start before api, api before web
    // This tests dependency resolution logic encoded in request ordering.
    let services: Vec<(&str, &str, Vec<&str>)> = vec![
        ("web", "nginx:1.27", vec!["api"]),
        ("api", "myapp:2.0", vec!["db"]),
        ("db", "postgres:16-alpine", vec![]),
    ];

    // Topological sort: db → api → web
    fn topo_order<'a>(services: &[(&'a str, &'a str, Vec<&'a str>)]) -> Vec<&'a str> {
        let mut visited: Vec<&'a str> = Vec::new();
        let mut stack: Vec<&'a str> = Vec::new();

        fn visit<'b>(
            name: &'b str,
            services: &[(&'b str, &'b str, Vec<&'b str>)],
            visited: &mut Vec<&'b str>,
            stack: &mut Vec<&'b str>,
        ) {
            if visited.contains(&name) {
                return;
            }
            visited.push(name);
            if let Some((_, _, deps)) = services.iter().find(|(n, _, _)| *n == name) {
                for dep in deps {
                    visit(dep, services, visited, stack);
                }
            }
            stack.push(name);
        }

        for (name, _, _) in services {
            visit(name, services, &mut visited, &mut stack);
        }
        stack
    }

    let order = topo_order(&services);

    // db must come before api which must come before web
    let db_pos = order.iter().position(|&n| n == "db")
        .expect("db must appear in ordering");
    let api_pos = order.iter().position(|&n| n == "api")
        .expect("api must appear in ordering");
    let web_pos = order.iter().position(|&n| n == "web")
        .expect("web must appear in ordering");

    assert!(db_pos < api_pos, "db must start before api");
    assert!(api_pos < web_pos, "api must start before web");

    // Build requests in correct order
    let requests: Vec<Vec<u8>> = order.iter().map(|name| {
        let spec = format!(r#"{{"name":"{}"}}"#, name);
        build_request(Command::CreateContainer, spec.as_bytes())
    }).collect();

    assert_eq!(requests.len(), 3, "three requests must be generated");
    for req in &requests {
        let (cmd_byte, _) = parse_request(req)
            .expect("each request must be valid");
        assert_eq!(
            Command::from_u8(cmd_byte),
            Some(Command::CreateContainer),
            "all compose requests must be CreateContainer"
        );
    }
}

// ---------------------------------------------------------------------------
// E2E: Volume and network config round-trip
// ---------------------------------------------------------------------------

#[test]
fn e2e_volume_and_network_schema_round_trip() {
    // Simulate a full selur compose schema with volumes and networks
    let compose_spec = r#"{
        "version": "1.0",
        "services": {
            "db": {
                "image": "postgres:16-alpine",
                "volumes": ["/data/postgres:/var/lib/postgresql/data"],
                "environment": {"POSTGRES_DB": "myapp", "POSTGRES_USER": "appuser"},
                "networks": ["backend"]
            }
        },
        "volumes": {"pgdata": {"driver": "local"}},
        "networks": {"backend": {"driver": "bridge"}}
    }"#;

    // Parse → encode → decode
    let request = build_request(Command::CreateContainer, compose_spec.as_bytes());
    let (cmd_byte, decoded) = parse_request(&request)
        .expect("compose spec round-trip must succeed");

    assert_eq!(Command::from_u8(cmd_byte), Some(Command::CreateContainer));

    let decoded_str = std::str::from_utf8(decoded)
        .expect("decoded payload must be valid UTF-8");

    // Verify key structural elements survive round-trip
    assert!(decoded_str.contains("postgres:16-alpine"), "image must survive round-trip");
    assert!(decoded_str.contains("/data/postgres"), "volume path must survive round-trip");
    assert!(decoded_str.contains("POSTGRES_DB"), "env var key must survive round-trip");
    assert!(decoded_str.contains("backend"), "network name must survive round-trip");
}

// ---------------------------------------------------------------------------
// E2E: Error path — circular dependency detection
// ---------------------------------------------------------------------------

#[test]
fn e2e_circular_dependency_detected() {
    // Services a→b→a form a cycle
    let deps: Vec<(&str, Vec<&str>)> = vec![
        ("a", vec!["b"]),
        ("b", vec!["a"]),
    ];


    fn has_cycle(deps: &[(&str, Vec<&str>)]) -> bool {
        fn dfs<'a>(
            node: &'a str,
            deps: &[(&'a str, Vec<&'a str>)],
            visiting: &mut Vec<&'a str>,
            visited: &mut Vec<&'a str>,
        ) -> bool {
            if visiting.contains(&node) {
                return true; // cycle detected
            }
            if visited.contains(&node) {
                return false;
            }
            visiting.push(node);
            if let Some((_, dep_list)) = deps.iter().find(|(n, _)| *n == node) {
                for dep in dep_list {
                    if dfs(dep, deps, visiting, visited) {
                        return true;
                    }
                }
            }
            visiting.retain(|&n| n != node);
            visited.push(node);
            false
        }

        let mut visiting = Vec::new();
        let mut visited = Vec::new();
        for (name, _) in deps {
            if dfs(name, deps, &mut visiting, &mut visited) {
                return true;
            }
        }
        false
    }

    assert!(has_cycle(&deps), "circular dependency a→b→a must be detected");
}

// ---------------------------------------------------------------------------
// E2E: Error path — invalid port range rejected
// ---------------------------------------------------------------------------

#[test]
fn e2e_invalid_port_range_rejected() {
    // Port 0 and port 65536 are invalid
    fn is_valid_port(port: u32) -> bool {
        port >= 1 && port <= 65535
    }

    assert!(!is_valid_port(0), "port 0 must be rejected");
    assert!(!is_valid_port(65536), "port 65536 must be rejected");
    assert!(is_valid_port(80), "port 80 must be accepted");
    assert!(is_valid_port(65535), "port 65535 must be accepted");
    assert!(is_valid_port(1), "port 1 must be accepted");
}

// ---------------------------------------------------------------------------
// E2E: VordrConfig construction and defaults
// ---------------------------------------------------------------------------

#[test]
fn e2e_vordr_config_defaults_are_sane() {
    let config = VordrConfig::default();

    // Timeout must be positive and reasonable (10s default)
    assert!(config.timeout_secs > 0, "timeout must be positive");
    assert!(config.timeout_secs <= 120, "timeout must be <= 120s to avoid hung operations");

    // Base URL must be non-empty
    assert!(!config.base_url.is_empty(), "base URL must not be empty");

    // Must reference a local endpoint by default (not a remote host)
    assert!(
        config.base_url.contains("127.0.0.1") || config.base_url.contains("localhost"),
        "default Vörðr endpoint '{}' must point to localhost", config.base_url
    );
}

#[test]
fn e2e_vordr_config_explicit() {
    let config = VordrConfig {
        base_url: "http://10.0.0.1:4010".to_string(),
        timeout_secs: 30,
    };

    assert_eq!(config.base_url, "http://10.0.0.1:4010");
    assert_eq!(config.timeout_secs, 30);
}

// ---------------------------------------------------------------------------
// E2E: All commands have an endpoint
// ---------------------------------------------------------------------------

#[test]
fn e2e_all_commands_have_endpoint() {
    let commands = [
        Command::CreateContainer,
        Command::StartContainer,
        Command::StopContainer,
        Command::InspectContainer,
        Command::DeleteContainer,
        Command::ListContainers,
    ];

    for cmd in &commands {
        let (method, path) = cmd.vordr_endpoint();

        assert!(
            ["GET", "POST", "DELETE"].contains(&method),
            "command {:?} has unknown HTTP method '{}'", cmd, method
        );
        assert!(
            path.starts_with("/api/v1/"),
            "command {:?} endpoint '{}' must be under /api/v1/", cmd, path
        );
        assert!(!path.is_empty(), "command {:?} must have a non-empty path", cmd);
    }
}
