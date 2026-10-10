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

#[test]
fn references_rename_and_completion() {
    let dir = std::env::temp_dir().join(format!("novo-lsp-refs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("main.nv");
    let uri = format!("file://{}", path.display().to_string().replace('\\', "/"));
    let uri = if uri.starts_with("file:///") {
        uri
    } else {
        uri.replacen("file://", "file:///", 1)
    };
    let text = "class Point:
    x: Int
    y: Int

    function norm() -> Int:
        return self.x * self.x + self.y * self.y

function make(n: Int) -> Point:
    p = Point(n, n)
    return p

total = 0
for i in range(3):
    total += make(i).norm()
show total
";
    let mut server = Server::start();
    let init = server.request(1, "initialize", json!({ "capabilities": {} }));
    assert_eq!(init["capabilities"]["renameProvider"], true);
    server.send(json!({ "jsonrpc": "2.0", "method": "textDocument/didOpen", "params": { "textDocument": { "uri": uri, "languageId": "novo", "version": 1, "text": text } } }));
    assert_eq!(server.receive()["params"]["diagnostics"], json!([]));

    let at = |line: u32, character: u32| json!({ "textDocument": { "uri": uri }, "position": { "line": line, "character": character } });
    let starts = |result: &Value| -> Vec<(u64, u64)> {
        let mut v: Vec<_> = result
            .as_array()
            .unwrap()
            .iter()
            .map(|l| {
                let s = &l["range"]["start"];
                (
                    s["line"].as_u64().unwrap(),
                    s["character"].as_u64().unwrap(),
                )
            })
            .collect();
        v.sort();
        v
    };

    let refs = server.request(2, "textDocument/references", at(9, 11));
    assert_eq!(starts(&refs), vec![(8, 4), (9, 11)]);
    let refs = server.request(3, "textDocument/references", at(13, 14));
    assert_eq!(starts(&refs), vec![(7, 9), (13, 13)]);

    let mut params = at(0, 7);
    params["newName"] = json!("Vec2");
    let edit = server.request(4, "textDocument/rename", params);
    let edits = &edit["changes"][&uri];
    assert_eq!(starts(edits), vec![(0, 6), (7, 25), (8, 8)]);
    assert!(
        edits
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["newText"] == "Vec2")
    );

    let mut params = at(14, 6);
    params["newName"] = json!("sum_of_norms");
    let edit = server.request(5, "textDocument/rename", params);
    assert_eq!(
        starts(&edit["changes"][&uri]),
        vec![(11, 0), (13, 4), (14, 5)]
    );

    let labels = |result: &Value| -> Vec<String> {
        result
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["label"].as_str().unwrap().to_string())
            .collect()
    };
    let names = server.request(6, "textDocument/completion", at(14, 6));
    let names = labels(&names);
    for want in ["total", "make", "Point", "len", "nn", "while"] {
        assert!(names.iter().any(|n| n == want), "missing {want}");
    }

    let edited = text.replace("    return p\n", "    q = p.\n    return p\n");
    server.send(json!({ "jsonrpc": "2.0", "method": "textDocument/didChange", "params": { "textDocument": { "uri": uri, "version": 2 }, "contentChanges": [{ "text": edited }] } }));
    server.receive();
    let members = labels(&server.request(7, "textDocument/completion", at(9, 10)));
    assert_eq!(members, vec!["x", "y", "norm"]);
    let edited = text.replace("    return p\n", "    q = nn.\n    return p\n");
    server.send(json!({ "jsonrpc": "2.0", "method": "textDocument/didChange", "params": { "textDocument": { "uri": uri, "version": 3 }, "contentChanges": [{ "text": edited }] } }));
    server.receive();
    let module = labels(&server.request(8, "textDocument/completion", at(9, 11)));
    assert!(module.iter().any(|n| n == "zeros"));

    server.request(9, "shutdown", Value::Null);
    server.send(json!({ "jsonrpc": "2.0", "method": "exit" }));
    assert!(server.child.wait().unwrap().success());
    let _ = std::fs::remove_dir_all(&dir);
}
