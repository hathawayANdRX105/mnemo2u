//! R4.T7: degradation fault injection against a raw-TCP
//! mock endpoint (断网 / 500 / 格式错) plus the zero-egress
//! gate (AC1: no opt-in → zero calls, asserted at the
//! connection level).
//!
//! The mock speaks bare HTTP/1.1 over `std::net` — one
//! response per accepted connection — so the wire client
//! (reqwest) is exercised end to end without a server
//! framework dependency.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use mnemo2u::llm::envelope::{DefaultMeteringSink, EgressGate, Policy, ProviderError};
use mnemo2u::llm::judge::{ClassifyRequest, JudgeClient, JudgeError};
use mnemo2u::llm::judge_jev::{JudgeEndpointClient, JudgeEndpointConfig};

/// Serves `responses` in order, one per accepted connection.
fn spawn_endpoint(responses: Vec<(u16, String)>) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let handle = thread::spawn(move || {
        for (status, body) in responses {
            let (mut stream, _) = listener.accept().expect("accept");
            drain_request(&mut stream);
            let response = format!(
                "HTTP/1.1 {status} Status\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).expect("write");
            stream.flush().expect("flush");
        }
    });
    (format!("http://{addr}/judge"), handle)
}

/// Reads one request (head plus `Content-Length`
/// body) so the client's write completes before
/// the response arrives — closing early would
/// reset the write and fail `send`.
fn drain_request(stream: &mut TcpStream) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).expect("read line") == 0 {
            break;
        }
        let line = line.trim_end().to_ascii_lowercase();
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("content-length:") {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    if content_length > 0 {
        let mut body = vec![0u8; content_length];
        reader
            .read_exact(&mut body)
            .expect("drain the request body");
    }
}

/// Counts accepted connections; stops after `expected`.
fn spawn_counting_endpoint(expected: usize) -> (String, Arc<AtomicUsize>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let accepted = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&accepted);
    let handle = thread::spawn(move || {
        while counter.load(Ordering::SeqCst) < expected {
            let (stream, _) = match listener.accept() {
                Ok(accepted) => accepted,
                Err(_) => break,
            };
            counter.fetch_add(1, Ordering::SeqCst);
            drop(stream);
        }
    });
    (format!("http://{addr}/judge"), accepted, handle)
}

/// One attempt, no backoff: a fault test must observe the
/// fault, not wait out a retry ladder.
fn single_attempt_policy() -> Policy {
    Policy {
        max_attempts: 1,
        initial_backoff: Duration::from_millis(1),
        max_backoff: Duration::from_millis(1),
        timeout: Duration::from_secs(5),
    }
}

fn two_attempt_policy() -> Policy {
    Policy {
        max_attempts: 2,
        initial_backoff: Duration::from_millis(1),
        max_backoff: Duration::from_millis(1),
        timeout: Duration::from_secs(5),
    }
}

fn client(endpoint: &str, policy: Policy, gate: EgressGate) -> JudgeEndpointClient {
    // A unique env var per process run: tests share the
    // process, and the key name is per-config anyway.
    let var = "MNEMO2U_JEV_TEST_KEY";
    std::env::set_var(var, "test-key");
    JudgeEndpointClient::new(
        JudgeEndpointConfig {
            endpoint: endpoint.to_string(),
            api_key_env: var.to_string(),
            model: "judge-test".to_string(),
            policy,
            batch_limit: 64,
            concurrency: 4,
        },
        reqwest::Client::new(),
        gate,
        Arc::new(DefaultMeteringSink::new()),
    )
    .expect("api key set")
}

fn request() -> ClassifyRequest {
    ClassifyRequest {
        items: vec![mnemo2u::llm::judge::ClassifyItem {
            id: "c0".into(),
            text: "ACME acquired Beta Labs".into(),
        }],
        catalog: vec![
            mnemo2u::llm::judge::ClassifyClass {
                id: "semantic".into(),
                description: "a durable statement".into(),
            },
            mnemo2u::llm::judge::ClassifyClass {
                id: "episodic".into(),
                description: "an event that happened".into(),
            },
        ],
        purpose: Some("write typing".into()),
    }
}

/// 断网: nothing listens — every attempt dies at the
/// transport layer and the call degrades to a backend
/// error, never to a silent fallback.
#[tokio::test]
async fn network_down_degrades_to_a_backend_error() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener); // nothing listens on this port now
    let endpoint = format!("http://127.0.0.1:{port}/judge");
    let client = client(&endpoint, single_attempt_policy(), EgressGate::new(true));

    let result = client.classify(&request()).await;
    assert!(
        matches!(
            result,
            Err(JudgeError::Backend(ProviderError::Unavailable { .. }))
        ),
        "a dead endpoint is a backend error, got {result:?}"
    );
}

/// 500: retried per policy, then degraded — the retry
/// ladder is observable at the connection level.
#[tokio::test]
async fn http_500_retries_then_degrades() {
    let (endpoint, server) = spawn_endpoint(vec![
        (500, "upstream exploded".into()),
        (500, "upstream exploded".into()),
    ]);
    let client = client(&endpoint, two_attempt_policy(), EgressGate::new(true));

    let result = client.classify(&request()).await;
    assert!(
        matches!(
            result,
            Err(JudgeError::Backend(ProviderError::Unavailable { .. }))
        ),
        "a 500 storm is a backend error, got {result:?}"
    );
    server.join().expect("server served both attempts");
}

/// 格式错: a 200 whose body is not the wire shape is an
/// invalid response — fixed by repairing the call, not by
/// retrying or substituting another model.
#[tokio::test]
async fn malformed_body_is_an_invalid_response() {
    let (endpoint, server) = spawn_endpoint(vec![(200, "not the jev wire shape".into())]);
    let client = client(&endpoint, single_attempt_policy(), EgressGate::new(true));

    let result = client.classify(&request()).await;
    assert!(
        matches!(result, Err(JudgeError::InvalidResponse)),
        "a malformed body is invalid_response, got {result:?}"
    );
    server.join().expect("server served one attempt");
}

/// AC1: without the workspace opt-in the judge capability
/// is denied before any network I/O — the endpoint sees
/// zero connections.
#[tokio::test]
async fn zero_egress_until_opted_in() {
    let (endpoint, accepted, server) = spawn_counting_endpoint(0);
    let client = client(&endpoint, single_attempt_policy(), EgressGate::new(false));

    let result = client.classify(&request()).await;
    assert!(
        matches!(
            result,
            Err(JudgeError::Backend(ProviderError::PolicyDenied))
        ),
        "a closed gate denies the call, got {result:?}"
    );
    assert_eq!(
        accepted.load(Ordering::SeqCst),
        0,
        "the endpoint must see zero connections"
    );
    server.join().expect("server exited");
}

/// A well-formed wire answer round-trips through the client
/// (the positive control for the fault fixtures above).
#[tokio::test]
async fn a_well_formed_answer_classifies() {
    let body = serde_json::json!({
        "answers": {
            "cls_0": {
                "choice": "semantic",
                "probabilities": {"semantic": 0.95, "episodic": 0.05},
                "confidence": 0.95,
            }
        },
        "usage": {"input_tokens": 11, "output_tokens": 7},
        "model": "judge-test",
    })
    .to_string();
    let (endpoint, server) = spawn_endpoint(vec![(200, body)]);
    let client = client(&endpoint, single_attempt_policy(), EgressGate::new(true));

    let response = client.classify(&request()).await.expect("classify");
    assert_eq!(response.verdicts.len(), 1);
    assert_eq!(response.verdicts[0].class_id, "semantic");
    server.join().expect("server served one attempt");
}
