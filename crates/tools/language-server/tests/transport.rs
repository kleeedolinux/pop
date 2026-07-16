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
    assert_eq!(messages[0]["result"]["capabilities"]["hoverProvider"], true);
    assert_eq!(
        messages[0]["result"]["capabilities"]["documentSymbolProvider"],
        true
    );
    assert_eq!(
        messages[0]["result"]["capabilities"]["codeActionProvider"],
        true
    );
    assert_eq!(
        messages[0]["result"]["capabilities"]["inlayHintProvider"],
        true
    );
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
fn stdio_exposes_compiler_quick_fixes_and_direct_call_parameter_hints() {
    let uri = "file:///workspace/actions.pop";
    let hint_uri = "file:///workspace/hints.pop";
    let source = "namespace Example\nexport function add(left: Int, right: Int): Int\n    return left + right\nend\nfunction value(): Int\n    return add(1, 2)\nend\n";
    let input = session(&[
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"locale":"en","capabilities":{}}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"languageId":"pop","version":7,"text":source}}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":hint_uri,"languageId":"pop","version":1,"text":"namespace Example\nfunction add(left: Int, right: Int): Int\n    return left + right\nend\nfunction value(): Int\n    return add(1, 2)\nend\n"}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"textDocument/codeAction","params":{"textDocument":{"uri":uri},"range":{"start":{"line":1,"character":0},"end":{"line":1,"character":6}},"context":{"diagnostics":[{"range":{"start":{"line":1,"character":0},"end":{"line":1,"character":6}},"severity":1,"code":"POP0004","source":"pop","message":"unsupported export","data":{"documentVersion":7,"fixIds":["replaceExportWithPublic"]}}]}}}),
        json!({"jsonrpc":"2.0","id":5,"method":"textDocument/codeAction","params":{"textDocument":{"uri":uri},"range":{"start":{"line":1,"character":0},"end":{"line":1,"character":6}},"context":{"diagnostics":[{"range":{"start":{"line":1,"character":0},"end":{"line":1,"character":6}},"severity":1,"code":"POP0004","source":"pop","message":"stale export","data":{"documentVersion":6,"fixIds":["replaceExportWithPublic"]}}]}}}),
        json!({"jsonrpc":"2.0","id":4,"method":"textDocument/inlayHint","params":{"textDocument":{"uri":hint_uri},"range":{"start":{"line":0,"character":0},"end":{"line":7,"character":0}}}}),
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
    let messages = responses(&output);
    let publication = messages
        .iter()
        .find(|message| message["method"] == "textDocument/publishDiagnostics")
        .expect("diagnostics");
    let diagnostic = publication["params"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|diagnostic| diagnostic["code"] == "POP0004")
        .expect("export diagnostic");
    assert_eq!(diagnostic["source"], "pop");
    assert_eq!(diagnostic["data"]["category"], "Syntax");
    assert_eq!(diagnostic["data"]["documentVersion"], 7);
    assert_eq!(diagnostic["data"]["fixIds"][0], "replaceExportWithPublic");

    let actions = &messages.iter().find(|message| message["id"] == 3).unwrap()["result"];
    assert_eq!(actions[0]["kind"], "quickfix");
    assert_eq!(actions[0]["isPreferred"], true);
    assert_eq!(
        actions[0]["edit"]["documentChanges"][0]["textDocument"]["version"],
        7
    );
    assert_eq!(
        actions[0]["edit"]["documentChanges"][0]["edits"][0]["newText"],
        "public"
    );
    assert!(
        messages.iter().find(|message| message["id"] == 5).unwrap()["result"]
            .as_array()
            .unwrap()
            .is_empty(),
        "a stale diagnostic snapshot must not receive a current edit"
    );

    let hints = messages.iter().find(|message| message["id"] == 4).unwrap()["result"]
        .as_array()
        .unwrap();
    assert!(hints.iter().any(|hint| hint["label"] == "left:"));
    assert!(hints.iter().any(|hint| hint["label"] == "right:"));
}

#[test]
fn stdio_preserves_compiler_secondary_labels_as_related_information() {
    let uri = "file:///workspace/labels.pop";
    let source = "namespace Example\npublic union Choice\n    One(value: Int)\n    Two\nend\nfunction read(choice: Choice): Int\n    match choice\n    when Choice.One(value) then\n        return value\n    when Choice.One(other) then\n        return other\n    when Choice.Two then\n        return 0\n    end\nend\n";
    let input = session(&[
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"locale":"en","capabilities":{}}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"languageId":"pop","version":1,"text":source}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"shutdown","params":null}),
        json!({"jsonrpc":"2.0","method":"exit","params":null}),
    ]);
    let mut output = Vec::new();
    serve(
        BufReader::new(Cursor::new(input)),
        &mut output,
        TransportLimits::default(),
    )
    .unwrap();
    let publication = responses(&output)
        .into_iter()
        .find(|message| message["method"] == "textDocument/publishDiagnostics")
        .unwrap();
    let diagnostic = publication["params"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|diagnostic| diagnostic["code"] == "POP2021")
        .expect("duplicate case diagnostic");
    assert_eq!(diagnostic["data"]["category"], "Type");
    assert_eq!(diagnostic["relatedInformation"][0]["location"]["uri"], uri);
    assert!(
        !diagnostic["relatedInformation"][0]["message"]
            .as_str()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn stdio_hover_and_document_symbols_use_compiler_results() {
    let uri = "file:///workspace/tooling.pop";
    let input = session(&[
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"locale":"en","capabilities":{}}}),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"languageId":"pop","version":1,"text":"namespace Example\n--- <summary>\n--- Returns one.\n--- </summary>\npublic function one(): Int\n    return 1\nend\n"}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"textDocument/hover","params":{"textDocument":{"uri":uri},"position":{"line":4,"character":18}}}),
        json!({"jsonrpc":"2.0","id":4,"method":"textDocument/documentSymbol","params":{"textDocument":{"uri":uri}}}),
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
    let messages = responses(&output);
    let hover = messages
        .iter()
        .find(|message| message["id"] == 3)
        .expect("hover");
    assert!(
        hover["result"]["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("Returns one.")
    );
    let symbols = messages
        .iter()
        .find(|message| message["id"] == 4)
        .expect("symbols");
    assert_eq!(symbols["result"][0]["name"], "one");
    assert_eq!(symbols["result"][0]["kind"], 12);
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
fn invalid_rich_request_params_do_not_terminate_the_session() {
    let input = session(&[
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}),
        json!({"jsonrpc":"2.0","id":8,"method":"textDocument/codeAction","params":{}}),
        json!({"jsonrpc":"2.0","id":9,"method":"textDocument/inlayHint","params":{}}),
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
    for id in [8, 9] {
        let error = messages
            .iter()
            .find(|message| message["id"] == id)
            .expect("invalid params response");
        assert_eq!(error["error"]["code"], -32602);
    }
    assert!(messages.iter().any(|message| message["id"] == 2));
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
