//! Static analysis, AST/symbol inspection, linting, formatting, disassembly, and compatibility reporting.
use crate::{
    debug::location,
    execution::{atomic_bytes, load_source_file, protect_output, resolve_modules, to_diagnostic},
    model::*,
};
use ppl_compiler::{
    CompiledProgram, Op,
    builtins::{BUILTINS, Builtin, Status},
    catalog::{self, CatalogKind},
};
use ppl_syntax::{
    Diagnostics, SourceFile, Span,
    ast::{Expr, ExprKind, FunctionDef, Item, Program, Stmt, StmtKind},
    hpprgm,
    lexer::{Keyword, Tok, Token, lex_with_trivia},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    path::Path,
    time::Instant,
};

fn source_error_report(op: &str, path: &Path, code: u8, msg: String, started: Instant) -> Report {
    let mut report = Report::new(op, Some(path.to_owned()));
    report.fail(
        match code {
            5 => "io_error",
            6 => "resource_limit",
            2 => "invalid_request",
            _ => "invalid_source",
        },
        code,
        &msg,
    );
    if code == 1 {
        report.diagnostics.push(Diagnostic {
            severity: "error".into(),
            class: "syntax".into(),
            code: "source-load".into(),
            message: msg,
            location: None,
            hint: None,
            unverified: false,
        });
    }
    report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
    report
}

fn compile_with_modules(
    path: &Path,
    modules: &[String],
    op: &str,
    started: Instant,
) -> Result<
    (
        ppl_engine::source::Source,
        SourceFile,
        CompiledProgram,
        Vec<Diagnostic>,
    ),
    Box<Report>,
> {
    let source = match load_source_file(path, None) {
        Ok(s) => s,
        Err((code, msg)) => {
            return Err(Box::new(source_error_report(op, path, code, msg, started)));
        }
    };
    let file = SourceFile::new(&source.identity.display_name, &source.text);
    let (sources, mut catalog) = match resolve_modules(&source, modules) {
        Ok(v) => v,
        Err((code, msg)) => {
            return Err(Box::new(source_error_report(op, path, code, msg, started)));
        }
    };
    let mut diags = Vec::new();
    let program = if catalog.is_empty() {
        let (prog, raw_diags) = ppl_compiler::compile_source(&file, &Default::default());
        diags.extend(raw_diags.items.iter().map(|d| to_diagnostic(&file, d)));
        prog
    } else {
        catalog.insert(
            file.name.clone(),
            ppl_vm::ModuleSource {
                identity: ppl_vm::SourceIdentity::new(&file.name, &source.identity.source_sha256),
                source: source.text.clone(),
            },
        );
        match ppl_vm::link_sources(&catalog, &file.name) {
            Ok(image) => {
                for (identity, mod_diags) in image.diagnostics() {
                    let sf = sources.get(&identity.name).unwrap_or(&file);
                    diags.extend(mod_diags.items.iter().map(|d| to_diagnostic(sf, d)));
                }
                Some(image.program().clone())
            }
            Err(e) => {
                let mut report = Report::new(op, Some(path.to_owned()));
                report.source = Some(source.identity);
                report.diagnostics.push(Diagnostic {
                    severity: "error".into(),
                    class: "semantic".into(),
                    code: "module-link".into(),
                    message: e.detail.clone(),
                    location: None,
                    hint: Some("Check the referenced module's exports and source".into()),
                    unverified: false,
                });
                report.fail("invalid_source", 1, e.to_string());
                report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
                return Err(Box::new(report));
            }
        }
    };
    let Some(program) = program.filter(|_| !source.text.trim().is_empty()) else {
        let mut report = Report::new(op, Some(path.to_owned()));
        report.source = Some(source.identity);
        report.compatibility_limitations = diags
            .iter()
            .filter(|d| d.class == "compatibility")
            .map(|d| d.message.clone())
            .collect();
        report.diagnostics = diags;
        report.fail("invalid_source", 1, "source validation failed");
        report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
        return Err(Box::new(report));
    };
    Ok((source, file, program, diags))
}

fn expr_to_json(file: &SourceFile, expr: &Expr) -> Value {
    let loc = location(file, expr.span);
    match &expr.kind {
        ExprKind::Number(n) => json!({"kind": "number", "value": n, "location": loc}),
        ExprKind::HashInt(h) => {
            json!({"kind": "hash_int", "value": format!("#{h}"), "location": loc})
        }
        ExprKind::Str(s) => json!({"kind": "string", "value": s, "location": loc}),
        ExprKind::Ident(id) => json!({"kind": "ident", "name": id, "location": loc}),
        ExprKind::List(items) => json!({
            "kind": "list",
            "items": items.iter().map(|e| expr_to_json(file, e)).collect::<Vec<_>>(),
            "location": loc,
        }),
        ExprKind::Bracket(items) => json!({
            "kind": "bracket",
            "items": items.iter().map(|e| expr_to_json(file, e)).collect::<Vec<_>>(),
            "location": loc,
        }),
        ExprKind::ComplexPair(re, im) => json!({
            "kind": "complex",
            "re": expr_to_json(file, re),
            "im": expr_to_json(file, im),
            "location": loc,
        }),
        ExprKind::Unary(op, inner) => json!({
            "kind": "unary",
            "op": format!("{op:?}"),
            "expr": expr_to_json(file, inner),
            "location": loc,
        }),
        ExprKind::Postfix(op, inner) => json!({
            "kind": "postfix",
            "op": format!("{op:?}"),
            "expr": expr_to_json(file, inner),
            "location": loc,
        }),
        ExprKind::Binary(op, lhs, rhs) => json!({
            "kind": "binary",
            "op": op.symbol(),
            "lhs": expr_to_json(file, lhs),
            "rhs": expr_to_json(file, rhs),
            "location": loc,
        }),
        ExprKind::Call(callee, args) => json!({
            "kind": "call",
            "callee": expr_to_json(file, callee),
            "args": args.iter().map(|a| expr_to_json(file, a)).collect::<Vec<_>>(),
            "location": loc,
        }),
        ExprKind::Index(base, indices) => json!({
            "kind": "index",
            "base": expr_to_json(file, base),
            "indices": indices.iter().map(|a| expr_to_json(file, a)).collect::<Vec<_>>(),
            "location": loc,
        }),
        ExprKind::Assign(target, value) => json!({
            "kind": "assign",
            "target": expr_to_json(file, target),
            "value": expr_to_json(file, value),
            "location": loc,
        }),
        ExprKind::Store(value, target) => json!({
            "kind": "store",
            "value": expr_to_json(file, value),
            "target": expr_to_json(file, target),
            "location": loc,
        }),
        ExprKind::Quoted(inner) => json!({
            "kind": "quoted",
            "expr": expr_to_json(file, inner),
            "location": loc,
        }),
        ExprKind::Error => json!({"kind": "error", "location": loc}),
    }
}

fn stmt_to_json(file: &SourceFile, stmt: &Stmt) -> Value {
    let loc = location(file, stmt.span);
    match &stmt.kind {
        StmtKind::Expr(e) => {
            json!({"kind": "expr", "expr": expr_to_json(file, e), "location": loc})
        }
        StmtKind::Local(vars) => json!({
            "kind": "local",
            "vars": vars.iter().map(|v| json!({
                "name": v.name.name,
                "init": v.init.as_ref().map(|e| expr_to_json(file, e)),
                "location": location(file, v.name.span),
            })).collect::<Vec<_>>(),
            "location": loc,
        }),
        StmtKind::Block(stmts) => json!({
            "kind": "block",
            "stmts": stmts.iter().map(|s| stmt_to_json(file, s)).collect::<Vec<_>>(),
            "location": loc,
        }),
        StmtKind::If {
            cond,
            then_body,
            else_body,
        } => json!({
            "kind": "if",
            "cond": expr_to_json(file, cond),
            "then": then_body.iter().map(|s| stmt_to_json(file, s)).collect::<Vec<_>>(),
            "else": else_body.as_ref().map(|b| b.iter().map(|s| stmt_to_json(file, s)).collect::<Vec<_>>()),
            "location": loc,
        }),
        StmtKind::Case { arms, default } => json!({
            "kind": "case",
            "arms": arms.iter().map(|(c, b)| json!({
                "cond": expr_to_json(file, c),
                "body": b.iter().map(|s| stmt_to_json(file, s)).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "default": default.as_ref().map(|b| b.iter().map(|s| stmt_to_json(file, s)).collect::<Vec<_>>()),
            "location": loc,
        }),
        StmtKind::IfErr {
            body,
            handler,
            else_body,
        } => json!({
            "kind": "iferr",
            "body": body.iter().map(|s| stmt_to_json(file, s)).collect::<Vec<_>>(),
            "handler": handler.iter().map(|s| stmt_to_json(file, s)).collect::<Vec<_>>(),
            "else": else_body.as_ref().map(|b| b.iter().map(|s| stmt_to_json(file, s)).collect::<Vec<_>>()),
            "location": loc,
        }),
        StmtKind::For {
            var,
            from,
            to,
            step,
            down,
            body,
        } => json!({
            "kind": "for",
            "var": var.name,
            "from": expr_to_json(file, from),
            "to": expr_to_json(file, to),
            "step": step.as_ref().map(|e| expr_to_json(file, e)),
            "down": down,
            "body": body.iter().map(|s| stmt_to_json(file, s)).collect::<Vec<_>>(),
            "location": loc,
        }),
        StmtKind::While { cond, body } => json!({
            "kind": "while",
            "cond": expr_to_json(file, cond),
            "body": body.iter().map(|s| stmt_to_json(file, s)).collect::<Vec<_>>(),
            "location": loc,
        }),
        StmtKind::Repeat { body, until } => json!({
            "kind": "repeat",
            "body": body.iter().map(|s| stmt_to_json(file, s)).collect::<Vec<_>>(),
            "until": expr_to_json(file, until),
            "location": loc,
        }),
        StmtKind::Return(e) => json!({
            "kind": "return",
            "expr": e.as_ref().map(|x| expr_to_json(file, x)),
            "location": loc,
        }),
        StmtKind::Break(e) => json!({
            "kind": "break",
            "expr": e.as_ref().map(|x| expr_to_json(file, x)),
            "location": loc,
        }),
        StmtKind::Continue => json!({"kind": "continue", "location": loc}),
        StmtKind::Kill => json!({"kind": "kill", "location": loc}),
    }
}

fn format_ast_tree(file: &SourceFile, program: &Program) -> String {
    fn write_expr(out: &mut String, indent: usize, expr: &Expr) {
        let pad = " ".repeat(indent);
        match &expr.kind {
            ExprKind::Number(n) => {
                let _ = writeln!(out, "{pad}Number({n})");
            }
            ExprKind::HashInt(h) => {
                let _ = writeln!(out, "{pad}HashInt(#{h})");
            }
            ExprKind::Str(s) => {
                let _ = writeln!(out, "{pad}String({s:?})");
            }
            ExprKind::Ident(id) => {
                let _ = writeln!(out, "{pad}Ident({id})");
            }
            ExprKind::List(items) => {
                let _ = writeln!(out, "{pad}List({})", items.len());
                for item in items {
                    write_expr(out, indent + 2, item);
                }
            }
            ExprKind::Bracket(items) => {
                let _ = writeln!(out, "{pad}Bracket({})", items.len());
                for item in items {
                    write_expr(out, indent + 2, item);
                }
            }
            ExprKind::ComplexPair(r, i) => {
                let _ = writeln!(out, "{pad}ComplexPair");
                write_expr(out, indent + 2, r);
                write_expr(out, indent + 2, i);
            }
            ExprKind::Unary(op, inner) => {
                let _ = writeln!(out, "{pad}Unary({op:?})");
                write_expr(out, indent + 2, inner);
            }
            ExprKind::Postfix(op, inner) => {
                let _ = writeln!(out, "{pad}Postfix({op:?})");
                write_expr(out, indent + 2, inner);
            }
            ExprKind::Binary(op, lhs, rhs) => {
                let _ = writeln!(out, "{pad}Binary({})", op.symbol());
                write_expr(out, indent + 2, lhs);
                write_expr(out, indent + 2, rhs);
            }
            ExprKind::Call(callee, args) => {
                let _ = writeln!(out, "{pad}Call(args={})", args.len());
                write_expr(out, indent + 2, callee);
                for arg in args {
                    write_expr(out, indent + 2, arg);
                }
            }
            ExprKind::Index(base, idx) => {
                let _ = writeln!(out, "{pad}Index({})", idx.len());
                write_expr(out, indent + 2, base);
                for i in idx {
                    write_expr(out, indent + 2, i);
                }
            }
            ExprKind::Assign(t, v) => {
                let _ = writeln!(out, "{pad}Assign(:=)");
                write_expr(out, indent + 2, t);
                write_expr(out, indent + 2, v);
            }
            ExprKind::Store(v, t) => {
                let _ = writeln!(out, "{pad}Store(▶)");
                write_expr(out, indent + 2, v);
                write_expr(out, indent + 2, t);
            }
            ExprKind::Quoted(inner) => {
                let _ = writeln!(out, "{pad}Quoted");
                write_expr(out, indent + 2, inner);
            }
            ExprKind::Error => {
                let _ = writeln!(out, "{pad}Error");
            }
        }
    }

    fn write_stmt(out: &mut String, file: &SourceFile, indent: usize, stmt: &Stmt) {
        let pad = " ".repeat(indent);
        let line = file.line_col(stmt.span.start).line;
        match &stmt.kind {
            StmtKind::Expr(e) => {
                let _ = writeln!(out, "{pad}ExprStmt @{line}");
                write_expr(out, indent + 2, e);
            }
            StmtKind::Local(vars) => {
                let names: Vec<_> = vars.iter().map(|v| v.name.name.as_str()).collect();
                let _ = writeln!(out, "{pad}Local({}) @{line}", names.join(", "));
            }
            StmtKind::Block(stmts) => {
                let _ = writeln!(out, "{pad}Block @{line}");
                for s in stmts {
                    write_stmt(out, file, indent + 2, s);
                }
            }
            StmtKind::If {
                cond,
                then_body,
                else_body,
            } => {
                let _ = writeln!(out, "{pad}If @{line}");
                write_expr(out, indent + 2, cond);
                for s in then_body {
                    write_stmt(out, file, indent + 2, s);
                }
                if let Some(eb) = else_body {
                    let _ = writeln!(out, "{pad}Else");
                    for s in eb {
                        write_stmt(out, file, indent + 2, s);
                    }
                }
            }
            StmtKind::Case { arms, default } => {
                let _ = writeln!(out, "{pad}Case @{line}");
                for (c, b) in arms {
                    let _ = writeln!(out, "{pad}  Arm");
                    write_expr(out, indent + 4, c);
                    for s in b {
                        write_stmt(out, file, indent + 4, s);
                    }
                }
                if let Some(d) = default {
                    let _ = writeln!(out, "{pad}  Default");
                    for s in d {
                        write_stmt(out, file, indent + 4, s);
                    }
                }
            }
            StmtKind::IfErr {
                body,
                handler,
                else_body,
            } => {
                let _ = writeln!(out, "{pad}IfErr @{line}");
                for s in body {
                    write_stmt(out, file, indent + 2, s);
                }
                let _ = writeln!(out, "{pad}Handler");
                for s in handler {
                    write_stmt(out, file, indent + 2, s);
                }
                if let Some(e) = else_body {
                    let _ = writeln!(out, "{pad}Else");
                    for s in e {
                        write_stmt(out, file, indent + 2, s);
                    }
                }
            }
            StmtKind::For {
                var,
                from,
                to,
                down,
                body,
                ..
            } => {
                let _ = writeln!(
                    out,
                    "{pad}For({}{}) @{line}",
                    var.name,
                    if *down { " DOWNTO" } else { " TO" }
                );
                write_expr(out, indent + 2, from);
                write_expr(out, indent + 2, to);
                for s in body {
                    write_stmt(out, file, indent + 2, s);
                }
            }
            StmtKind::While { cond, body } => {
                let _ = writeln!(out, "{pad}While @{line}");
                write_expr(out, indent + 2, cond);
                for s in body {
                    write_stmt(out, file, indent + 2, s);
                }
            }
            StmtKind::Repeat { body, until } => {
                let _ = writeln!(out, "{pad}Repeat @{line}");
                for s in body {
                    write_stmt(out, file, indent + 2, s);
                }
                let _ = writeln!(out, "{pad}Until");
                write_expr(out, indent + 2, until);
            }
            StmtKind::Return(e) => {
                let _ = writeln!(out, "{pad}Return @{line}");
                if let Some(e) = e {
                    write_expr(out, indent + 2, e);
                }
            }
            StmtKind::Break(e) => {
                let _ = writeln!(out, "{pad}Break @{line}");
                if let Some(e) = e {
                    write_expr(out, indent + 2, e);
                }
            }
            StmtKind::Continue => {
                let _ = writeln!(out, "{pad}Continue @{line}");
            }
            StmtKind::Kill => {
                let _ = writeln!(out, "{pad}Kill @{line}");
            }
        }
    }

    let mut out = String::from("Program\n");
    for (p, span) in &program.pragmas {
        let _ = writeln!(out, "  Pragma({p}) @{}", file.line_col(span.start).line);
    }
    for item in &program.items {
        match item {
            Item::Globals {
                exported,
                vars,
                span,
            } => {
                let vis = if *exported { "EXPORT" } else { "LOCAL" };
                let names: Vec<_> = vars.iter().map(|v| v.name.name.as_str()).collect();
                let _ = writeln!(
                    out,
                    "  Globals({vis}: {}) @{}",
                    names.join(", "),
                    file.line_col(span.start).line
                );
            }
            Item::ForwardDecl { name, params, span } => {
                let p: Vec<_> = params.iter().map(|x| x.name.as_str()).collect();
                let _ = writeln!(
                    out,
                    "  ForwardDecl({}({})) @{}",
                    name.name,
                    p.join(", "),
                    file.line_col(span.start).line
                );
            }
            Item::CasBlock { text, span } => {
                let _ = writeln!(
                    out,
                    "  CasBlock({} lines) @{}",
                    text.lines().count().max(1),
                    file.line_col(span.start).line
                );
            }
            Item::Function(f) => {
                let vis = if f.is_key {
                    "KEY"
                } else if f.exported {
                    "EXPORT"
                } else if f.is_local {
                    "LOCAL"
                } else {
                    "PRIVATE"
                };
                let p: Vec<_> = f.params.iter().map(|x| x.name.as_str()).collect();
                let _ = writeln!(
                    out,
                    "  Function({vis} {}({})) @{}",
                    f.name.name,
                    p.join(", "),
                    file.line_col(f.span.start).line
                );
                for s in &f.body {
                    write_stmt(&mut out, file, 4, s);
                }
            }
        }
    }
    out
}

/// Parse a PPL program or container and emit its AST (and optional token stream).
pub fn inspect_ast(path: &Path, include_tokens: bool) -> Report {
    let started = Instant::now();
    let source = match load_source_file(path, None) {
        Ok(s) => s,
        Err((code, msg)) => return source_error_report("ast", path, code, msg, started),
    };
    let mut report = Report::new("ast", Some(path.to_owned()));
    report.source = Some(source.identity.clone());
    let file = SourceFile::new(&source.identity.display_name, &source.text);
    let (program, diags) = ppl_syntax::parse(&source.text);
    report.diagnostics = diags
        .items
        .iter()
        .map(|d| to_diagnostic(&file, d))
        .collect();
    if diags.has_errors() {
        report.fail("invalid_source", 1, "syntax parsing reported errors");
    }
    let items: Vec<Value> = program
        .items
        .iter()
        .map(|item| match item {
            Item::Function(f) => json!({
                "kind": "function",
                "name": f.name.name,
                "exported": f.exported,
                "is_key": f.is_key,
                "is_local": f.is_local,
                "params": f.params.iter().map(|p| json!({
                    "name": p.name,
                    "location": location(&file, p.span),
                })).collect::<Vec<_>>(),
                "body": f.body.iter().map(|s| stmt_to_json(&file, s)).collect::<Vec<_>>(),
                "location": location(&file, f.span),
            }),
            Item::ForwardDecl { name, params, span } => json!({
                "kind": "forward_decl",
                "name": name.name,
                "params": params.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
                "location": location(&file, *span),
            }),
            Item::Globals {
                exported,
                vars,
                span,
            } => json!({
                "kind": "globals",
                "exported": exported,
                "vars": vars.iter().map(|v| json!({
                    "name": v.name.name,
                    "init": v.init.as_ref().map(|e| expr_to_json(&file, e)),
                    "location": location(&file, v.name.span),
                })).collect::<Vec<_>>(),
                "location": location(&file, *span),
            }),
            Item::CasBlock { text, span } => json!({
                "kind": "cas_block",
                "text": text,
                "location": location(&file, *span),
            }),
        })
        .collect();
    let pragmas: Vec<Value> = program
        .pragmas
        .iter()
        .map(|(text, span)| json!({"text": text, "location": location(&file, *span)}))
        .collect();
    let mut data = json!({
        "pragmas": pragmas,
        "items": items,
        "tree": format_ast_tree(&file, &program),
    });
    if include_tokens {
        let mut lex_diags = Diagnostics::default();
        let tokens = lex_with_trivia(&source.text, &mut lex_diags);
        let tok_json: Vec<Value> = tokens
            .into_iter()
            .filter(|t| !matches!(t.tok, Tok::Eof))
            .map(|t| {
                let raw = &source.text[t.span.start as usize..t.span.end as usize];
                json!({
                    "kind": t.tok.describe(),
                    "text": raw,
                    "soft": t.soft,
                    "location": location(&file, t.span),
                })
            })
            .collect();
        data["tokens"] = Value::Array(tok_json);
    }
    report.data = data;
    report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
    report
}

/// Extract exported/private functions, globals, forward declarations, pragmas, and CAS blocks.
pub fn inspect_symbols(path: &Path, modules: &[String], exported_only: bool) -> Report {
    let started = Instant::now();
    let (source, file, program, diags) =
        match compile_with_modules(path, modules, "symbols", started) {
            Ok(v) => v,
            Err(r) => return *r,
        };
    let (ast, _) = ppl_syntax::parse(&source.text);
    let mut report = Report::new("symbols", Some(path.to_owned()));
    report.source = Some(source.identity);
    report.compatibility_limitations = diags
        .iter()
        .filter(|d| d.class == "compatibility")
        .map(|d| d.message.clone())
        .collect();
    report.diagnostics = diags;

    let mut functions = Vec::new();
    let mut forward_decls = Vec::new();
    let mut globals = Vec::new();
    let mut cas_blocks = Vec::new();

    for item in &ast.items {
        match item {
            Item::Function(f) => {
                if exported_only && !f.exported {
                    continue;
                }
                let chunk = program
                    .function_index(&f.name.name)
                    .map(|idx| &program.functions[idx]);
                functions.push(json!({
                    "name": f.name.name,
                    "exported": f.exported,
                    "is_local": f.is_local,
                    "is_key": f.is_key,
                    "params": f.params.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
                    "locals": chunk.map(|c| c.local_names.clone()).unwrap_or_default(),
                    "bytecode_len": chunk.map(|c| c.code.len()).unwrap_or(0),
                    "location": location(&file, f.span),
                }));
            }
            Item::ForwardDecl { name, params, span } => {
                if !exported_only {
                    forward_decls.push(json!({
                        "name": name.name,
                        "params": params.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
                        "location": location(&file, *span),
                    }));
                }
            }
            Item::Globals { exported, vars, .. } => {
                if exported_only && !*exported {
                    continue;
                }
                for v in vars {
                    globals.push(json!({
                        "name": v.name.name,
                        "exported": exported,
                        "has_init": v.init.is_some(),
                        "location": location(&file, v.name.span),
                    }));
                }
            }
            Item::CasBlock { text, span } => {
                if !exported_only {
                    cas_blocks.push(json!({
                        "line_count": text.lines().count().max(1),
                        "location": location(&file, *span),
                    }));
                }
            }
        }
    }

    let pragmas: Vec<Value> = ast
        .pragmas
        .iter()
        .map(|(text, span)| json!({"text": text, "location": location(&file, *span)}))
        .collect();

    report.data = json!({
        "functions": functions,
        "forward_decls": forward_decls,
        "globals": globals,
        "cas_blocks": cas_blocks,
        "pragmas": pragmas,
    });
    report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
    report
}

/// Disassemble compiled PPL bytecode into structured JSON and text.
pub fn disassemble_program(
    path: &Path,
    modules: &[String],
    function_filter: Option<&str>,
) -> Report {
    let started = Instant::now();
    let (source, file, program, diags) =
        match compile_with_modules(path, modules, "disasm", started) {
            Ok(v) => v,
            Err(r) => return *r,
        };
    let mut report = Report::new("disasm", Some(path.to_owned()));
    report.source = Some(source.identity);
    report.compatibility_limitations = diags
        .iter()
        .filter(|d| d.class == "compatibility")
        .map(|d| d.message.clone())
        .collect();
    report.diagnostics = diags;

    if let Some(fname) = function_filter
        && !std::iter::once(&program.init)
            .chain(program.functions.iter())
            .any(|c| c.name.eq_ignore_ascii_case(fname))
    {
        report.fail(
            "invalid_request",
            2,
            format!("no compiled function named `{fname}`"),
        );
        report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
        return report;
    }

    let mut fn_entries = Vec::new();
    let mut filtered_text = String::new();
    for chunk in std::iter::once(&program.init).chain(program.functions.iter()) {
        if let Some(fname) = function_filter
            && !chunk.name.eq_ignore_ascii_case(fname)
        {
            continue;
        }
        let display_name = if chunk.name.is_empty() {
            "<init>"
        } else {
            chunk.name.as_str()
        };
        let _ = writeln!(
            filtered_text,
            "fn {display_name} (params {}, locals {}):",
            chunk.params, chunk.locals
        );
        let mut insns = Vec::with_capacity(chunk.code.len());
        for (pc, op) in chunk.code.iter().enumerate() {
            let detail = match op {
                Op::Const(k) => format!("{:?}", program.consts[*k as usize]),
                Op::LoadGlobal(g) | Op::StoreGlobal(g) => program.globals[*g as usize].name.clone(),
                Op::LoadLocal(l) | Op::StoreLocal(l) => chunk
                    .local_names
                    .get(*l as usize)
                    .cloned()
                    .unwrap_or_else(|| "?".into()),
                Op::Call { func, .. } => program.functions[*func as usize].name.clone(),
                Op::CallExternal { name, .. } | Op::Unsupported { name, .. } => {
                    program.names[*name as usize].clone()
                }
                _ => String::new(),
            };
            if detail.is_empty() {
                let _ = writeln!(filtered_text, "  {pc:4}  {op:?}");
            } else {
                let _ = writeln!(filtered_text, "  {pc:4}  {op:?}    ; {detail}");
            }
            let span = chunk.spans.get(pc).copied().unwrap_or_default();
            let lc = file.line_col(span.start);
            insns.push(json!({
                "pc": pc,
                "op": format!("{op:?}"),
                "detail": if detail.is_empty() { Value::Null } else { Value::String(detail) },
                "line": lc.line,
                "column": lc.col,
            }));
        }
        fn_entries.push(json!({
            "name": display_name,
            "exported": chunk.exported,
            "params": chunk.params,
            "locals": chunk.locals,
            "local_names": chunk.local_names,
            "instructions": insns,
        }));
    }

    let text = if function_filter.is_some() {
        filtered_text
    } else {
        program.disassemble()
    };
    let consts: Vec<String> = program.consts.iter().map(|c| format!("{c:?}")).collect();
    let globals: Vec<Value> = program
        .globals
        .iter()
        .enumerate()
        .map(|(i, g)| {
            json!({
                "index": i,
                "name": g.name,
                "program_var": g.program_var,
                "exported": g.exported,
                "external": g.external,
                "read_only": g.read_only,
            })
        })
        .collect();

    report.data = json!({
        "text": text,
        "consts": consts,
        "globals": globals,
        "functions": fn_entries,
    });
    report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
    report
}

// --- Static Linter ---

struct LintCtx<'a> {
    file: &'a SourceFile,
    diags: Vec<Diagnostic>,
}

impl<'a> LintCtx<'a> {
    fn warn(&mut self, code: &str, span: Span, msg: String, hint: Option<String>) {
        self.diags.push(Diagnostic {
            severity: "warning".into(),
            class: "semantic".into(),
            code: code.into(),
            message: msg,
            location: Some(location(self.file, span)),
            hint,
            unverified: false,
        });
    }
    fn note(&mut self, code: &str, class: &str, span: Span, msg: String, hint: Option<String>) {
        self.diags.push(Diagnostic {
            severity: "note".into(),
            class: class.into(),
            code: code.into(),
            message: msg,
            location: Some(location(self.file, span)),
            hint,
            unverified: false,
        });
    }
}

fn collect_expr_reads_and_calls(
    expr: &Expr,
    reads: &mut BTreeSet<String>,
    calls: &mut BTreeSet<String>,
    in_write_target: bool,
) {
    match &expr.kind {
        ExprKind::Ident(name) => {
            if !in_write_target {
                reads.insert(name.to_ascii_lowercase());
            }
        }
        ExprKind::List(xs) | ExprKind::Bracket(xs) => {
            for x in xs {
                collect_expr_reads_and_calls(x, reads, calls, false);
            }
        }
        ExprKind::ComplexPair(a, b) => {
            collect_expr_reads_and_calls(a, reads, calls, false);
            collect_expr_reads_and_calls(b, reads, calls, false);
        }
        ExprKind::Unary(_, e) | ExprKind::Postfix(_, e) | ExprKind::Quoted(e) => {
            collect_expr_reads_and_calls(e, reads, calls, false);
        }
        ExprKind::Binary(_, a, b) => {
            collect_expr_reads_and_calls(a, reads, calls, false);
            collect_expr_reads_and_calls(b, reads, calls, false);
        }
        ExprKind::Call(callee, args) => {
            if let ExprKind::Ident(name) = &callee.kind {
                calls.insert(name.to_ascii_lowercase());
                if let Some(spec) = ppl_compiler::builtins::lookup_case_insensitive(name) {
                    for (idx, arg) in args.iter().enumerate() {
                        if idx < spec.ref_args as usize {
                            // Ref arguments like INPUT(X) read/write X; treat as used.
                            collect_expr_reads_and_calls(arg, reads, calls, false);
                        } else {
                            collect_expr_reads_and_calls(arg, reads, calls, false);
                        }
                    }
                    return;
                }
            } else {
                collect_expr_reads_and_calls(callee, reads, calls, false);
            }
            for arg in args {
                collect_expr_reads_and_calls(arg, reads, calls, false);
            }
        }
        ExprKind::Index(base, idx) => {
            collect_expr_reads_and_calls(base, reads, calls, in_write_target);
            for i in idx {
                collect_expr_reads_and_calls(i, reads, calls, false);
            }
        }
        ExprKind::Assign(target, val) | ExprKind::Store(val, target) => {
            collect_expr_reads_and_calls(val, reads, calls, false);
            if matches!(target.kind, ExprKind::Index(_, _)) {
                collect_expr_reads_and_calls(target, reads, calls, true);
            }
        }
        ExprKind::Number(_) | ExprKind::HashInt(_) | ExprKind::Str(_) | ExprKind::Error => {}
    }
}

fn scan_stmt_for_lint(
    stmt: &Stmt,
    reads: &mut BTreeSet<String>,
    calls: &mut BTreeSet<String>,
    locals: &mut Vec<(String, Span)>,
    ctx: &mut LintCtx<'_>,
) {
    match &stmt.kind {
        StmtKind::Expr(e) => collect_expr_reads_and_calls(e, reads, calls, false),
        StmtKind::Local(vars) => {
            for v in vars {
                locals.push((v.name.name.clone(), v.name.span));
                if let Some(init) = &v.init {
                    collect_expr_reads_and_calls(init, reads, calls, false);
                }
            }
        }
        StmtKind::Block(stmts) => scan_stmt_list_for_lint(stmts, reads, calls, locals, ctx),
        StmtKind::If {
            cond,
            then_body,
            else_body,
        } => {
            collect_expr_reads_and_calls(cond, reads, calls, false);
            if then_body.is_empty() {
                ctx.note(
                    "empty-block",
                    "semantic",
                    stmt.span,
                    "empty THEN branch in IF statement".into(),
                    Some("Add statements or remove the redundant branch".into()),
                );
            }
            scan_stmt_list_for_lint(then_body, reads, calls, locals, ctx);
            if let Some(eb) = else_body {
                if eb.is_empty() {
                    ctx.note(
                        "empty-block",
                        "semantic",
                        stmt.span,
                        "empty ELSE branch in IF statement".into(),
                        Some("Remove the empty ELSE branch".into()),
                    );
                }
                scan_stmt_list_for_lint(eb, reads, calls, locals, ctx);
            }
        }
        StmtKind::Case { arms, default } => {
            for (c, b) in arms {
                collect_expr_reads_and_calls(c, reads, calls, false);
                if b.is_empty() {
                    ctx.note(
                        "empty-block",
                        "semantic",
                        c.span,
                        "empty CASE arm body".into(),
                        None,
                    );
                }
                scan_stmt_list_for_lint(b, reads, calls, locals, ctx);
            }
            if let Some(d) = default {
                scan_stmt_list_for_lint(d, reads, calls, locals, ctx);
            }
        }
        StmtKind::IfErr {
            body,
            handler,
            else_body,
        } => {
            scan_stmt_list_for_lint(body, reads, calls, locals, ctx);
            if handler.is_empty() {
                ctx.note(
                    "empty-block",
                    "semantic",
                    stmt.span,
                    "empty IFERR error handler".into(),
                    None,
                );
            }
            scan_stmt_list_for_lint(handler, reads, calls, locals, ctx);
            if let Some(eb) = else_body {
                scan_stmt_list_for_lint(eb, reads, calls, locals, ctx);
            }
        }
        StmtKind::For {
            var,
            from,
            to,
            step,
            body,
            ..
        } => {
            reads.insert(var.name.to_ascii_lowercase());
            collect_expr_reads_and_calls(from, reads, calls, false);
            collect_expr_reads_and_calls(to, reads, calls, false);
            if let Some(s) = step {
                collect_expr_reads_and_calls(s, reads, calls, false);
            }
            if body.is_empty() {
                ctx.note(
                    "empty-block",
                    "semantic",
                    stmt.span,
                    "empty FOR loop body".into(),
                    None,
                );
            }
            scan_stmt_list_for_lint(body, reads, calls, locals, ctx);
        }
        StmtKind::While { cond, body } => {
            collect_expr_reads_and_calls(cond, reads, calls, false);
            if body.is_empty() {
                ctx.note(
                    "empty-block",
                    "semantic",
                    stmt.span,
                    "empty WHILE loop body".into(),
                    None,
                );
            }
            scan_stmt_list_for_lint(body, reads, calls, locals, ctx);
        }
        StmtKind::Repeat { body, until } => {
            if body.is_empty() {
                ctx.note(
                    "empty-block",
                    "semantic",
                    stmt.span,
                    "empty REPEAT loop body".into(),
                    None,
                );
            }
            scan_stmt_list_for_lint(body, reads, calls, locals, ctx);
            collect_expr_reads_and_calls(until, reads, calls, false);
        }
        StmtKind::Return(e) | StmtKind::Break(e) => {
            if let Some(e) = e {
                collect_expr_reads_and_calls(e, reads, calls, false);
            }
        }
        StmtKind::Continue | StmtKind::Kill => {}
    }
}

fn scan_stmt_list_for_lint(
    stmts: &[Stmt],
    reads: &mut BTreeSet<String>,
    calls: &mut BTreeSet<String>,
    locals: &mut Vec<(String, Span)>,
    ctx: &mut LintCtx<'_>,
) {
    let mut terminated = false;
    let mut reported_unreachable = false;
    for stmt in stmts {
        if terminated && !reported_unreachable {
            ctx.warn(
                "unreachable-code",
                stmt.span,
                "unreachable statement after RETURN/BREAK/CONTINUE/KILL".into(),
                Some("Remove dead statements or move them before the terminator".into()),
            );
            reported_unreachable = true;
        }
        scan_stmt_for_lint(stmt, reads, calls, locals, ctx);
        if matches!(
            stmt.kind,
            StmtKind::Return(_) | StmtKind::Break(_) | StmtKind::Continue | StmtKind::Kill
        ) {
            terminated = true;
        }
    }
}

/// Perform compiler validation + AST/token static lint checks on a PPL file.
pub fn lint_source(path: &Path, modules: &[String], deny_warnings: bool) -> Report {
    let started = Instant::now();
    let source = match load_source_file(path, None) {
        Ok(s) => s,
        Err((code, msg)) => return source_error_report("lint", path, code, msg, started),
    };
    let mut report = Report::new("lint", Some(path.to_owned()));
    report.source = Some(source.identity.clone());
    let file = SourceFile::new(&source.identity.display_name, &source.text);

    let (ast, parse_diags) = ppl_syntax::parse(&source.text);
    for d in &parse_diags.items {
        report.diagnostics.push(to_diagnostic(&file, d));
    }

    let mut compile_errors = parse_diags.has_errors();
    if !parse_diags.has_errors() {
        match compile_with_modules(path, modules, "lint", started) {
            Ok((_, _, _, comp_diags)) => {
                report.diagnostics = comp_diags;
            }
            Err(err_report) => {
                report.diagnostics = err_report.diagnostics;
                compile_errors = true;
            }
        }
    }

    let mut ctx = LintCtx {
        file: &file,
        diags: Vec::new(),
    };

    // Token-level checks: soft keyword casing
    let mut lex_diags = Diagnostics::default();
    let tokens = lex_with_trivia(&source.text, &mut lex_diags);
    for tok in &tokens {
        if let Some(ref spelled) = tok.soft
            && let Tok::Kw(kw) = tok.tok
        {
            ctx.note(
                "soft-keyword-casing",
                "semantic",
                tok.span,
                format!("lowercase or mixed-case keyword `{spelled}`"),
                Some(format!("Use canonical uppercase `{}`", kw.as_str())),
            );
        }
    }

    // Program-level symbols & calls
    let mut global_names = BTreeSet::new();
    let mut all_calls = BTreeSet::new();
    let mut fn_defs: Vec<&FunctionDef> = Vec::new();
    for item in &ast.items {
        match item {
            Item::Globals { vars, .. } => {
                for v in vars {
                    global_names.insert(v.name.name.to_ascii_lowercase());
                    if let Some(init) = &v.init {
                        let mut dummy_reads = BTreeSet::new();
                        collect_expr_reads_and_calls(init, &mut dummy_reads, &mut all_calls, false);
                    }
                }
            }
            Item::Function(f) => fn_defs.push(f),
            Item::CasBlock { span, .. } => {
                ctx.note(
                    "cas-block-unsupported",
                    "compatibility",
                    *span,
                    "#cas ... #end block is preserved but not executed by the native engine".into(),
                    Some("Move symbolic expressions to external CAS or test on calculator".into()),
                );
            }
            Item::ForwardDecl { .. } => {}
        }
    }

    let single_fn = fn_defs.len() == 1;
    for f in &fn_defs {
        if ppl_compiler::builtins::lookup_case_insensitive(&f.name.name).is_some() {
            ctx.note(
                "shadowed-builtin",
                "semantic",
                f.name.span,
                format!(
                    "function `{}` shares a name with a built-in command",
                    f.name.name
                ),
                Some("Consider renaming to avoid confusion with built-in functions".into()),
            );
        }
        let mut reads = BTreeSet::new();
        let mut locals = Vec::new();
        for p in &f.params {
            let low = p.name.to_ascii_lowercase();
            if global_names.contains(&low) {
                ctx.warn(
                    "shadowed-global",
                    p.span,
                    format!(
                        "parameter `{}` shadows a file-level global variable",
                        p.name
                    ),
                    None,
                );
            }
        }
        scan_stmt_list_for_lint(&f.body, &mut reads, &mut all_calls, &mut locals, &mut ctx);
        for (name, span) in locals {
            let low = name.to_ascii_lowercase();
            if global_names.contains(&low) {
                ctx.warn(
                    "shadowed-global",
                    span,
                    format!("local variable `{name}` shadows a file-level global variable"),
                    None,
                );
            }
            if ppl_compiler::builtins::lookup_case_insensitive(&name).is_some() {
                ctx.note(
                    "shadowed-builtin",
                    "semantic",
                    span,
                    format!("local variable `{name}` shares a name with a built-in command"),
                    None,
                );
            }
            if !reads.contains(&low) {
                ctx.warn(
                    "unused-local",
                    span,
                    format!("local variable `{name}` in `{}` is never read", f.name.name),
                    Some("Remove the unused local or read its value".into()),
                );
            }
        }
    }

    if !single_fn {
        for f in &fn_defs {
            if !f.exported && !f.is_key && !all_calls.contains(&f.name.name.to_ascii_lowercase()) {
                ctx.warn(
                    "unused-private-function",
                    f.name.span,
                    format!("private function `{}` is never called", f.name.name),
                    Some("Mark EXPORT if it is an entry point, or remove if unused".into()),
                );
            }
        }
    }

    report.diagnostics.extend(ctx.diags);
    report.diagnostics.sort_by_key(|d| {
        (
            d.location.as_ref().map(|l| l.line).unwrap_or(0),
            d.location.as_ref().map(|l| l.column).unwrap_or(0),
        )
    });
    report.compatibility_limitations = report
        .diagnostics
        .iter()
        .filter(|d| d.class == "compatibility")
        .map(|d| d.message.clone())
        .collect();

    let mut errors = 0usize;
    let mut warnings = 0usize;
    let mut notes = 0usize;
    let mut by_code: BTreeMap<String, usize> = BTreeMap::new();
    for d in &report.diagnostics {
        match d.severity.as_str() {
            "error" => errors += 1,
            "warning" => warnings += 1,
            _ => notes += 1,
        }
        *by_code.entry(d.code.clone()).or_insert(0) += 1;
    }
    report.data = json!({
        "summary": {
            "errors": errors,
            "warnings": warnings,
            "notes": notes,
            "by_code": by_code,
        }
    });

    if compile_errors || errors > 0 {
        report.fail("invalid_source", 1, "lint found syntax or semantic errors");
    } else if deny_warnings && warnings > 0 {
        report.fail(
            "policy_failed",
            1,
            format!("lint rejected {warnings} warning(s)"),
        );
    }
    report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
    report
}

// --- Syntax-Aware Formatter ---

#[derive(Clone, Debug)]
pub struct FormatResult {
    pub original: String,
    pub formatted: String,
    pub changed: bool,
    pub diff_lines: Vec<String>,
}

/// Format PPL source deterministically using the upstream trivia lexer and verify re-parse + idempotence.
pub fn format_source(
    path: &Path,
    indent_width: usize,
    uppercase_keywords: bool,
) -> Result<FormatResult, Box<Report>> {
    let started = Instant::now();
    if !(1..=8).contains(&indent_width) {
        return Err(Box::new(source_error_report(
            "format",
            path,
            2,
            "indent width must be between 1 and 8".into(),
            started,
        )));
    }
    let source = match load_source_file(path, None) {
        Ok(s) => s,
        Err((code, msg)) => {
            return Err(Box::new(source_error_report(
                "format", path, code, msg, started,
            )));
        }
    };
    let file = SourceFile::new(&source.identity.display_name, &source.text);
    let (_, parse_diags) = ppl_syntax::parse(&source.text);
    if parse_diags.has_errors() {
        let mut report = Report::new("format", Some(path.to_owned()));
        report.source = Some(source.identity);
        report.diagnostics = parse_diags
            .items
            .iter()
            .map(|d| to_diagnostic(&file, d))
            .collect();
        report.fail(
            "invalid_source",
            1,
            "cannot format source with syntax errors",
        );
        report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
        return Err(Box::new(report));
    }

    let formatted =
        format_ppl_text(&source.text, indent_width, uppercase_keywords).map_err(|e| {
            let mut report = Report::new("format", Some(path.to_owned()));
            report.source = Some(source.identity.clone());
            report.fail("infrastructure_error", 70, e);
            report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
            Box::new(report)
        })?;

    // Verify re-parse & idempotence before returning formatted code.
    let (_, recheck) = ppl_syntax::parse(&formatted);
    if recheck.has_errors() {
        let mut report = Report::new("format", Some(path.to_owned()));
        report.source = Some(source.identity);
        report.fail(
            "infrastructure_error",
            70,
            "formatter produced syntax that failed verification re-parse",
        );
        report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
        return Err(Box::new(report));
    }
    let second = format_ppl_text(&formatted, indent_width, uppercase_keywords).unwrap_or_default();
    if second != formatted {
        let mut report = Report::new("format", Some(path.to_owned()));
        report.source = Some(source.identity);
        report.fail(
            "infrastructure_error",
            70,
            "formatter output was not idempotent",
        );
        report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
        return Err(Box::new(report));
    }

    let changed = formatted != source.text;
    let diff_lines = if changed {
        build_simple_diff(&source.text, &formatted)
    } else {
        vec![]
    };
    Ok(FormatResult {
        original: source.text,
        formatted,
        changed,
        diff_lines,
    })
}

fn build_simple_diff(old: &str, new: &str) -> Vec<String> {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let max = old_lines.len().max(new_lines.len());
    let mut out = Vec::new();
    for i in 0..max {
        match (old_lines.get(i), new_lines.get(i)) {
            (Some(a), Some(b)) if a == b => {}
            (Some(a), Some(b)) => {
                out.push(format!("L{}: - {a}", i + 1));
                out.push(format!("L{}: + {b}", i + 1));
            }
            (Some(a), None) => out.push(format!("L{}: - {a}", i + 1)),
            (None, Some(b)) => out.push(format!("L{}: + {b}", i + 1)),
            (None, None) => {}
        }
    }
    out
}

fn format_ppl_text(
    src: &str,
    indent_width: usize,
    uppercase_keywords: bool,
) -> Result<String, String> {
    let mut lex_diags = Diagnostics::default();
    let tokens: Vec<Token> = lex_with_trivia(src, &mut lex_diags)
        .into_iter()
        .filter(|t| !matches!(t.tok, Tok::Eof))
        .collect();
    if lex_diags.has_errors() {
        return Err("lexer error during formatting".into());
    }

    let mut out = String::with_capacity(src.len() + 64);
    let mut indent: usize = 0;
    let mut paren_depth: usize = 0;
    let mut line_start = true;
    let mut prev_tok: Option<&Token> = None;
    let mut pending_else_if = false;

    let slice_of = |t: &Token| -> String {
        let raw = &src[t.span.start as usize..t.span.end as usize];
        if uppercase_keywords
            && let Tok::Kw(kw) = t.tok
            && t.soft.is_none()
        {
            return kw.as_str().to_owned();
        }
        if uppercase_keywords
            && let Tok::Kw(kw) = t.tok
            && t.soft.is_some()
        {
            // Only uppercase soft keywords that act as structural block keywords.
            let is_structural = matches!(
                kw,
                Keyword::Begin
                    | Keyword::End
                    | Keyword::If
                    | Keyword::Then
                    | Keyword::Else
                    | Keyword::Case
                    | Keyword::Default
                    | Keyword::Iferr
                    | Keyword::For
                    | Keyword::From
                    | Keyword::To
                    | Keyword::Downto
                    | Keyword::Step
                    | Keyword::Do
                    | Keyword::While
                    | Keyword::Repeat
                    | Keyword::Until
                    | Keyword::Return
                    | Keyword::Break
                    | Keyword::Continue
                    | Keyword::Kill
                    | Keyword::Export
                    | Keyword::Local
                    | Keyword::Key
                    | Keyword::And
                    | Keyword::Or
                    | Keyword::Xor
                    | Keyword::Not
                    | Keyword::Mod
                    | Keyword::Div
            );
            if is_structural {
                return kw.as_str().to_owned();
            }
        }
        raw.to_owned()
    };

    let push_indent = |out: &mut String, indent: usize, width: usize| {
        for _ in 0..(indent * width) {
            out.push(' ');
        }
    };

    let newline = |out: &mut String, line_start: &mut bool| {
        while out.ends_with(' ') {
            out.pop();
        }
        if !out.ends_with('\n') && !out.is_empty() {
            out.push('\n');
        }
        *line_start = true;
    };

    for i in 0..tokens.len() {
        let tok = &tokens[i];
        let next = tokens.get(i + 1);
        let text = slice_of(tok);

        match &tok.tok {
            Tok::Comment => {
                if !line_start {
                    out.push(' ');
                } else {
                    push_indent(&mut out, indent, indent_width);
                }
                out.push_str(&text);
                newline(&mut out, &mut line_start);
                prev_tok = Some(tok);
                continue;
            }
            Tok::Pragma(_) | Tok::CasBlock(_) => {
                if !line_start {
                    newline(&mut out, &mut line_start);
                }
                push_indent(&mut out, indent, indent_width);
                out.push_str(text.trim_end());
                newline(&mut out, &mut line_start);
                prev_tok = Some(tok);
                continue;
            }
            Tok::Kw(Keyword::End) => {
                if !line_start {
                    newline(&mut out, &mut line_start);
                }
                indent = indent.saturating_sub(1);
                push_indent(&mut out, indent, indent_width);
                out.push_str(&text);
                line_start = false;
                // Keep END; on the same line if followed by `;`
                if !matches!(next.map(|n| &n.tok), Some(Tok::Semi)) {
                    newline(&mut out, &mut line_start);
                }
                prev_tok = Some(tok);
                continue;
            }
            Tok::Kw(Keyword::Until) => {
                if !line_start {
                    newline(&mut out, &mut line_start);
                }
                indent = indent.saturating_sub(1);
                push_indent(&mut out, indent, indent_width);
                out.push_str(&text);
                line_start = false;
                prev_tok = Some(tok);
                continue;
            }
            Tok::Kw(Keyword::Else) => {
                if !line_start {
                    newline(&mut out, &mut line_start);
                }
                indent = indent.saturating_sub(1);
                push_indent(&mut out, indent, indent_width);
                out.push_str(&text);
                line_start = false;
                if matches!(next.map(|n| &n.tok), Some(Tok::Kw(Keyword::If))) {
                    pending_else_if = true;
                } else {
                    indent += 1;
                    if !matches!(next.map(|n| &n.tok), Some(Tok::Comment)) {
                        newline(&mut out, &mut line_start);
                    }
                }
                prev_tok = Some(tok);
                continue;
            }
            Tok::Kw(Keyword::Default) => {
                if !line_start {
                    newline(&mut out, &mut line_start);
                }
                push_indent(&mut out, indent.saturating_sub(1), indent_width);
                out.push_str(&text);
                line_start = false;
                if !matches!(next.map(|n| &n.tok), Some(Tok::Comment)) {
                    newline(&mut out, &mut line_start);
                }
                prev_tok = Some(tok);
                continue;
            }
            Tok::Kw(Keyword::Begin) => {
                if !line_start {
                    newline(&mut out, &mut line_start);
                }
                push_indent(&mut out, indent, indent_width);
                out.push_str(&text);
                indent += 1;
                line_start = false;
                if !matches!(next.map(|n| &n.tok), Some(Tok::Comment)) {
                    newline(&mut out, &mut line_start);
                }
                prev_tok = Some(tok);
                continue;
            }
            Tok::Kw(Keyword::Repeat | Keyword::Case | Keyword::Iferr) => {
                if !line_start {
                    newline(&mut out, &mut line_start);
                }
                push_indent(&mut out, indent, indent_width);
                out.push_str(&text);
                indent += 1;
                line_start = false;
                if !matches!(next.map(|n| &n.tok), Some(Tok::Comment)) {
                    newline(&mut out, &mut line_start);
                }
                prev_tok = Some(tok);
                continue;
            }
            Tok::Kw(Keyword::Then | Keyword::Do) => {
                if line_start {
                    push_indent(&mut out, indent, indent_width);
                } else {
                    out.push(' ');
                }
                out.push_str(&text);
                indent += 1;
                pending_else_if = false;
                line_start = false;
                if !matches!(next.map(|n| &n.tok), Some(Tok::Comment)) {
                    newline(&mut out, &mut line_start);
                }
                prev_tok = Some(tok);
                continue;
            }
            Tok::Kw(Keyword::If) if pending_else_if => {
                out.push(' ');
                out.push_str(&text);
                line_start = false;
                prev_tok = Some(tok);
                continue;
            }
            Tok::Semi => {
                out.push(';');
                if paren_depth == 0 && !matches!(next.map(|n| &n.tok), Some(Tok::Comment)) {
                    newline(&mut out, &mut line_start);
                    // Separate top-level function definitions with one blank line.
                    if indent == 0
                        && matches!(prev_tok.map(|p| &p.tok), Some(Tok::Kw(Keyword::End)))
                        && next.is_some()
                    {
                        out.push('\n');
                    }
                } else {
                    line_start = false;
                }
                prev_tok = Some(tok);
                continue;
            }
            _ => {}
        }

        if line_start {
            push_indent(&mut out, indent, indent_width);
            line_start = false;
        } else if needs_space(prev_tok, tok) {
            out.push(' ');
        }

        if matches!(tok.tok, Tok::LParen | Tok::LBracket | Tok::LBrace) {
            paren_depth += 1;
        } else if matches!(tok.tok, Tok::RParen | Tok::RBracket | Tok::RBrace) {
            paren_depth = paren_depth.saturating_sub(1);
        }

        out.push_str(&text);
        prev_tok = Some(tok);
    }

    while out.ends_with('\n') {
        out.pop();
    }
    if !out.is_empty() {
        out.push('\n');
    }
    Ok(out)
}

fn needs_space(prev: Option<&Token>, curr: &Token) -> bool {
    let Some(prev) = prev else {
        return false;
    };
    if matches!(
        prev.tok,
        Tok::LParen | Tok::LBracket | Tok::LBrace | Tok::Sqrt | Tok::Comment
    ) {
        return false;
    }
    if matches!(
        curr.tok,
        Tok::RParen
            | Tok::RBracket
            | Tok::RBrace
            | Tok::Comma
            | Tok::Semi
            | Tok::Bang
            | Tok::Squared
            | Tok::Cubed
    ) {
        return false;
    }
    // Function call `foo(x)` or indexing `arr[1]`: no space between ident/close and `(`/ `[`
    if matches!(curr.tok, Tok::LParen | Tok::LBracket)
        && matches!(
            prev.tok,
            Tok::Ident(_) | Tok::RParen | Tok::RBracket | Tok::Kw(_)
        )
    {
        // Exception: `RETURN (x)` or `IF (x)` or `WHILE (x)` or `UNTIL (x)` or `AND`/`OR`/`XOR`/`NOT`
        if let Tok::Kw(kw) = prev.tok {
            return matches!(
                kw,
                Keyword::If
                    | Keyword::While
                    | Keyword::Until
                    | Keyword::Return
                    | Keyword::And
                    | Keyword::Or
                    | Keyword::Xor
                    | Keyword::Not
                    | Keyword::Case
            );
        }
        return false;
    }
    // Unary minus after an operator or opening delimiter or keyword
    if matches!(prev.tok, Tok::Minus) {
        return false;
    }
    if matches!(curr.tok, Tok::Minus)
        && matches!(
            prev.tok,
            Tok::Assign
                | Tok::Store
                | Tok::Comma
                | Tok::LParen
                | Tok::LBracket
                | Tok::LBrace
                | Tok::Eq
                | Tok::EqEq
                | Tok::Ne
                | Tok::Lt
                | Tok::Le
                | Tok::Gt
                | Tok::Ge
                | Tok::Plus
                | Tok::Minus
                | Tok::Star
                | Tok::Slash
                | Tok::Caret
                | Tok::Kw(_)
        )
    {
        return true;
    }
    true
}

// --- Program Inspect, Compatibility Audit, Command Catalog, Docs ---

/// Inspect `.ppl` or binary `.hpprgm` container metadata, AST summary, and bytecode metrics.
pub fn inspect_program(path: &Path, modules: &[String]) -> Report {
    let started = Instant::now();
    let raw_bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => return source_error_report("inspect", path, 5, e.to_string(), started),
    };
    let is_container = raw_bytes.starts_with(&hpprgm::MAGIC.to_le_bytes());
    let container_info = if is_container {
        match hpprgm::parse(&raw_bytes) {
            Ok(container) => {
                let exports: Vec<Value> = container
                    .exports()
                    .into_iter()
                    .map(|(name, flags)| json!({"name": name, "flags": flags}))
                    .collect();
                let sections: Vec<Value> = container
                    .objects
                    .iter()
                    .map(|o| {
                        let kind = match o.ty {
                            hpprgm::SECTION_EXPORTS => "exports",
                            hpprgm::SECTION_VARIABLES => "variables",
                            hpprgm::SECTION_FILES => "files",
                            hpprgm::SECTION_TABLE => "table",
                            _ => "header_or_other",
                        };
                        json!({"type": format!("{:#010x}", o.ty), "kind": kind, "offset": o.offset})
                    })
                    .collect();
                let main_name = container.source().ok().map(|s| s.name);
                Some(json!({
                    "format": "hpprgm-binary",
                    "raw_size_bytes": raw_bytes.len(),
                    "header_ints": container.header_ints(),
                    "main_file": main_name,
                    "exports": exports,
                    "sections": sections,
                }))
            }
            Err(e) => {
                return source_error_report("inspect", path, 1, e.to_string(), started);
            }
        }
    } else {
        None
    };

    let (source, file, program, diags) =
        match compile_with_modules(path, modules, "inspect", started) {
            Ok(v) => v,
            Err(r) => return *r,
        };
    let (ast, _) = ppl_syntax::parse(&source.text);
    let mut report = Report::new("inspect", Some(path.to_owned()));
    report.source = Some(source.identity);
    report.compatibility_limitations = diags
        .iter()
        .filter(|d| d.class == "compatibility")
        .map(|d| d.message.clone())
        .collect();
    report.diagnostics = diags;

    let exported_fns = program.functions.iter().filter(|f| f.exported).count();
    let total_bytecode: usize = std::iter::once(&program.init)
        .chain(program.functions.iter())
        .map(|c| c.code.len())
        .sum();
    let cas_blocks = ast
        .items
        .iter()
        .filter(|i| matches!(i, Item::CasBlock { .. }))
        .count();
    let line_count = source.text.lines().count();
    let char_count = source.text.chars().count();
    let _ = file;

    report.data = json!({
        "container": container_info,
        "source_metrics": {
            "lines": line_count,
            "chars": char_count,
            "raw_bytes": raw_bytes.len(),
            "pragmas": ast.pragmas.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>(),
            "cas_blocks": cas_blocks,
        },
        "compiled_metrics": {
            "functions_total": program.functions.len(),
            "functions_exported": exported_fns,
            "globals_total": program.globals.len(),
            "program_globals": program.globals.iter().filter(|g| g.program_var).count(),
            "consts_total": program.consts.len(),
            "bytecode_instructions": total_bytecode,
            "unsupported_names": program.names,
        },
        "compatibility_limitations": report.compatibility_limitations,
    });
    report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
    report
}

fn collect_calls_from_ast(
    program: &Program,
    file: &SourceFile,
    calls: &mut BTreeMap<String, Vec<Location>>,
) {
    fn walk_expr(expr: &Expr, file: &SourceFile, calls: &mut BTreeMap<String, Vec<Location>>) {
        match &expr.kind {
            ExprKind::Call(callee, args) => {
                if let ExprKind::Ident(name) = &callee.kind {
                    calls
                        .entry(name.clone())
                        .or_default()
                        .push(location(file, expr.span));
                } else {
                    walk_expr(callee, file, calls);
                }
                for a in args {
                    walk_expr(a, file, calls);
                }
            }
            ExprKind::List(xs) | ExprKind::Bracket(xs) => {
                for x in xs {
                    walk_expr(x, file, calls);
                }
            }
            ExprKind::ComplexPair(a, b)
            | ExprKind::Binary(_, a, b)
            | ExprKind::Assign(a, b)
            | ExprKind::Store(a, b) => {
                walk_expr(a, file, calls);
                walk_expr(b, file, calls);
            }
            ExprKind::Unary(_, e) | ExprKind::Postfix(_, e) | ExprKind::Quoted(e) => {
                walk_expr(e, file, calls);
            }
            ExprKind::Index(base, idx) => {
                walk_expr(base, file, calls);
                for i in idx {
                    walk_expr(i, file, calls);
                }
            }
            ExprKind::Number(_)
            | ExprKind::HashInt(_)
            | ExprKind::Str(_)
            | ExprKind::Ident(_)
            | ExprKind::Error => {}
        }
    }
    fn walk_stmt(stmt: &Stmt, file: &SourceFile, calls: &mut BTreeMap<String, Vec<Location>>) {
        match &stmt.kind {
            StmtKind::Expr(e) => walk_expr(e, file, calls),
            StmtKind::Local(vars) => {
                for v in vars {
                    if let Some(init) = &v.init {
                        walk_expr(init, file, calls);
                    }
                }
            }
            StmtKind::Block(b) => {
                for s in b {
                    walk_stmt(s, file, calls);
                }
            }
            StmtKind::If {
                cond,
                then_body,
                else_body,
            } => {
                walk_expr(cond, file, calls);
                for s in then_body {
                    walk_stmt(s, file, calls);
                }
                if let Some(eb) = else_body {
                    for s in eb {
                        walk_stmt(s, file, calls);
                    }
                }
            }
            StmtKind::Case { arms, default } => {
                for (c, b) in arms {
                    walk_expr(c, file, calls);
                    for s in b {
                        walk_stmt(s, file, calls);
                    }
                }
                if let Some(d) = default {
                    for s in d {
                        walk_stmt(s, file, calls);
                    }
                }
            }
            StmtKind::IfErr {
                body,
                handler,
                else_body,
            } => {
                for s in body {
                    walk_stmt(s, file, calls);
                }
                for s in handler {
                    walk_stmt(s, file, calls);
                }
                if let Some(eb) = else_body {
                    for s in eb {
                        walk_stmt(s, file, calls);
                    }
                }
            }
            StmtKind::For {
                from,
                to,
                step,
                body,
                ..
            } => {
                walk_expr(from, file, calls);
                walk_expr(to, file, calls);
                if let Some(s) = step {
                    walk_expr(s, file, calls);
                }
                for s in body {
                    walk_stmt(s, file, calls);
                }
            }
            StmtKind::While { cond, body } => {
                walk_expr(cond, file, calls);
                for s in body {
                    walk_stmt(s, file, calls);
                }
            }
            StmtKind::Repeat { body, until } => {
                for s in body {
                    walk_stmt(s, file, calls);
                }
                walk_expr(until, file, calls);
            }
            StmtKind::Return(Some(e)) | StmtKind::Break(Some(e)) => walk_expr(e, file, calls),
            _ => {}
        }
    }

    for item in &program.items {
        match item {
            Item::Function(f) => {
                for s in &f.body {
                    walk_stmt(s, file, calls);
                }
            }
            Item::Globals { vars, .. } => {
                for v in vars {
                    if let Some(init) = &v.init {
                        walk_expr(init, file, calls);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Analyze HP Prime compatibility for a PPL program against the upstream builtin and catalog tables.
pub fn analyze_compat(path: &Path, modules: &[String], strict: bool) -> Report {
    let started = Instant::now();
    let (source, file, program, diags) =
        match compile_with_modules(path, modules, "compat", started) {
            Ok(v) => v,
            Err(r) => return *r,
        };
    let (ast, _) = ppl_syntax::parse(&source.text);
    let mut report = Report::new("compat", Some(path.to_owned()));
    report.source = Some(source.identity);
    report.compatibility_limitations = diags
        .iter()
        .filter(|d| d.class == "compatibility")
        .map(|d| d.message.clone())
        .collect();
    report.diagnostics = diags;

    let user_fns: BTreeSet<String> = program
        .functions
        .iter()
        .map(|f| f.name.to_ascii_lowercase())
        .collect();
    let mut call_sites: BTreeMap<String, Vec<Location>> = BTreeMap::new();
    collect_calls_from_ast(&ast, &file, &mut call_sites);

    let mut commands = Vec::new();
    let mut implemented_count = 0usize;
    let mut approximate_count = 0usize;
    let mut unsupported_count = 0usize;
    let mut catalog_count = 0usize;

    for (name, locs) in &call_sites {
        if user_fns.contains(&name.to_ascii_lowercase()) {
            continue;
        }
        if let Some(spec) = ppl_compiler::builtins::lookup_case_insensitive(name) {
            match spec.status {
                Status::Implemented => implemented_count += 1,
                Status::Approximate => approximate_count += 1,
                Status::Unsupported => unsupported_count += 1,
            }
            commands.push(json!({
                "name": spec.name,
                "used_as": name,
                "category": spec.category.as_str(),
                "status": spec.status.as_str(),
                "source": "builtin",
                "locations": locs,
            }));
        } else if let Some(cat) = catalog::lookup_case_insensitive(name) {
            catalog_count += 1;
            commands.push(json!({
                "name": cat.name,
                "used_as": name,
                "category": match cat.kind {
                    CatalogKind::Command => "catalog-command",
                    CatalogKind::Cas => "catalog-cas",
                },
                "status": "unsupported",
                "source": "catalog",
                "locations": locs,
            }));
        }
    }

    let cas_blocks: Vec<Location> = ast
        .items
        .iter()
        .filter_map(|i| match i {
            Item::CasBlock { span, .. } => Some(location(&file, *span)),
            _ => None,
        })
        .collect();

    let compatible = unsupported_count == 0 && catalog_count == 0 && cas_blocks.is_empty();
    let fully_native = compatible && approximate_count == 0;
    report.data = json!({
        "compatible": compatible,
        "fully_native": fully_native,
        "implemented_count": implemented_count,
        "approximate_count": approximate_count,
        "unsupported_count": unsupported_count,
        "catalog_count": catalog_count,
        "cas_block_count": cas_blocks.len(),
        "cas_blocks": cas_blocks,
        "commands": commands,
    });
    if strict && !compatible {
        report.fail(
            "policy_failed",
            1,
            format!(
                "compatibility check failed: {unsupported_count} unsupported builtin(s), {catalog_count} catalog command(s), {} CAS block(s)",
                cas_blocks.len()
            ),
        );
    }
    report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
    report
}

/// List or query the upstream HP Prime command tables (`BUILTINS` and optional `catalog`).
pub fn list_commands(
    category: Option<&str>,
    status: Option<&str>,
    search: Option<&str>,
    include_catalog: bool,
) -> Report {
    let started = Instant::now();
    let mut report = Report::new("compat", None);
    let needle = search.map(|s| s.to_ascii_lowercase());
    let mut entries = Vec::new();
    for spec in BUILTINS {
        if category.is_some_and(|c| !c.eq_ignore_ascii_case(spec.category.as_str())) {
            continue;
        }
        if status.is_some_and(|s| !s.eq_ignore_ascii_case(spec.status.as_str())) {
            continue;
        }
        if needle
            .as_ref()
            .is_some_and(|q| !spec.name.to_ascii_lowercase().contains(q))
        {
            continue;
        }
        entries.push(json!({
            "name": spec.name,
            "category": spec.category.as_str(),
            "arity": spec.arity_text(),
            "status": spec.status.as_str(),
            "source": "builtin",
            "ref_args": spec.ref_args,
        }));
    }
    if include_catalog && status.is_none_or(|s| s.eq_ignore_ascii_case("unsupported")) {
        for cat in catalog::entries() {
            let cat_label = match cat.kind {
                CatalogKind::Command => "catalog",
                CatalogKind::Cas => "cas",
            };
            if category.is_some_and(|c| !c.eq_ignore_ascii_case(cat_label)) {
                continue;
            }
            if needle
                .as_ref()
                .is_some_and(|q| !cat.name.to_ascii_lowercase().contains(q))
            {
                continue;
            }
            entries.push(json!({
                "name": cat.name,
                "category": cat_label,
                "arity": "documented",
                "status": "unsupported",
                "source": catalog::SOURCE,
                "ref_args": 0,
            }));
        }
    }
    report.data = json!({
        "count": entries.len(),
        "builtins_total": BUILTINS.len(),
        "implemented": BUILTINS.iter().filter(|b| b.status == Status::Implemented).count(),
        "approximate": BUILTINS.iter().filter(|b| b.status == Status::Approximate).count(),
        "unsupported": BUILTINS.iter().filter(|b| b.status == Status::Unsupported).count(),
        "catalog_total": catalog::entries().count(),
        "commands": entries,
    });
    report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
    report
}

/// Extract doc comments (`// ...`) paired with exported/local functions and globals in a PPL file.
pub fn extract_docs(path: &Path) -> Report {
    let started = Instant::now();
    let source = match load_source_file(path, None) {
        Ok(s) => s,
        Err((code, msg)) => return source_error_report("docs", path, code, msg, started),
    };
    let mut report = Report::new("docs", Some(path.to_owned()));
    report.source = Some(source.identity.clone());
    let file = SourceFile::new(&source.identity.display_name, &source.text);
    let (ast, diags) = ppl_syntax::parse(&source.text);
    report.diagnostics = diags
        .items
        .iter()
        .map(|d| to_diagnostic(&file, d))
        .collect();
    if diags.has_errors() {
        report.fail("invalid_source", 1, "syntax parsing reported errors");
        report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
        return report;
    }

    let lines: Vec<&str> = source.text.lines().collect();
    let leading_doc = |start_line: u32| -> Vec<String> {
        let mut idx = start_line.saturating_sub(2) as isize;
        let mut comments = Vec::new();
        while idx >= 0 {
            let trimmed = lines[idx as usize].trim();
            if let Some(rest) = trimmed.strip_prefix("//") {
                comments.push(rest.trim_start().to_owned());
                idx -= 1;
            } else {
                break;
            }
        }
        comments.reverse();
        comments
    };

    let mut md = format!("# PPL API Reference: `{}`\n\n", file.name);
    let mut symbols = Vec::new();
    for item in &ast.items {
        match item {
            Item::Function(f) => {
                let loc = location(&file, f.span);
                let docs = leading_doc(loc.line);
                let kind = if f.is_key {
                    "KEY"
                } else if f.exported {
                    "EXPORT"
                } else if f.is_local {
                    "LOCAL"
                } else {
                    "PRIVATE"
                };
                let params: Vec<String> = f.params.iter().map(|p| p.name.clone()).collect();
                let sig = format!("{kind} {}({})", f.name.name, params.join(", "));
                let _ = writeln!(md, "## `{sig}`\n");
                let _ = writeln!(md, "- Defined at `{}:{}`\n", loc.file, loc.line);
                if !docs.is_empty() {
                    let _ = writeln!(md, "{}\n", docs.join("\n"));
                }
                symbols.push(json!({
                    "kind": kind,
                    "name": f.name.name,
                    "params": params,
                    "signature": sig,
                    "docs": docs,
                    "location": loc,
                }));
            }
            Item::Globals {
                exported,
                vars,
                span,
            } => {
                let loc = location(&file, *span);
                let docs = leading_doc(loc.line);
                let kind = if *exported { "EXPORT" } else { "LOCAL" };
                for v in vars {
                    let vloc = location(&file, v.name.span);
                    let _ = writeln!(md, "- Global `{kind} {}` (line {})", v.name.name, vloc.line);
                    symbols.push(json!({
                        "kind": format!("{kind}_GLOBAL"),
                        "name": v.name.name,
                        "docs": docs,
                        "location": vloc,
                    }));
                }
                if !vars.is_empty() {
                    md.push('\n');
                }
            }
            _ => {}
        }
    }

    report.data = json!({
        "symbols": symbols,
        "markdown": md,
    });
    report.duration_ms = started.elapsed().as_secs_f64() * 1000.;
    report
}

/// Write extracted or looked-up documentation Markdown atomically when `--output` is requested.
pub fn write_docs_output(
    output: &Path,
    input: Option<&Path>,
    markdown: &str,
) -> Result<(), String> {
    if let Some(src) = input {
        protect_output(output, &[src.to_path_buf()])?;
    }
    atomic_bytes(output, markdown.as_bytes())
        .map_err(|e| format!("cannot write {}: {e}", output.display()))
}

pub fn builtin_summary_counts() -> (usize, usize, usize, usize, usize) {
    let total = BUILTINS.len();
    let implemented = BUILTINS
        .iter()
        .filter(|b| b.status == Status::Implemented)
        .count();
    let approximate = BUILTINS
        .iter()
        .filter(|b| b.status == Status::Approximate)
        .count();
    let unsupported = BUILTINS
        .iter()
        .filter(|b| b.status == Status::Unsupported)
        .count();
    let catalog = catalog::entries().count();
    (total, implemented, approximate, unsupported, catalog)
}

pub fn format_builtin_variant_name(b: Builtin) -> &'static str {
    ppl_compiler::builtins::spec(b).name
}
