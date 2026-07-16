use std::io::Write as _;
use std::io::{BufReader, Cursor};
use std::process::{Command, Stdio};

use pop_language_server::{ExitStatus, TransportError, TransportLimits, serve};
use serde_json::{Value, json};

fn frame(value: &Value) -> Vec<u8> {
    let body = serde_json::to_vec(value).expect("JSON");
    let mut framed = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    framed.extend(body);
    framed
}

fn session(messages: &[Value]) -> Vec<u8> {
    messages.iter().flat_map(frame).collect()
}

fn responses(bytes: &[u8]) -> Vec<Value> {
    let mut cursor = 0;
    let mut values = Vec::new();
    while cursor < bytes.len() {
        let header_end = bytes[cursor..]
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|offset| cursor + offset)
            .expect("response header");
        let header = std::str::from_utf8(&bytes[cursor..header_end]).expect("UTF-8 header");
        let length = header
            .strip_prefix("Content-Length: ")
            .expect("content length")
            .parse::<usize>()
            .expect("numeric length");
        let body_start = header_end + 4;
        let body_end = body_start + length;
        values.push(serde_json::from_slice(&bytes[body_start..body_end]).expect("response JSON"));
        cursor = body_end;
    }
    values
}

#[test]
fn stdio_session_negotiates_utf16_and_publishes_localized_diagnostics() {
    let uri = "file:///workspace/main.pop";
    let input = session(&[
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {"locale": "pt-BR", "capabilities": {}}
        }),
        json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
        json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {"textDocument": {
                "uri": uri,
                "languageId": "pop",
                "version": 1,
                "text": "namespace Exemplo\npublic function quebrada(\n"
            }}
        }),
        json!({"jsonrpc": "2.0", "id": 2, "method": "shutdown", "params": null}),
        json!({"jsonrpc": "2.0", "method": "exit", "params": null}),
    ]);
    let mut output = Vec::new();
    let status = serve(
        BufReader::new(Cursor::new(input)),
        &mut output,
        TransportLimits::default(),
    )
    .expect("serve session");
    assert_eq!(status, ExitStatus::Success);

    let messages = responses(&output);
    assert_eq!(messages[0]["id"], 1);
    assert_eq!(
        messages[0]["result"]["capabilities"]["positionEncoding"],
        "utf-16"
    );
    assert_eq!(messages[0]["result"]["capabilities"]["textDocumentSync"], 1);
    let publication = messages
        .iter()
        .find(|message| message["method"] == "textDocument/publishDiagnostics")
        .expect("diagnostic publication");
    assert_eq!(publication["params"]["uri"], uri);
    assert_eq!(publication["params"]["version"], 1);
    let diagnostics = publication["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics");
    assert!(!diagnostics.is_empty());
    assert!(diagnostics.iter().all(|diagnostic| {
        diagnostic["code"]
            .as_str()
            .is_some_and(|code| code.starts_with("POP"))
    }));
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic["message"]
            .as_str()
            .is_some_and(|message| message.contains("Esperado") || message.contains("esperado"))
    }));
    assert_eq!(messages.last().expect("shutdown response")["id"], 2);
}

#[test]
fn change_republishes_and_close_clears_diagnostics() {
    let uri = "file:///workspace/main.pop";
    let input = session(&[
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"locale":"en","capabilities":{}}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"languageId":"pop","version":1,"text":"namespace Example\n"}}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":"namespace Example\npublic function broken(\n"}]}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didClose","params":{"textDocument":{"uri":uri}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ]);
    let mut output = Vec::new();
    serve(
        BufReader::new(Cursor::new(input)),
        &mut output,
        TransportLimits::default(),
    )
    .expect("serve session");
    let publications: Vec<_> = responses(&output)
        .into_iter()
        .filter(|message| message["method"] == "textDocument/publishDiagnostics")
        .collect();
    assert_eq!(publications.len(), 3);
    assert!(
        publications[0]["params"]["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(publications[1]["params"]["version"], 2);
    assert!(
        !publications[1]["params"]["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        publications[2]["params"]["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn unknown_request_gets_method_not_found_without_terminating_session() {
    let input = session(&[
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}),
        json!({"jsonrpc":"2.0","id":9,"method":"pop/privateUnknown","params":null}),
        json!({"jsonrpc":"2.0","id":2,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ]);
    let mut output = Vec::new();
    let status = serve(
        BufReader::new(Cursor::new(input)),
        &mut output,
        TransportLimits::default(),
    )
    .expect("serve session");
    assert_eq!(status, ExitStatus::Success);
    let messages = responses(&output);
    let error = messages
        .iter()
        .find(|message| message["id"] == 9)
        .expect("method error");
    assert_eq!(error["error"]["code"], -32601);
}

#[test]
fn transport_rejects_frames_over_the_configured_limit() {
    let input = b"Content-Length: 1025\r\n\r\n".to_vec();
    let error = serve(
        BufReader::new(Cursor::new(input)),
        Vec::new(),
        TransportLimits::new(1024, 512),
    )
    .expect_err("oversized frame");
    assert_eq!(
        error,
        TransportError::FrameTooLarge {
            length: 1025,
            limit: 1024
        }
    );
}

#[test]
fn exit_without_shutdown_reports_failure() {
    let input = session(&[
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ]);
    let status = serve(
        BufReader::new(Cursor::new(input)),
        Vec::new(),
        TransportLimits::default(),
    )
    .expect("serve session");
    assert_eq!(status, ExitStatus::Failure);
}

#[test]
fn oversized_document_is_reported_in_the_session_language() {
    let uri = "file:///workspace/large.pop";
    let input = session(&[
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"locale":"es","capabilities":{}}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"languageId":"pop","version":1,"text":"namespace TooLarge\n"}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ]);
    let mut output = Vec::new();
    serve(
        BufReader::new(Cursor::new(input)),
        &mut output,
        TransportLimits::new(4096, 8),
    )
    .expect("serve session");
    let log = responses(&output)
        .into_iter()
        .find(|message| message["method"] == "window/logMessage")
        .expect("localized size error");
    assert!(
        log["params"]["message"]
            .as_str()
            .unwrap()
            .contains("límite")
    );
    assert!(log["params"]["message"].as_str().unwrap().contains(uri));
}

#[test]
fn executable_serves_a_complete_stdio_session() {
    let input = session(&[
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ]);
    let mut child = Command::new(env!("CARGO_BIN_EXE_pop-language-server"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start language server");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(&input)
        .expect("write protocol session");
    let output = child.wait_with_output().expect("wait for language server");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let messages = responses(&output.stdout);
    assert_eq!(messages[0]["result"]["serverInfo"]["name"], "Pop Lang");
    assert_eq!(messages[1]["id"], 2);
}
