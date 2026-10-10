//! Incremental checking for the interactive prompt: each input is checked as
//! more top-level code of one growing module.

use super::hir::{self, FuncId, ModuleId};
use super::{Checker, ModuleSlot};
use crate::diagnostic::Diagnostic;
use crate::source::SourceMap;
use crate::syntax;
use std::collections::HashMap;
use std::path::PathBuf;

pub struct Session {
    checker: Checker,
    module: ModuleId,
    inputs: u32,
}

/// A checked input: the whole program so far, and the function holding the input's code.
pub struct Input {
    pub program: hir::Program,
    pub run: FuncId,
}

impl Default for Session {
    fn default() -> Self {
        let mut checker = Checker::new(SourceMap::default());
        checker.redefine = true;
        checker.modules.push(ModuleSlot {
            path: PathBuf::from("<repl>"),
            names: HashMap::new(),
            init: None,
            loaded: true,
        });
        Session {
            checker,
            module: ModuleId(0),
            inputs: 0,
        }
    }
}

impl Session {
    pub fn sources(&self) -> &SourceMap {
        &self.checker.sources
    }

    /// Checks `text`. On failure everything it declared is forgotten.
    pub fn submit(&mut self, text: &str) -> Result<Input, Vec<Diagnostic>> {
        self.inputs += 1;
        let file = self
            .checker
            .sources
            .add(format!("<input {}>", self.inputs), text.to_string());
        let source = self.checker.sources.get(file).text.clone();
        let module = syntax::parse(file, &source).map_err(|d| vec![d])?;

        let c = &mut self.checker;
        let saved = (
            c.funcs.len(),
            c.classes.len(),
            c.globals.len(),
            c.modules.len(),
            c.deferred.len(),
            c.modules[self.module.0 as usize].names.clone(),
        );
        c.declare_module(
            self.module,
            &module,
            &std::env::current_dir().unwrap_or_default().join("<repl>"),
        );
        let run = c.check_top_level(self.module, &module.stmts);
        c.solve_deferred();
        if !c.diags.is_empty() {
            let (funcs, classes, globals, modules, deferred, names) = saved;
            c.funcs.truncate(funcs);
            c.classes.truncate(classes);
            c.globals.truncate(globals);
            c.modules.truncate(modules);
            c.deferred.truncate(deferred);
            c.modules[self.module.0 as usize].names = names;
            return Err(std::mem::take(&mut c.diags));
        }
        c.modules[self.module.0 as usize].init = Some(run);

        let mut program = c.snapshot(self.module);
        echo_last_value(&mut program.funcs[run.0 as usize].body);
        Ok(Input { program, run })
    }
}

/// Shows the value of a final bare expression, as interactive prompts do.
fn echo_last_value(body: &mut hir::Block) {
    let Some(last) = body.last_mut() else { return };
    let hir::StmtKind::Expr(e) = &last.kind else {
        return;
    };
    if e.ty == super::types::Ty::None || matches!(e.ty, super::types::Ty::Var(_)) {
        return;
    }
    let hir::StmtKind::Expr(e) = std::mem::replace(&mut last.kind, hir::StmtKind::Break) else {
        unreachable!()
    };
    last.kind = hir::StmtKind::Show(vec![e]);
}
