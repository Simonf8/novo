//! Static checking: resolves every name, infers every type, and lowers the
//! surface AST into the typed HIR that engines execute.

pub mod builtins;
mod expr;
pub mod hir;
pub mod session;
mod shape;
mod solve;
mod stmt;
pub mod types;

use crate::diagnostic::Diagnostic;
use crate::source::{SourceMap, Span};
use crate::syntax::{self, ast};
use builtins::StdModule;
use hir::{FuncId, GlobalId, LocalId, ModuleId};
use solve::Constraint;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use types::{ClassId, Infer, Ty};

pub fn check(sources: &mut SourceMap, entry: &Path) -> Result<hir::Program, Vec<Diagnostic>> {
    let mut checker = Checker::new(std::mem::take(sources));
    let result = match checker.load_module(entry, None) {
        Some(id) => checker.finish(id),
        None => Err(std::mem::take(&mut checker.diags)),
    };
    *sources = checker.sources;
    result
}

/// Checks `entry` for an editor, reading `text` in place of the file on disk.
/// Returns whatever could be checked alongside the errors.
pub fn analyze(
    sources: &mut SourceMap,
    entry: &Path,
    text: String,
) -> (Option<hir::Program>, Vec<Diagnostic>) {
    let mut checker = Checker::new(std::mem::take(sources));
    checker.overlay = Some((
        std::fs::canonicalize(entry).unwrap_or_else(|_| entry.to_path_buf()),
        text,
    ));
    let module = checker.load_module(entry, None);
    checker.solve_deferred();
    let program = match module {
        Some(id) if checker.funcs.iter().all(|f| f.hir.is_some()) => Some(checker.snapshot(id)),
        _ => None,
    };
    *sources = std::mem::take(&mut checker.sources);
    (program, checker.diags)
}

/// Standard modules written in Novo, compiled into the executable.
const SOURCE_MODULES: &[(&str, &str)] = &[("layers", include_str!("../stdlib/layers.nv"))];

fn source_module_path(name: &str) -> Option<PathBuf> {
    SOURCE_MODULES
        .iter()
        .any(|(n, _)| *n == name)
        .then(|| PathBuf::from(format!("<std>/{name}.nv")))
}

fn varied_names(body: &[ast::Stmt]) -> HashSet<String> {
    let mut names = HashMap::new();
    let mut returns = 0;
    stmt::count_assignments(body, &mut names, &mut returns);
    let mut varied: HashSet<String> = names
        .into_iter()
        .filter(|(_, n)| *n > 1)
        .map(|(name, _)| name)
        .collect();
    if returns > 1 {
        varied.insert("return".into());
    }
    varied
}

#[derive(Debug, Clone, Copy)]
enum Item {
    Global(GlobalId),
    Func(FuncId),
    Class(ClassId),
    Module(ModuleId),
    Std(StdModule),
}

struct ModuleSlot {
    path: PathBuf,
    names: HashMap<String, Item>,
    init: Option<FuncId>,
    loaded: bool,
}

#[derive(Clone)]
struct Scheme {
    vars: Vec<u32>,
    ty: Ty,
    constraints: Vec<Constraint>,
}

enum FnState {
    Pending,
    InProgress(Ty),
    Done(Scheme),
}

struct Param {
    name: String,
    default: Option<hir::Expr>,
}

struct FuncSlot {
    name: String,
    module: ModuleId,
    class: Option<ClassId>,
    def: Option<Rc<ast::FunctionDef>>,
    state: FnState,
    params: Vec<Param>,
    hir: Option<hir::Func>,
}

struct ClassSlot {
    name: String,
    span: Span,
    fields: Vec<FieldSlot>,
    methods: Vec<(String, FuncId)>,
}

struct FieldSlot {
    name: String,
    ty: Ty,
    default: Option<hir::Expr>,
}

#[derive(Default)]
struct FnCtx {
    module: u32,
    locals: Vec<hir::Local>,
    names: HashMap<String, LocalId>,
    scopes: Vec<(String, LocalId)>,
    captures: Vec<(LocalId, LocalId)>,
    is_closure: bool,
    is_toplevel: bool,
    is_const: bool,
    self_name: Option<(String, Ty)>,
    ret: Option<Ty>,
    loop_depth: u32,
    depth: u32,
    pending: Vec<Constraint>,
    /// Names assigned in more than one place, and `return` if the function has
    /// more than one; these do not keep tensor shapes.
    varied: HashSet<String>,
}

impl FnCtx {
    fn add_local(&mut self, name: &str, ty: Ty, span: Span) -> LocalId {
        self.locals.push(hir::Local {
            name: name.to_string(),
            ty,
            span,
        });
        LocalId(self.locals.len() as u32 - 1)
    }
}

enum Resolved {
    Local(LocalId),
    Global(GlobalId),
    Func(FuncId),
    Class(ClassId),
    Module(ModuleId),
    Std(StdModule),
    Builtin(builtins::Builtin),
    Recurse(Ty),
}

struct Checker {
    sources: SourceMap,
    /// In an interactive session names may be redefined.
    redefine: bool,
    /// Text to use for one file instead of what is on disk, as an editor has it.
    overlay: Option<(PathBuf, String)>,
    /// Named tensor dimensions such as `B` in `Tensor[B, 784]`, one scope per function.
    dim_names: Vec<HashMap<String, Ty>>,
    infer: Infer,
    diags: Vec<Diagnostic>,
    funcs: Vec<FuncSlot>,
    classes: Vec<ClassSlot>,
    globals: Vec<hir::Global>,
    modules: Vec<ModuleSlot>,
    stack: Vec<FnCtx>,
    deferred: Vec<Constraint>,
}

impl Checker {
    fn new(sources: SourceMap) -> Self {
        Checker {
            sources,
            redefine: false,
            overlay: None,
            dim_names: Vec::new(),
            infer: Infer::default(),
            diags: Vec::new(),
            funcs: Vec::new(),
            classes: Vec::new(),
            globals: Vec::new(),
            modules: Vec::new(),
            stack: Vec::new(),
            deferred: Vec::new(),
        }
    }

    fn error(&mut self, diag: Diagnostic) {
        self.diags.push(diag);
    }

    fn class_names(&self) -> Vec<String> {
        self.classes.iter().map(|c| c.name.clone()).collect()
    }

    fn show(&self, ty: &Ty) -> String {
        let ty = self.infer.zonk(ty);
        ty.display(&self.class_names()).to_string()
    }

    fn ctx(&mut self) -> &mut FnCtx {
        self.stack.last_mut().expect("inside a function")
    }

    fn module_of_ctx(&self) -> ModuleId {
        ModuleId(self.stack.last().expect("inside a function").module)
    }

    fn mismatch(&mut self, span: Span, expected: &Ty, found: &Ty) {
        let msg = format!(
            "expected `{}`, found `{}`",
            self.show(expected),
            self.show(found)
        );
        self.error(Diagnostic::error(span, "mismatched types").with_label(msg));
    }

    fn unify_at(&mut self, span: Span, expected: &Ty, found: &Ty) -> bool {
        if self.infer.unify(expected, found) {
            return true;
        }
        self.mismatch(span, expected, found);
        false
    }

    /// Checks that `expr` fits `expected`, widening an `Int` to a `Float` when needed.
    fn coerce(&mut self, expr: hir::Expr, expected: &Ty) -> hir::Expr {
        let found = self.infer.shallow(&expr.ty);
        if found == Ty::Int && self.infer.shallow(expected) == Ty::Float {
            let span = expr.span;
            return hir::Expr {
                kind: hir::ExprKind::IntToFloat(Box::new(expr)),
                ty: Ty::Float,
                span,
            };
        }
        self.unify_at(expr.span, expected, &found);
        expr
    }

    // ---- modules -------------------------------------------------------------------------

    fn load_module(&mut self, display: &Path, from: Option<Span>) -> Option<ModuleId> {
        let path = std::fs::canonicalize(display).unwrap_or_else(|_| display.to_path_buf());
        if let Some(i) = self.modules.iter().position(|m| m.path == path) {
            if !self.modules[i].loaded {
                let span = from.unwrap_or_default();
                self.error(
                    Diagnostic::error(span, "circular module import")
                        .with_help("move the shared code into a third module"),
                );
                return None;
            }
            return Some(ModuleId(i as u32));
        }
        let overlay = self
            .overlay
            .as_ref()
            .filter(|(p, _)| *p == path)
            .map(|(_, t)| Ok(t.clone()));
        let builtin = SOURCE_MODULES
            .iter()
            .find(|(n, _)| source_module_path(n).as_deref() == Some(path.as_path()))
            .map(|(_, t)| Ok(t.to_string()));
        let text = match overlay
            .or(builtin)
            .unwrap_or_else(|| std::fs::read_to_string(&path))
        {
            Ok(t) => t,
            Err(e) => {
                let msg = format!("cannot read `{}`: {e}", path.display());
                match from {
                    Some(span) => self.error(Diagnostic::error(span, msg)),
                    None => self.diags.push(Diagnostic::error(Span::default(), msg)),
                }
                return None;
            }
        };
        let file = self.sources.add(display, text);
        let module = match syntax::parse(file, &self.sources.get(file).text.clone()) {
            Ok(m) => m,
            Err(d) => {
                self.error(d);
                return None;
            }
        };
        let id = ModuleId(self.modules.len() as u32);
        self.modules.push(ModuleSlot {
            path: path.clone(),
            names: HashMap::new(),
            init: None,
            loaded: false,
        });
        self.declare_module(id, &module, display);
        self.check_module(id, module);
        self.modules[id.0 as usize].loaded = true;
        Some(id)
    }

    fn define(&mut self, module: ModuleId, name: &ast::Ident, item: Item) {
        let names = &mut self.modules[module.0 as usize].names;
        if let Some(existing) = names.get(&name.name)
            && !self.redefine
            && !matches!((existing, item), (Item::Global(_), Item::Global(_)))
        {
            let msg = format!("`{}` is defined more than once", name.name);
            self.error(Diagnostic::error(name.span, msg));
            return;
        }
        names.insert(name.name.clone(), item);
    }

    fn declare_module(&mut self, id: ModuleId, module: &ast::Module, path: &Path) {
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let mut classes = Vec::new();
        for stmt in &module.stmts {
            match &stmt.kind {
                ast::StmtKind::Function(def) => {
                    let fid = self.add_func(&def.name.name, id, None, Some(Rc::new(def.clone())));
                    self.define(id, &def.name, Item::Func(fid));
                }
                ast::StmtKind::Class(def) => {
                    let cid = ClassId(self.classes.len() as u32);
                    self.classes.push(ClassSlot {
                        name: def.name.name.clone(),
                        span: def.name.span,
                        fields: Vec::new(),
                        methods: Vec::new(),
                    });
                    self.define(id, &def.name, Item::Class(cid));
                    classes.push((cid, def.clone()));
                }
                ast::StmtKind::Use(ast::Use::File { path: rel, alias }) => {
                    if let Some(mid) = self.load_module(&dir.join(rel), Some(stmt.span)) {
                        let name = match alias {
                            Some(a) => a.clone(),
                            None => {
                                let stem = Path::new(rel)
                                    .file_stem()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or(rel)
                                    .to_string();
                                ast::Ident {
                                    name: stem,
                                    span: stmt.span,
                                }
                            }
                        };
                        self.define(id, &name, Item::Module(mid));
                    }
                }
                ast::StmtKind::Use(ast::Use::Module { name, alias }) => {
                    match StdModule::from_name(&name.name) {
                        Some(m) => self.define(id, alias.as_ref().unwrap_or(name), Item::Std(m)),
                        None if source_module_path(&name.name).is_some() => {
                            let path = source_module_path(&name.name).expect("checked above");
                            if let Some(mid) = self.load_module(&path, Some(stmt.span)) {
                                self.define(id, alias.as_ref().unwrap_or(name), Item::Module(mid));
                            }
                        }
                        None => {
                            let msg = format!("unknown module `{}`", name.name);
                            let help = format!(
                                "standard modules are math, time, json, csv, fs, env, random, nn and layers; to load a file write `use \"{}.nv\"`",
                                name.name
                            );
                            self.error(Diagnostic::error(name.span, msg).with_help(help));
                        }
                    }
                }
                _ => {}
            }
        }
        let mut assigned = Vec::new();
        stmt::assigned_names(&module.stmts, &mut assigned);
        for name in assigned {
            if self.modules[id.0 as usize].names.contains_key(&name.name) {
                if !matches!(
                    self.modules[id.0 as usize].names[&name.name],
                    Item::Global(_)
                ) {
                    let msg = format!(
                        "cannot assign to `{}` because it names a function, class or module",
                        name.name
                    );
                    self.error(Diagnostic::error(name.span, msg));
                }
                continue;
            }
            let ty = self.infer.fresh();
            let gid = GlobalId(self.globals.len() as u32);
            self.globals.push(hir::Global {
                name: name.name.clone(),
                ty,
                span: name.span,
            });
            self.define(id, &name, Item::Global(gid));
        }
        for (cid, def) in classes {
            self.declare_class(id, cid, &def);
        }
    }

    fn add_func(
        &mut self,
        name: &str,
        module: ModuleId,
        class: Option<ClassId>,
        def: Option<Rc<ast::FunctionDef>>,
    ) -> FuncId {
        let id = FuncId(self.funcs.len() as u32);
        self.funcs.push(FuncSlot {
            name: name.to_string(),
            module,
            class,
            def,
            state: FnState::Pending,
            params: Vec::new(),
            hir: None,
        });
        id
    }

    fn declare_class(&mut self, module: ModuleId, cid: ClassId, def: &ast::ClassDef) {
        self.stack.push(FnCtx {
            module: module.0,
            is_toplevel: true,
            is_const: true,
            ..FnCtx::default()
        });
        let mut fields: Vec<FieldSlot> = Vec::new();
        for field in &def.fields {
            if fields.iter().any(|f| f.name == field.name.name) {
                self.error(Diagnostic::error(
                    field.name.span,
                    format!("field `{}` is declared twice", field.name.name),
                ));
                continue;
            }
            let ty = match &field.ty {
                Some(t) => self.resolve_type(t),
                None => self.infer.fresh(),
            };
            let default = field.default.as_ref().map(|e| {
                let value = self.infer_expr(e);
                let value = self.unshape(value);
                self.coerce(value, &ty)
            });
            fields.push(FieldSlot {
                name: field.name.name.clone(),
                ty,
                default,
            });
        }
        self.end_const_ctx(def.name.span);
        let mut methods = Vec::new();
        for m in &def.methods {
            if fields.iter().any(|f| f.name == m.name.name)
                || methods
                    .iter()
                    .any(|(n, _): &(String, FuncId)| *n == m.name.name)
            {
                self.error(Diagnostic::error(
                    m.name.span,
                    format!("`{}` is defined more than once in this class", m.name.name),
                ));
                continue;
            }
            let name = format!("{}.{}", def.name.name, m.name.name);
            let fid = self.add_func(&name, module, Some(cid), Some(Rc::new(m.clone())));
            methods.push((m.name.name.clone(), fid));
        }
        let class = &mut self.classes[cid.0 as usize];
        class.fields = fields;
        class.methods = methods;
    }

    /// Pops a context used for default values, which must not need their own locals.
    fn end_const_ctx(&mut self, span: Span) {
        let ctx = self.stack.pop().expect("const context");
        if !ctx.locals.is_empty() {
            self.error(
                Diagnostic::error(span, "default values must be simple expressions")
                    .with_help("compute the value inside the function instead"),
            );
        }
        self.deferred.extend(ctx.pending);
    }

    fn check_module(&mut self, id: ModuleId, module: ast::Module) {
        let init = self.check_top_level(id, &module.stmts);
        self.modules[id.0 as usize].init = Some(init);
    }

    /// Checks a module's top-level statements as the body of a new function,
    /// then checks any of the module's functions not yet inferred.
    fn check_top_level(&mut self, id: ModuleId, stmts: &[ast::Stmt]) -> FuncId {
        let init = self.add_func("<module>", id, None, None);
        self.stack.push(FnCtx {
            module: id.0,
            is_toplevel: true,
            varied: varied_names(stmts),
            ..FnCtx::default()
        });
        self.dim_names.push(HashMap::new());
        let body = self.check_block(stmts);
        self.dim_names.pop();
        self.solve_pending();
        let ctx = self.stack.pop().unwrap();
        self.deferred.extend(ctx.pending);
        self.funcs[init.0 as usize].hir = Some(hir::Func {
            name: "<module>".into(),
            generics: Vec::new(),
            params: 0,
            defaults: Vec::new(),
            captures: Vec::new(),
            locals: ctx.locals,
            body,
            ty: Ty::func(vec![], Ty::None),
            span: Span::default(),
        });
        self.funcs[init.0 as usize].state = FnState::Done(Scheme {
            vars: vec![],
            ty: Ty::func(vec![], Ty::None),
            constraints: vec![],
        });

        let pending: Vec<FuncId> = (0..self.funcs.len() as u32)
            .map(FuncId)
            .filter(|f| {
                self.funcs[f.0 as usize].module == id
                    && matches!(self.funcs[f.0 as usize].state, FnState::Pending)
            })
            .collect();
        for f in pending {
            self.func_scheme(f);
        }
        init
    }

    // ---- functions -----------------------------------------------------------------------

    /// Returns the function's type scheme, inferring the function first if needed.
    fn func_scheme(&mut self, id: FuncId) -> Scheme {
        match &self.funcs[id.0 as usize].state {
            FnState::Done(s) => return s.clone(),
            FnState::InProgress(ty) => {
                return Scheme {
                    vars: vec![],
                    ty: ty.clone(),
                    constraints: vec![],
                };
            }
            FnState::Pending => {}
        }
        let def = self.funcs[id.0 as usize]
            .def
            .clone()
            .expect("function source");
        let module = self.funcs[id.0 as usize].module;
        let class = self.funcs[id.0 as usize].class;
        let mut ctx = FnCtx {
            module: module.0,
            ..FnCtx::default()
        };

        self.dim_names.push(HashMap::new());
        let mut param_tys = Vec::new();
        if let Some(cid) = class {
            let ty = Ty::Class(cid);
            let local = ctx.add_local("self", ty.clone(), def.name.span);
            ctx.names.insert("self".into(), local);
            param_tys.push(ty);
        }
        self.stack.push(FnCtx {
            module: module.0,
            is_toplevel: true,
            is_const: true,
            ..FnCtx::default()
        });
        let mut params = Vec::new();
        for p in &def.params {
            let ty = match &p.ty {
                Some(t) => self.resolve_type(t),
                None => self.infer.fresh(),
            };
            let default = p.default.as_ref().map(|e| {
                let value = self.infer_expr(e);
                self.coerce(value, &ty)
            });
            if default.is_none() && params.iter().any(|q: &Param| q.default.is_some()) {
                self.error(Diagnostic::error(
                    p.name.span,
                    "parameters without defaults must come first",
                ));
            }
            if ctx.names.contains_key(&p.name.name) {
                self.error(Diagnostic::error(
                    p.name.span,
                    format!("parameter `{}` is declared twice", p.name.name),
                ));
            }
            let local = ctx.add_local(&p.name.name, ty.clone(), p.name.span);
            ctx.names.insert(p.name.name.clone(), local);
            param_tys.push(ty);
            params.push(Param {
                name: p.name.name.clone(),
                default,
            });
        }
        let ret = match &def.ret {
            Some(t) => self.resolve_type(t),
            None => self.infer.fresh(),
        };
        self.end_const_ctx(def.name.span);
        let fty = Ty::func(param_tys, ret.clone());
        ctx.ret = Some(ret);
        self.funcs[id.0 as usize].params = params;
        self.funcs[id.0 as usize].state = FnState::InProgress(fty.clone());

        let nparams = ctx.locals.len();
        self.declare_locals(&mut ctx, &def.body);
        self.stack.push(ctx);
        let body = self.check_function_body(&def.body, def.name.span, &def.name.name);
        self.solve_pending();
        let ctx = self.stack.pop().unwrap();
        self.dim_names.pop();

        self.funcs[id.0 as usize].state = FnState::Pending;
        let scheme = self.generalize(&fty, ctx.pending);
        let defaults = self.funcs[id.0 as usize]
            .params
            .iter()
            .map(|p| p.default.clone())
            .collect();
        self.funcs[id.0 as usize].hir = Some(hir::Func {
            name: self.funcs[id.0 as usize].name.clone(),
            generics: scheme.vars.clone(),
            params: nparams,
            defaults,
            captures: Vec::new(),
            locals: ctx.locals,
            body,
            ty: fty,
            span: def.name.span,
        });
        self.funcs[id.0 as usize].state = FnState::Done(scheme.clone());
        scheme
    }

    /// Adds a local for every name the body assigns that is not a module global.
    fn declare_locals(&mut self, ctx: &mut FnCtx, body: &[ast::Stmt]) {
        ctx.varied = varied_names(body);
        let mut assigned = Vec::new();
        stmt::assigned_names(body, &mut assigned);
        let names = &self.modules[ctx.module as usize].names;
        for name in assigned {
            if ctx.names.contains_key(&name.name)
                || matches!(names.get(&name.name), Some(Item::Global(_)))
            {
                continue;
            }
            let ty = self.infer.fresh();
            let local = ctx.add_local(&name.name, ty, name.span);
            ctx.names.insert(name.name, local);
        }
    }

    fn check_function_body(
        &mut self,
        body: &[ast::Stmt],
        name_span: Span,
        name: &str,
    ) -> hir::Block {
        let block = self.check_block(body);
        if !stmt::always_returns(body) {
            let ret = self.ctx().ret.clone().unwrap();
            if !self.infer.unify(&ret, &Ty::None) {
                let msg = format!("function `{name}` can reach its end without returning a value");
                let help = format!(
                    "it returns `{}` elsewhere; add a `return` at the end",
                    self.show(&ret)
                );
                self.error(Diagnostic::error(name_span, msg).with_help(help));
            }
        }
        block
    }

    fn env_vars(&self) -> Vec<u32> {
        let mut vars = Vec::new();
        for g in &self.globals {
            self.infer.free_vars(&g.ty, &mut vars);
        }
        for f in &self.funcs {
            if let FnState::InProgress(ty) = &f.state {
                self.infer.free_vars(ty, &mut vars);
            }
        }
        for c in &self.classes {
            for f in &c.fields {
                self.infer.free_vars(&f.ty, &mut vars);
            }
        }
        for ctx in &self.stack {
            for l in &ctx.locals {
                self.infer.free_vars(&l.ty, &mut vars);
            }
        }
        for c in &self.deferred {
            for t in c.tys() {
                self.infer.free_vars(t, &mut vars);
            }
        }
        vars
    }

    fn generalize(&mut self, ty: &Ty, pending: Vec<Constraint>) -> Scheme {
        let env = self.env_vars();
        let mut vars = Vec::new();
        self.infer.free_vars(ty, &mut vars);
        for c in &pending {
            for t in c.tys() {
                self.infer.free_vars(t, &mut vars);
            }
        }
        vars.retain(|v| !env.contains(v));

        let mut kept = pending;
        loop {
            let (inside, outside): (Vec<_>, Vec<_>) = kept.into_iter().partition(|c| {
                let mut cv = Vec::new();
                for t in c.tys() {
                    self.infer.free_vars(t, &mut cv);
                }
                cv.iter().all(|v| vars.contains(v))
            });
            kept = inside;
            if outside.is_empty() {
                break;
            }
            for c in outside {
                let mut cv = Vec::new();
                for t in c.tys() {
                    self.infer.free_vars(t, &mut cv);
                }
                vars.retain(|v| !cv.contains(v));
                self.deferred.push(c);
            }
        }
        Scheme {
            vars,
            ty: self.infer.zonk(ty),
            constraints: kept,
        }
    }

    /// Instantiates a scheme with fresh variables, returning its type and the types
    /// chosen for its quantified variables.
    fn instantiate(&mut self, scheme: &Scheme) -> (Ty, Vec<Ty>) {
        if scheme.vars.is_empty() {
            return (scheme.ty.clone(), Vec::new());
        }
        let map: Vec<(u32, Ty)> = scheme
            .vars
            .iter()
            .map(|v| (*v, self.infer.fresh()))
            .collect();
        for c in &scheme.constraints {
            let c = c.substitute(&self.infer, &map);
            self.add_constraint(c);
        }
        let inst = map.iter().map(|(_, t)| t.clone()).collect();
        (self.infer.substitute(&scheme.ty, &map), inst)
    }

    // ---- names ---------------------------------------------------------------------------

    fn lookup(&mut self, name: &str) -> Option<Resolved> {
        let top = self.stack.len() - 1;
        if let Some(r) = self.lookup_in(top, name) {
            return Some(r);
        }
        let module = self.stack[top].module as usize;
        if let Some(item) = self.modules[module].names.get(name) {
            return Some(match *item {
                Item::Global(g) => Resolved::Global(g),
                Item::Func(f) => Resolved::Func(f),
                Item::Class(c) => Resolved::Class(c),
                Item::Module(m) => Resolved::Module(m),
                Item::Std(s) => Resolved::Std(s),
            });
        }
        builtins::Builtin::global(name).map(Resolved::Builtin)
    }

    /// Finds `name` among the locals of the context at `depth`, capturing it from
    /// enclosing contexts when that context is a closure.
    fn lookup_in(&mut self, depth: usize, name: &str) -> Option<Resolved> {
        let ctx = &self.stack[depth];
        if let Some((_, l)) = ctx.scopes.iter().rev().find(|(n, _)| n == name) {
            return Some(Resolved::Local(*l));
        }
        if let Some(l) = ctx.names.get(name) {
            return Some(Resolved::Local(*l));
        }
        if let Some((n, ty)) = &ctx.self_name
            && n == name
        {
            return Some(Resolved::Recurse(ty.clone()));
        }
        if !ctx.is_closure || depth == 0 {
            return None;
        }
        match self.lookup_in(depth - 1, name)? {
            Resolved::Local(outer) => {
                let outer_local = &self.stack[depth - 1].locals[outer.0 as usize];
                let (ty, span) = (outer_local.ty.clone(), outer_local.span);
                let ctx = &mut self.stack[depth];
                let inner = ctx.add_local(name, ty, span);
                ctx.names.insert(name.to_string(), inner);
                ctx.captures.push((inner, outer));
                Some(Resolved::Local(inner))
            }
            other => Some(other),
        }
    }

    /// A dimension in `Tensor[...]`: a size, `_` for any size, or a name that
    /// stands for the same size wherever it appears in the function.
    fn resolve_dim(&mut self, t: &ast::TypeExpr) -> Ty {
        if !t.args.is_empty() {
            self.error(Diagnostic::error(
                t.span,
                "a tensor dimension is a number or a name",
            ));
            return Ty::Any;
        }
        if let Ok(n) = t.name.parse::<u64>() {
            return Ty::Dim(n);
        }
        if t.name == "_" {
            return Ty::Any;
        }
        if let Some(d) = self
            .dim_names
            .iter()
            .rev()
            .find_map(|scope| scope.get(&t.name))
        {
            return d.clone();
        }
        let d = self.infer.fresh();
        if let Some(scope) = self.dim_names.last_mut() {
            scope.insert(t.name.clone(), d.clone());
        }
        d
    }

    fn resolve_type(&mut self, t: &ast::TypeExpr) -> Ty {
        let arity = |c: &mut Self, n: usize| -> bool {
            if t.args.len() != n {
                let msg = format!(
                    "`{}` takes {n} type argument{}",
                    t.name,
                    if n == 1 { "" } else { "s" }
                );
                c.error(Diagnostic::error(t.span, msg));
                return false;
            }
            true
        };
        let simple = match t.name.as_str() {
            "Int" | "int" => Some(Ty::Int),
            "Float" | "float" => Some(Ty::Float),
            "Bool" | "bool" => Some(Ty::Bool),
            "String" | "str" | "string" => Some(Ty::Str),
            "None" => Some(Ty::None),
            "Any" => Some(Ty::Any),
            "Tensor" if t.args.is_empty() => Some(Ty::tensor()),
            "Optimizer" => Some(Ty::Optimizer),
            _ => None,
        };
        if let Some(ty) = simple {
            arity(self, 0);
            return ty;
        }
        match t.name.as_str() {
            "List" | "list" => {
                if !arity(self, 1) {
                    return Ty::Any;
                }
                let e = self.resolve_type(&t.args[0]);
                Ty::list(e)
            }
            "Task" => {
                if !arity(self, 1) {
                    return Ty::Any;
                }
                let e = self.resolve_type(&t.args[0]);
                Ty::task(e)
            }
            "Tuple" => {
                let items = t.args.iter().map(|a| self.resolve_type(a)).collect();
                Ty::Tuple(items)
            }
            "Tensor" => Ty::Tensor(Some(t.args.iter().map(|a| self.resolve_dim(a)).collect())),
            "Map" | "map" | "dict" => {
                if !arity(self, 2) {
                    return Ty::Any;
                }
                let k = self.resolve_type(&t.args[0]);
                let v = self.resolve_type(&t.args[1]);
                Ty::map(k, v)
            }
            name if name.contains('.') => {
                let (module, class) = name.split_once('.').expect("contains a dot");
                let found = match self.lookup(module) {
                    Some(Resolved::Module(m)) => {
                        match self.modules[m.0 as usize].names.get(class) {
                            Some(Item::Class(c)) => Some(*c),
                            _ => None,
                        }
                    }
                    _ => None,
                };
                match found {
                    Some(c) => {
                        arity(self, 0);
                        Ty::Class(c)
                    }
                    None => {
                        self.error(Diagnostic::error(t.span, format!("unknown type `{name}`")));
                        Ty::Any
                    }
                }
            }
            name => match self.lookup(name) {
                Some(Resolved::Class(c)) => {
                    arity(self, 0);
                    Ty::Class(c)
                }
                _ => {
                    self.error(Diagnostic::error(t.span, format!("unknown type `{name}`")).with_help("built-in types are Int, Float, Bool, String, None, Any, List[T], Map[K, V], Task[T], Tensor, Tensor[dims...] and Optimizer"));
                    Ty::Any
                }
            },
        }
    }

    // ---- finishing -----------------------------------------------------------------------

    /// Re-checks constraints that were waiting on types fixed later in the program.
    fn solve_deferred(&mut self) {
        let deferred = std::mem::take(&mut self.deferred);
        self.stack.push(FnCtx::default());
        for c in deferred {
            self.add_constraint(c);
        }
        self.solve_pending();
        let ctx = self.stack.pop().expect("pushed above");
        self.deferred.extend(ctx.pending);
    }

    fn finish(&mut self, entry: ModuleId) -> Result<hir::Program, Vec<Diagnostic>> {
        self.solve_deferred();
        if !self.diags.is_empty() {
            return Err(std::mem::take(&mut self.diags));
        }
        Ok(self.snapshot(entry))
    }

    /// The checked program so far, with every type resolved as far as it can be.
    fn snapshot(&self, entry: ModuleId) -> hir::Program {
        let infer = &self.infer;
        let generics: Vec<Vec<u32>> = self
            .funcs
            .iter()
            .map(|f| {
                f.hir
                    .as_ref()
                    .map(|h| h.generics.clone())
                    .unwrap_or_default()
            })
            .collect();
        let z = Zonk {
            infer,
            generics: &generics,
        };
        let funcs = self
            .funcs
            .iter()
            .map(|slot| {
                let mut f = slot.hir.clone().expect("every function is checked");
                f.ty = infer.zonk(&f.ty);
                for l in &mut f.locals {
                    l.ty = infer.zonk(&l.ty);
                }
                z.block(&mut f.body);
                f
            })
            .collect();
        let classes = self
            .classes
            .iter()
            .map(|c| hir::Class {
                name: c.name.clone(),
                span: c.span,
                fields: c
                    .fields
                    .iter()
                    .map(|f| hir::Field {
                        name: f.name.clone(),
                    })
                    .collect(),
                methods: c.methods.clone(),
            })
            .collect();
        let globals = self
            .globals
            .iter()
            .map(|g| hir::Global {
                name: g.name.clone(),
                ty: infer.zonk(&g.ty),
                span: g.span,
            })
            .collect();
        let modules = self
            .modules
            .iter()
            .map(|m| hir::Module {
                init: m.init.expect("module init"),
            })
            .collect();
        hir::Program {
            funcs,
            classes,
            globals,
            modules,
            entry,
        }
    }
}

/// Resolves the types left in the HIR once checking is complete. A call that
/// recorded no instantiation of a generic function is a recursive call made
/// before the function was generalized, so it uses the function's own variables.
struct Zonk<'a> {
    infer: &'a Infer,
    generics: &'a [Vec<u32>],
}

impl Zonk<'_> {
    fn block(&self, block: &mut hir::Block) {
        for stmt in block {
            self.stmt(stmt);
        }
    }

    fn inst(&self, func: FuncId, inst: &mut Vec<Ty>) {
        let generics = &self.generics[func.0 as usize];
        if inst.is_empty() && !generics.is_empty() {
            *inst = generics.iter().map(|v| Ty::Var(*v)).collect();
        }
        // Engines do not use shapes, so they must not split instantiations.
        for t in inst {
            *t = self.infer.zonk(t).unshaped();
        }
    }

    fn place(&self, place: &mut hir::Place) {
        match place {
            hir::Place::Field { target, .. } => self.expr(target),
            hir::Place::Index { target, index } => {
                self.expr(target);
                self.expr(index);
            }
            hir::Place::Local(_) | hir::Place::Global(_) => {}
        }
    }

    fn stmt(&self, stmt: &mut hir::Stmt) {
        use hir::StmtKind::*;
        match &mut stmt.kind {
            Expr(e) => self.expr(e),
            Assign { place, value, .. } => {
                self.place(place);
                self.expr(value);
            }
            Show(es) => es.iter_mut().for_each(|e| self.expr(e)),
            If {
                branches,
                otherwise,
            } => {
                for (c, b) in branches {
                    self.expr(c);
                    self.block(b);
                }
                self.block(otherwise);
            }
            While { cond, body } => {
                self.expr(cond);
                self.block(body);
            }
            For { var, iter, body } => {
                self.place(var);
                self.expr(iter);
                self.block(body);
            }
            Repeat { count, body, .. } => {
                self.expr(count);
                self.block(body);
            }
            Return(Some(e)) => self.expr(e),
            Try { body, var, handler } => {
                self.block(body);
                self.place(var);
                self.block(handler);
            }
            Return(None) | Break | Continue | InitModule(_) => {}
        }
    }

    fn expr(&self, expr: &mut hir::Expr) {
        use hir::ExprKind::*;
        expr.ty = self.infer.zonk(&expr.ty);
        match &mut expr.kind {
            FStr(parts) => {
                for p in parts {
                    if let hir::FStrPart::Expr { expr, .. } = p {
                        self.expr(expr);
                    }
                }
            }
            List(es) | Tuple(es) | RunParallel(es) => es.iter_mut().for_each(|e| self.expr(e)),
            Map(entries) => {
                for (k, v) in entries {
                    self.expr(k);
                    self.expr(v);
                }
            }
            CallFunc {
                func, args, inst, ..
            } => {
                self.inst(*func, inst);
                args.iter_mut().for_each(|e| self.expr(e));
            }
            Func { func, inst } => self.inst(*func, inst),
            Closure { captures: args, .. } | CallBuiltin { args, .. } | New { args, .. } => {
                args.iter_mut().for_each(|e| self.expr(e))
            }
            Unary { operand: e, .. }
            | Field { target: e, .. }
            | Together(e)
            | Await(e)
            | IntToFloat(e) => self.expr(e),
            IfElse {
                cond,
                then,
                otherwise,
            } => {
                self.expr(cond);
                self.expr(then);
                self.expr(otherwise);
            }
            Binary { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            Call { callee, args } => {
                self.expr(callee);
                args.iter_mut().for_each(|e| self.expr(e));
            }
            MethodCall { recv, args, .. } => {
                self.expr(recv);
                args.iter_mut().for_each(|e| self.expr(e));
            }
            Index { target, index } => {
                self.expr(target);
                self.expr(index);
            }
            Slice { target, start, end } => {
                self.expr(target);
                if let Some(s) = start {
                    self.expr(s);
                }
                if let Some(e) = end {
                    self.expr(e);
                }
            }
            Query { target, body, .. } => {
                self.expr(target);
                if let Some(b) = body {
                    self.expr(b);
                }
            }
            Int(_) | Float(_) | Bool(_) | Str(_) | None | Local(_) | Global(_) | Recurse
            | PyImport(_) => {}
        }
    }
}
