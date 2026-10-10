use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdout, Command, Stdio};

struct Server {
    child: Child,
    out: BufReader<ChildStdout>,
}

impl Server {
    fn start() -> Server {
        let mut child = Command::new(env!("CARGO_BIN_EXE_novo"))
            .arg("lsp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let out = BufReader::new(child.stdout.take().unwrap());
        Server { child, out }
    }

    fn send(&mut self, message: Value) {
        let body = message.to_string();
        write!(
            self.child.stdin.as_mut().unwrap(),
            "Content-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    }

    fn receive(&mut self) -> Value {
        let mut length = 0;
        loop {
            let mut line = String::new();
            self.out.read_line(&mut line).unwrap();
            let line = line.trim();
            if line.is_empty() {
                break;
            }
            if let Some(n) = line.strip_prefix("Content-Length: ") {
                length = n.parse().unwrap();
            }
        }
        let mut body = vec![0; length];
        self.out.read_exact(&mut body).unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    fn request(&mut self, id: i64, method: &str, params: Value) -> Value {
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let reply = self.receive();
            if reply["id"] == id {
                return reply["result"].clone();
            }
        }
    }
}

#[test]
fn diagnostics_hover_and_formatting() {
    let dir = std::env::temp_dir().join(format!("novo-lsp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("main.nv");
    let uri = format!("file://{}", path.display().to_string().replace('\\', "/"));
    let uri = if uri.starts_with("file:///") {
        uri
    } else {
        uri.replacen("file://", "file:///", 1)
    };

    let mut server = Server::start();
    let init = server.request(1, "initialize", json!({ "capabilities": {} }));
    assert_eq!(init["capabilities"]["hoverProvider"], true);
    server.send(json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }));

    let open = |text: &str| json!({ "textDocument": { "uri": uri, "languageId": "novo", "version": 1, "text": text } });
    server.send(json!({ "jsonrpc": "2.0", "method": "textDocument/didOpen", "params": open("x = 1\ny = x + \"a\"\n") }));
    let diags = server.receive();
    assert_eq!(diags["method"], "textDocument/publishDiagnostics");
    assert_eq!(diags["params"]["uri"], uri);
    let first = &diags["params"]["diagnostics"][0];
    assert!(
        first["message"]
            .as_str()
            .unwrap()
            .contains("cannot apply `+`")
    );
    assert_eq!(
        first["range"]["start"],
        json!({ "line": 1, "character": 4 })
    );

    let fixed = "names = [\"a\"]\ncount=len(names)\n";
    server.send(json!({ "jsonrpc": "2.0", "method": "textDocument/didChange", "params": { "textDocument": { "uri": uri, "version": 2 }, "contentChanges": [{ "text": fixed }] } }));
    assert_eq!(server.receive()["params"]["diagnostics"], json!([]));

    let hover = server.request(
        2,
        "textDocument/hover",
        json!({ "textDocument": { "uri": uri }, "position": { "line": 1, "character": 11 } }),
    );
    assert_eq!(
        hover["contents"]["value"],
        "```novo\nnames: List[String]\n```"
    );
    let hover = server.request(
        3,
        "textDocument/hover",
        json!({ "textDocument": { "uri": uri }, "position": { "line": 1, "character": 1 } }),
    );
    assert_eq!(hover["contents"]["value"], "```novo\ncount: Int\n```");

    let def = server.request(
        6,
        "textDocument/definition",
        json!({ "textDocument": { "uri": uri }, "position": { "line": 1, "character": 11 } }),
    );
    assert_eq!(def["uri"], uri);
    assert_eq!(def["range"]["start"], json!({ "line": 0, "character": 0 }));

    let edits = server.request(4, "textDocument/formatting", json!({ "textDocument": { "uri": uri }, "options": { "tabSize": 4, "insertSpaces": true } }));
    assert_eq!(edits[0]["newText"], "names = [\"a\"]\ncount = len(names)\n");

    server.request(5, "shutdown", Value::Null);
    server.send(json!({ "jsonrpc": "2.0", "method": "exit" }));
    assert!(server.child.wait().unwrap().success());
    let _ = std::fs::remove_dir_all(&dir);
}
