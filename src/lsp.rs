//! A language server over stdio: diagnostics as you type, hover types,
//! definitions, references, renaming, completion and formatting.

use crate::check::builtins::{Builtin, METHOD_NAMES, StdModule, method_signature};
use crate::check::types::{ClassId, Ty};
use crate::check::{self, hir};
use crate::diagnostic::Diagnostic;
use crate::source::{FileId, SourceMap, Span};
use crate::syntax::token::Tok;
use serde_json::{Value as Json, json};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

struct Document {
    text: String,
    /// The last successful analysis, kept for hovering while the file has errors.
    analysis: Option<Analysis>,
    /// Files this document's diagnostics were last published to.
    published: HashSet<String>,
}

struct Analysis {
    program: hir::Program,
    sources: SourceMap,
    file: FileId,
}

pub fn serve() -> i32 {
    let mut input = std::io::stdin().lock();
    let mut docs: HashMap<String, Document> = HashMap::new();
    let mut shutting_down = false;
    while let Some(message) = read_message(&mut input) {
        let id = message.get("id").cloned();
        let method = message.get("method").and_then(Json::as_str).unwrap_or("");
        let params = message.get("params").cloned().unwrap_or(Json::Null);
        let result = match method {
            "initialize" => Some(json!({
                "capabilities": {
                    "textDocumentSync": 1,
                    "hoverProvider": true,
                    "definitionProvider": true,
                    "documentFormattingProvider": true,
                    "referencesProvider": true,
                    "renameProvider": true,
                    "completionProvider": { "triggerCharacters": ["."] },
                },
                "serverInfo": { "name": "novo", "version": env!("CARGO_PKG_VERSION") },
            })),
            "shutdown" => {
                shutting_down = true;
                Some(Json::Null)
            }
            "exit" => return if shutting_down { 0 } else { 1 },
            "textDocument/didOpen" => {
                let uri = str_at(&params, &["textDocument", "uri"]);
                let text = str_at(&params, &["textDocument", "text"]);
                let doc = docs.entry(uri.clone()).or_insert(Document {
                    text: String::new(),
                    analysis: None,
                    published: HashSet::new(),
                });
                doc.text = text;
                analyze(&uri, doc);
                None
            }
            "textDocument/didChange" => {
                let uri = str_at(&params, &["textDocument", "uri"]);
                let changes = params.get("contentChanges").and_then(Json::as_array);
                if let (Some(doc), Some(text)) = (
                    docs.get_mut(&uri),
                    changes
                        .and_then(|c| c.last())
                        .and_then(|c| c.get("text"))
                        .and_then(Json::as_str),
                ) {
                    doc.text = text.to_string();
                    analyze(&uri, doc);
                }
                None
            }
            "textDocument/didClose" => {
                let uri = str_at(&params, &["textDocument", "uri"]);
                if let Some(doc) = docs.remove(&uri) {
                    for file in doc.published {
                        publish(&file, Vec::new());
                    }
                }
                None
            }
            "textDocument/hover" => {
                let uri = str_at(&params, &["textDocument", "uri"]);
                Some(
                    docs.get(&uri)
                        .and_then(|doc| hover(doc, &params))
                        .unwrap_or(Json::Null),
                )
            }
            "textDocument/definition" => {
                let uri = str_at(&params, &["textDocument", "uri"]);
                Some(
                    docs.get(&uri)
                        .and_then(|doc| definition(doc, &params))
                        .unwrap_or(Json::Null),
                )
            }
            "textDocument/references" => {
                let uri = str_at(&params, &["textDocument", "uri"]);
                Some(
                    docs.get(&uri)
                        .and_then(|doc| references(doc, &params))
                        .unwrap_or(Json::Null),
                )
            }
            "textDocument/rename" => {
                let uri = str_at(&params, &["textDocument", "uri"]);
                Some(
                    docs.get(&uri)
                        .and_then(|doc| rename(doc, &params))
                        .unwrap_or(Json::Null),
                )
            }
            "textDocument/completion" => {
                let uri = str_at(&params, &["textDocument", "uri"]);
                Some(
                    docs.get(&uri)
                        .map(|doc| completion(doc, &params))
                        .unwrap_or(json!([])),
                )
            }
            "textDocument/formatting" => {
                let uri = str_at(&params, &["textDocument", "uri"]);
                Some(docs.get(&uri).map(format_edits).unwrap_or(json!([])))
            }
            _ if id.is_some() => {
                send(
                    &json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("unsupported method `{method}`") } }),
                );
                None
            }
            _ => None,
        };
        if let (Some(id), Some(result)) = (id, result) {
            send(&json!({ "jsonrpc": "2.0", "id": id, "result": result }));
        }
    }
    1
}

fn read_message(input: &mut impl BufRead) -> Option<Json> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if input.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let mut body = vec![0; length?];
    input.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

fn send(message: &Json) {
    let body = message.to_string();
    let mut out = std::io::stdout().lock();
    let _ = write!(out, "Content-Length: {}\r\n\r\n{body}", body.len());
    let _ = out.flush();
}

fn str_at(v: &Json, path: &[&str]) -> String {
    path.iter()
        .try_fold(v, |v, k| v.get(k))
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_string()
}

fn publish(uri: &str, diagnostics: Vec<Json>) {
    send(&json!({
        "jsonrpc": "2.0",
        "method": "textDocument/publishDiagnostics",
        "params": { "uri": uri, "diagnostics": diagnostics },
    }));
}

fn analyze(uri: &str, doc: &mut Document) {
    let Some(path) = uri_to_path(uri) else { return };
    let mut sources = SourceMap::default();
    let (program, diags) = check::analyze(&mut sources, &path, doc.text.clone());
    let file = sources
        .files()
        .position(|f| f.path == path)
        .map(|i| FileId(i as u32));

    let mut by_file: HashMap<String, Vec<Json>> = HashMap::new();
    for d in &diags {
        let target = match sources.files().nth(d.span.file.0 as usize) {
            Some(f) if f.path != path => path_to_uri(&f.path),
            _ => uri.to_string(),
        };
        by_file
            .entry(target)
            .or_default()
            .push(diagnostic(&sources, d));
    }
    let stale: Vec<String> = doc
        .published
        .iter()
        .filter(|f| !by_file.contains_key(*f))
        .cloned()
        .collect();
    for f in stale {
        publish(&f, Vec::new());
    }
    by_file.entry(uri.to_string()).or_default();
    doc.published = by_file.keys().cloned().collect();
    for (target, list) in by_file {
        publish(&target, list);
    }
    if let (Some(program), Some(file)) = (program, file) {
        doc.analysis = Some(Analysis {
            program,
            sources,
            file,
        });
    }
}

fn diagnostic(sources: &SourceMap, d: &Diagnostic) -> Json {
    let mut message = d.message.clone();
    if let Some(label) = &d.label {
        message += &format!(": {label}");
    }
    if let Some(help) = &d.help {
        message += &format!("\nhelp: {help}");
    }
    for note in &d.notes {
        message += &format!("\nnote: {note}");
    }
    json!({ "range": range(sources, d.span), "severity": 1, "source": "novo", "message": message })
}

fn range(sources: &SourceMap, span: Span) -> Json {
    let text = sources
        .files()
        .nth(span.file.0 as usize)
        .map_or("", |f| f.text.as_str());
    json!({ "start": position(text, span.lo as usize), "end": position(text, span.hi as usize) })
}

/// An LSP position: zero-based line and UTF-16 column.
fn position(text: &str, offset: usize) -> Json {
    let offset = floor_char_boundary(text, offset);
    let start = text[..offset].rfind('\n').map_or(0, |n| n + 1);
    let line = text[..offset].matches('\n').count();
    json!({ "line": line, "character": text[start..offset].encode_utf16().count() })
}

fn offset(text: &str, line: usize, character: usize) -> usize {
    let start = if line == 0 {
        0
    } else {
        text.match_indices('\n')
            .nth(line - 1)
            .map_or(text.len(), |(i, _)| i + 1)
    };
    let mut units = 0;
    for (i, c) in text[start..].char_indices() {
        if units >= character || c == '\n' {
            return start + i;
        }
        units += c.len_utf16();
    }
    text.len()
}

fn floor_char_boundary(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn format_edits(doc: &Document) -> Json {
    match crate::syntax::format::format(FileId(0), &doc.text) {
        Ok(formatted) if formatted != doc.text => json!([{
            "range": { "start": position(&doc.text, 0), "end": position(&doc.text, doc.text.len()) },
            "newText": formatted,
        }]),
        _ => json!([]),
    }
}

/// The innermost expression or assignment target under the cursor, with the function holding it.
fn node_at<'a>(
    doc: &'a Document,
    params: &Json,
) -> Option<(&'a Analysis, &'a hir::Func, Node<'a>)> {
    let analysis = doc.analysis.as_ref()?;
    let pos = params.get("position")?;
    let line = pos.get("line")?.as_u64()? as usize;
    let character = pos.get("character")?.as_u64()? as usize;
    let at = offset(&doc.text, line, character) as u32;
    let mut best: Option<(u32, &hir::Func, Node)> = None;
    for f in &analysis.program.funcs {
        walk_block(&f.body, &mut |node| {
            let span = match &node {
                Node::Expr(e) => e.span,
                Node::Target(span, _) => *span,
            };
            let width = span.hi - span.lo;
            if span.file == analysis.file
                && span.lo <= at
                && at < span.hi.max(span.lo + 1)
                && best.as_ref().is_none_or(|(w, ..)| width < *w)
            {
                best = Some((width, f, node));
            }
        });
    }
    best.map(|(_, f, node)| (analysis, f, node))
}

fn hover(doc: &Document, params: &Json) -> Option<Json> {
    let (analysis, f, node) = node_at(doc, params)?;
    let prog = &analysis.program;
    let classes: Vec<String> = prog.classes.iter().map(|c| c.name.clone()).collect();
    let named =
        |name: &str, ty: &crate::check::types::Ty| format!("{name}: {}", ty.display(&classes));
    let text = match node {
        Node::Expr(e) => match &e.kind {
            hir::ExprKind::Local(l) => named(&f.locals[l.0 as usize].name, &e.ty),
            hir::ExprKind::Global(g) => named(&prog.globals[g.0 as usize].name, &e.ty),
            hir::ExprKind::Func { func, .. } => named(
                &prog.funcs[func.0 as usize].name,
                &prog.funcs[func.0 as usize].ty,
            ),
            _ => e.ty.display(&classes).to_string(),
        },
        Node::Target(_, hir::Place::Local(l)) => {
            named(&f.locals[l.0 as usize].name, &f.locals[l.0 as usize].ty)
        }
        Node::Target(_, hir::Place::Global(g)) => named(
            &prog.globals[g.0 as usize].name,
            &prog.globals[g.0 as usize].ty,
        ),
        Node::Target(..) => return None,
    };
    Some(json!({ "contents": { "kind": "markdown", "value": format!("```novo\n{text}\n```") } }))
}

fn definition(doc: &Document, params: &Json) -> Option<Json> {
    let (analysis, f, node) = node_at(doc, params)?;
    let prog = &analysis.program;
    let span = match node {
        Node::Expr(e) => match &e.kind {
            hir::ExprKind::Local(l) => f.locals[l.0 as usize].span,
            hir::ExprKind::Global(g) => prog.globals[g.0 as usize].span,
            hir::ExprKind::Func { func, .. }
            | hir::ExprKind::CallFunc { func, .. }
            | hir::ExprKind::Closure { func, .. } => prog.funcs[func.0 as usize].span,
            hir::ExprKind::New { class, .. } => prog.classes[class.0 as usize].span,
            _ => return None,
        },
        Node::Target(_, hir::Place::Local(l)) => f.locals[l.0 as usize].span,
        Node::Target(_, hir::Place::Global(g)) => prog.globals[g.0 as usize].span,
        Node::Target(..) => return None,
    };
    let file = analysis.sources.files().nth(span.file.0 as usize)?;
    if span == Span::default() || file.path.to_string_lossy().starts_with('<') {
        return None;
    }
    let uri = if span.file == analysis.file {
        str_at(params, &["textDocument", "uri"])
    } else {
        path_to_uri(&file.path)
    };
    Some(json!({ "uri": uri, "range": range(&analysis.sources, span) }))
}

/// Something a name can refer to. Locals are known by where they are
/// declared, which captured copies in closures share.
#[derive(Clone, Copy, PartialEq)]
enum Symbol {
    Local(Span),
    Global(hir::GlobalId),
    Func(hir::FuncId),
    Class(ClassId),
}

fn cursor(doc: &Document, params: &Json) -> Option<u32> {
    let pos = params.get("position")?;
    let line = pos.get("line")?.as_u64()? as usize;
    let character = pos.get("character")?.as_u64()? as usize;
    Some(offset(&doc.text, line, character) as u32)
}

fn contains(span: Span, file: FileId, at: u32) -> bool {
    span.file == file && span.lo <= at && at <= span.hi
}

/// The symbol under the cursor, from a use or from its definition.
fn symbol_at<'a>(doc: &'a Document, params: &Json) -> Option<(&'a Analysis, Symbol)> {
    let analysis = doc.analysis.as_ref()?;
    let prog = &analysis.program;
    let at = cursor(doc, params)?;
    let file = analysis.file;
    if let Some((_, f, node)) = node_at(doc, params) {
        let symbol = match node {
            Node::Expr(e) => match &e.kind {
                hir::ExprKind::Local(l) => Some(Symbol::Local(f.locals[l.0 as usize].span)),
                hir::ExprKind::Global(g) => Some(Symbol::Global(*g)),
                hir::ExprKind::Func { func, .. } | hir::ExprKind::CallFunc { func, .. } => {
                    Some(Symbol::Func(*func))
                }
                hir::ExprKind::New { class, .. } => Some(Symbol::Class(*class)),
                _ => None,
            },
            Node::Target(_, hir::Place::Local(l)) => {
                Some(Symbol::Local(f.locals[l.0 as usize].span))
            }
            Node::Target(_, hir::Place::Global(g)) => Some(Symbol::Global(*g)),
            Node::Target(..) => None,
        };
        if let Some(s) = symbol.filter(|s| *s != Symbol::Local(Span::default())) {
            return Some((analysis, s));
        }
    }
    for (i, f) in prog.funcs.iter().enumerate() {
        if contains(f.span, file, at) {
            return Some((analysis, Symbol::Func(hir::FuncId(i as u32))));
        }
        if let Some(l) = f.locals.iter().find(|l| contains(l.span, file, at)) {
            return Some((analysis, Symbol::Local(l.span)));
        }
    }
    for (i, c) in prog.classes.iter().enumerate() {
        if contains(c.span, file, at) {
            return Some((analysis, Symbol::Class(ClassId(i as u32))));
        }
    }
    for (i, g) in prog.globals.iter().enumerate() {
        if contains(g.span, file, at) {
            return Some((analysis, Symbol::Global(hir::GlobalId(i as u32))));
        }
    }
    None
}

/// The name a symbol is written with and where it is defined.
fn symbol_name(prog: &hir::Program, symbol: Symbol) -> (String, Span) {
    match symbol {
        Symbol::Local(span) => {
            let name = prog
                .funcs
                .iter()
                .flat_map(|f| &f.locals)
                .find(|l| l.span == span)
                .map(|l| l.name.clone())
                .unwrap_or_default();
            (name, span)
        }
        Symbol::Global(g) => (
            prog.globals[g.0 as usize].name.clone(),
            prog.globals[g.0 as usize].span,
        ),
        Symbol::Func(f) => {
            let func = &prog.funcs[f.0 as usize];
            let name = func.name.rsplit('.').next().unwrap_or(&func.name);
            (name.to_string(), func.span)
        }
        Symbol::Class(c) => (
            prog.classes[c.0 as usize].name.clone(),
            prog.classes[c.0 as usize].span,
        ),
    }
}

/// The first place `name` appears as a whole word inside `span`.
fn word_in(sources: &SourceMap, span: Span, name: &str) -> Option<Span> {
    let text = &sources.files().nth(span.file.0 as usize)?.text;
    let hay = text.get(span.lo as usize..(span.hi as usize).min(text.len()))?;
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut from = 0;
    while let Some(i) = hay[from..].find(name) {
        let lo = from + i;
        let hi = lo + name.len();
        let before = hay[..lo].chars().next_back();
        let after = hay[hi..].chars().next();
        if !before.is_some_and(is_word) && !after.is_some_and(is_word) {
            return Some(Span {
                file: span.file,
                lo: span.lo + lo as u32,
                hi: span.lo + hi as u32,
            });
        }
        from = hi;
    }
    None
}

/// Every place the symbol is written, its definition included.
fn occurrences(analysis: &Analysis, symbol: Symbol) -> Vec<Span> {
    let prog = &analysis.program;
    let (name, def) = symbol_name(prog, symbol);
    let mut spans = vec![def];
    for f in &prog.funcs {
        walk_block(&f.body, &mut |node| {
            let (span, hit) = match node {
                Node::Expr(e) => (
                    e.span,
                    match &e.kind {
                        hir::ExprKind::Local(l) => {
                            symbol == Symbol::Local(f.locals[l.0 as usize].span)
                        }
                        hir::ExprKind::Global(g) => symbol == Symbol::Global(*g),
                        hir::ExprKind::Func { func, .. } | hir::ExprKind::CallFunc { func, .. } => {
                            symbol == Symbol::Func(*func)
                        }
                        hir::ExprKind::New { class, .. } => symbol == Symbol::Class(*class),
                        _ => false,
                    },
                ),
                Node::Target(span, hir::Place::Local(l)) => {
                    (span, symbol == Symbol::Local(f.locals[l.0 as usize].span))
                }
                Node::Target(span, hir::Place::Global(g)) => (span, symbol == Symbol::Global(*g)),
                Node::Target(..) => return,
            };
            if hit && let Some(s) = word_in(&analysis.sources, span, &name) {
                spans.push(s);
            }
        });
    }
    if let Symbol::Class(c) = symbol {
        // Classes also appear in type annotations, which the program does not keep.
        let variant = prog.classes[c.0 as usize].variant;
        for (i, file) in analysis.sources.files().enumerate() {
            let Ok(tokens) = crate::syntax::lexer::tokenize(FileId(i as u32), &file.text) else {
                continue;
            };
            for (k, t) in tokens.iter().enumerate() {
                let after_dot = k > 0 && tokens[k - 1].tok == Tok::Dot;
                if t.tok == Tok::Ident(name.clone()) && (variant || !after_dot) {
                    spans.push(t.span);
                }
            }
        }
    }
    spans.retain(|s| *s != Span::default());
    spans.sort_by_key(|s| (s.file.0, s.lo));
    spans.dedup();
    spans
}

fn location(analysis: &Analysis, uri: &str, span: Span) -> Option<(String, Json)> {
    let file = analysis.sources.files().nth(span.file.0 as usize)?;
    if file.path.to_string_lossy().starts_with('<') {
        return None;
    }
    let uri = if span.file == analysis.file {
        uri.to_string()
    } else {
        path_to_uri(&file.path)
    };
    Some((uri, range(&analysis.sources, span)))
}

fn references(doc: &Document, params: &Json) -> Option<Json> {
    let (analysis, symbol) = symbol_at(doc, params)?;
    let uri = str_at(params, &["textDocument", "uri"]);
    let locations: Vec<Json> = occurrences(analysis, symbol)
        .into_iter()
        .filter_map(|s| location(analysis, &uri, s))
        .map(|(uri, range)| json!({ "uri": uri, "range": range }))
        .collect();
    Some(Json::Array(locations))
}

fn rename(doc: &Document, params: &Json) -> Option<Json> {
    let new_name = params.get("newName")?.as_str()?;
    let valid = new_name
        .chars()
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_')
        && new_name.chars().all(|c| c.is_alphanumeric() || c == '_')
        && Tok::keyword(new_name).is_none();
    if !valid {
        return None;
    }
    let (analysis, symbol) = symbol_at(doc, params)?;
    let uri = str_at(params, &["textDocument", "uri"]);
    let mut changes: HashMap<String, Vec<Json>> = HashMap::new();
    for span in occurrences(analysis, symbol) {
        // Names defined in the standard library cannot be renamed.
        let (target, range) = location(analysis, &uri, span)?;
        changes
            .entry(target)
            .or_default()
            .push(json!({ "range": range, "newText": new_name }));
    }
    Some(json!({ "changes": changes }))
}

const KEYWORDS: &[&str] = &[
    "show",
    "if",
    "otherwise",
    "repeat",
    "for",
    "in",
    "while",
    "function",
    "task",
    "return",
    "true",
    "false",
    "none",
    "and",
    "or",
    "not",
    "use",
    "python",
    "as",
    "class",
    "try",
    "catch",
    "break",
    "continue",
    "pass",
    "together",
    "await",
    "match",
    "enum",
    "interface",
    "where",
    "select",
    "times",
];

const STD_MODULES: &[&str] = &[
    "math",
    "time",
    "json",
    "fs",
    "env",
    "nn",
    "random",
    "csv",
    "tokenizer",
    "layers",
    "llama",
];

/// Completion kinds from the LSP specification.
const METHOD: u32 = 2;
const FUNCTION: u32 = 3;
const FIELD: u32 = 5;
const VARIABLE: u32 = 6;
const CLASS: u32 = 7;
const MODULE: u32 = 9;
const KEYWORD: u32 = 14;

fn completion(doc: &Document, params: &Json) -> Json {
    let Some(at) = cursor(doc, params) else {
        return json!([]);
    };
    let line_start = doc.text[..at as usize].rfind('\n').map_or(0, |i| i + 1);
    let before = &doc.text[line_start..at as usize];
    let word_start = before
        .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
        .map_or(0, |i| i + 1);
    let mut items: Vec<(String, u32, String)> = Vec::new();
    if let Some(receiver) = before[..word_start].strip_suffix('.') {
        let start = receiver
            .rfind(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.'))
            .map_or(0, |i| i + 1);
        members(doc, at, &receiver[start..], &mut items);
    } else {
        names(doc, at, &mut items);
    }
    let mut seen = HashSet::new();
    let items: Vec<Json> = items
        .into_iter()
        .filter(|(label, ..)| seen.insert(label.clone()))
        .map(|(label, kind, detail)| json!({ "label": label, "kind": kind, "detail": detail }))
        .collect();
    Json::Array(items)
}

/// The function whose definition most closely precedes `at` in this file.
fn enclosing(analysis: &Analysis, at: u32) -> Option<&hir::Func> {
    analysis
        .program
        .funcs
        .iter()
        .filter(|f| f.span.file == analysis.file && f.span.lo <= at)
        .max_by_key(|f| f.span.lo)
}

fn names(doc: &Document, at: u32, items: &mut Vec<(String, u32, String)>) {
    if let Some(analysis) = &doc.analysis {
        let prog = &analysis.program;
        let classes: Vec<String> = prog.classes.iter().map(|c| c.name.clone()).collect();
        if let Some(f) = enclosing(analysis, at) {
            for l in f.locals.iter().filter(|l| l.span != Span::default()) {
                items.push((l.name.clone(), VARIABLE, l.ty.display(&classes).to_string()));
            }
        }
        for g in &prog.globals {
            if g.span.file == analysis.file {
                items.push((g.name.clone(), VARIABLE, g.ty.display(&classes).to_string()));
            }
        }
        for f in &prog.funcs {
            if f.span.file == analysis.file && !f.name.contains(['.', '<']) {
                items.push((f.name.clone(), FUNCTION, f.ty.display(&classes).to_string()));
            }
        }
        for c in &prog.classes {
            if c.span.file == analysis.file && !c.variant {
                items.push((c.name.clone(), CLASS, String::new()));
            }
        }
    }
    for b in Builtin::ALL {
        if !b.name().contains('.') {
            items.push((b.name().to_string(), FUNCTION, "builtin".into()));
        }
    }
    for m in STD_MODULES {
        items.push((m.to_string(), MODULE, "module".into()));
    }
    for k in KEYWORDS {
        items.push((k.to_string(), KEYWORD, String::new()));
    }
}

fn members(doc: &Document, at: u32, receiver: &str, items: &mut Vec<(String, u32, String)>) {
    let mut parts = receiver.split('.');
    let first = parts.next().unwrap_or_default();
    if StdModule::from_name(first).is_some() {
        let prefix = format!("{first}.");
        for b in Builtin::ALL {
            if let Some(member) = b.name().strip_prefix(&prefix) {
                items.push((member.to_string(), FUNCTION, b.name().to_string()));
            }
        }
        return;
    }
    let Some(analysis) = &doc.analysis else {
        return;
    };
    let prog = &analysis.program;
    let local = enclosing(analysis, at)
        .and_then(|f| f.locals.iter().rev().find(|l| l.name == first))
        .map(|l| l.ty.clone());
    let global = || {
        prog.globals
            .iter()
            .find(|g| g.name == first && g.span.file == analysis.file)
            .map(|g| g.ty.clone())
    };
    let Some(mut ty) = local.or_else(global) else {
        return;
    };
    for field in parts {
        let Ty::Class(c) = ty else { return };
        let Some(f) = prog.classes[c.0 as usize]
            .fields
            .iter()
            .find(|f| f.name == field)
        else {
            return;
        };
        ty = f.ty.clone();
    }
    let classes: Vec<String> = prog.classes.iter().map(|c| c.name.clone()).collect();
    match &ty {
        Ty::Class(c) => {
            let class = &prog.classes[c.0 as usize];
            for f in &class.fields {
                items.push((f.name.clone(), FIELD, f.ty.display(&classes).to_string()));
            }
            for (name, func) in &class.methods {
                let ty = prog.funcs[func.0 as usize].ty.display(&classes).to_string();
                items.push((name.clone(), METHOD, ty));
            }
        }
        _ => {
            for name in METHOD_NAMES {
                if method_signature(&ty, name).is_some() {
                    items.push((name.to_string(), METHOD, String::new()));
                }
            }
        }
    }
}

enum Node<'a> {
    Expr(&'a hir::Expr),
    /// An assignment target, with the span of the name being assigned.
    Target(Span, &'a hir::Place),
}

fn walk_block<'a>(block: &'a hir::Block, f: &mut impl FnMut(Node<'a>)) {
    for s in block {
        walk_stmt(s, f);
    }
}

fn walk_place<'a>(place: &'a hir::Place, name: Span, f: &mut impl FnMut(Node<'a>)) {
    match place {
        hir::Place::Local(_) | hir::Place::Global(_) => f(Node::Target(name, place)),
        hir::Place::Field { target, .. } => walk_expr(target, f),
        hir::Place::Index { target, index } => {
            walk_expr(target, f);
            walk_expr(index, f);
        }
    }
}

fn walk_stmt<'a>(s: &'a hir::Stmt, f: &mut impl FnMut(Node<'a>)) {
    use hir::StmtKind::*;
    match &s.kind {
        Expr(e) => walk_expr(e, f),
        Assign { place, value, .. } => {
            let name = Span {
                file: s.span.file,
                lo: s.span.lo,
                hi: value.span.lo.max(s.span.lo),
            };
            walk_place(place, name, f);
            walk_expr(value, f);
        }
        Show(es) => es.iter().for_each(|e| walk_expr(e, f)),
        If {
            branches,
            otherwise,
        } => {
            for (c, b) in branches {
                walk_expr(c, f);
                walk_block(b, f);
            }
            walk_block(otherwise, f);
        }
        While { cond, body } => {
            walk_expr(cond, f);
            walk_block(body, f);
        }
        For { var, iter, body } => {
            walk_place(
                var,
                Span {
                    file: s.span.file,
                    lo: s.span.lo,
                    hi: iter.span.lo.max(s.span.lo),
                },
                f,
            );
            walk_expr(iter, f);
            walk_block(body, f);
        }
        Repeat { count, body, .. } => {
            walk_expr(count, f);
            walk_block(body, f);
        }
        Return(Some(e)) => walk_expr(e, f),
        Try { body, catches } => {
            walk_block(body, f);
            for c in catches {
                walk_block(&c.body, f);
            }
        }
        Return(None) | Break | Continue | InitModule(_) => {}
    }
}

fn walk_exprs<'a>(es: &'a [hir::Expr], f: &mut impl FnMut(Node<'a>)) {
    for e in es {
        walk_expr(e, f);
    }
}

fn walk_expr<'a>(e: &'a hir::Expr, f: &mut impl FnMut(Node<'a>)) {
    use hir::ExprKind::*;
    f(Node::Expr(e));
    match &e.kind {
        FStr(parts) => {
            for p in parts {
                if let hir::FStrPart::Expr { expr, .. } = p {
                    walk_expr(expr, f);
                }
            }
        }
        List(es) | Tuple(es) | RunParallel(es) => walk_exprs(es, f),
        Closure { captures: es, .. }
        | CallFunc { args: es, .. }
        | CallBuiltin { args: es, .. }
        | New { args: es, .. } => walk_exprs(es, f),
        Map(entries) => {
            for (k, v) in entries {
                walk_expr(k, f);
                walk_expr(v, f);
            }
        }
        Unary { operand: x, .. }
        | Field { target: x, .. }
        | Together(x)
        | Await(x)
        | IntToFloat(x) => walk_expr(x, f),
        Binary { lhs, rhs, .. } => {
            walk_expr(lhs, f);
            walk_expr(rhs, f);
        }
        IfElse {
            cond,
            then,
            otherwise,
        } => {
            walk_expr(cond, f);
            walk_expr(then, f);
            walk_expr(otherwise, f);
        }
        Call { callee, args } => {
            walk_expr(callee, f);
            walk_exprs(args, f);
        }
        MethodCall { recv, args, .. } => {
            walk_expr(recv, f);
            walk_exprs(args, f);
        }
        Index { target, index } => {
            walk_expr(target, f);
            walk_expr(index, f);
        }
        Slice { target, start, end } => {
            walk_expr(target, f);
            start.iter().chain(end).for_each(|x| walk_expr(x, f));
        }
        Query { target, body, .. } => {
            walk_expr(target, f);
            body.iter().for_each(|x| walk_expr(x, f));
        }
        IsVariant { value, .. } => walk_expr(value, f),
        Int(_)
        | Float(_)
        | Bool(_)
        | Str(_)
        | None
        | Local(_)
        | Global(_)
        | Func { .. }
        | Recurse
        | PyImport(_) => {}
    }
}

fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let mut bytes = Vec::with_capacity(rest.len());
    let raw = rest.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%'
            && let Some(b) = rest
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            bytes.push(b);
            i += 3;
        } else {
            bytes.push(raw[i]);
            i += 1;
        }
    }
    let mut path = String::from_utf8(bytes).ok()?;
    if cfg!(windows) && path.starts_with('/') && path.get(2..3) == Some(":") {
        path.remove(0);
    }
    Some(PathBuf::from(path))
}

fn path_to_uri(path: &Path) -> String {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let text = path.to_string_lossy();
    let text = text
        .strip_prefix(r"\\?\")
        .unwrap_or(&text)
        .replace('\\', "/");
    let mut uri = String::from(if text.starts_with('/') {
        "file://"
    } else {
        "file:///"
    });
    for b in text.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                uri.push(b as char)
            }
            _ => uri.push_str(&format!("%{b:02X}")),
        }
    }
    uri
}
