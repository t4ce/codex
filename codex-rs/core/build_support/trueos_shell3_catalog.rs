use std::collections::HashMap;
use std::fs;
use std::path::Path;

use syn::Expr;
use syn::ExprArray;
use syn::ExprCall;
use syn::ExprLit;
use syn::ExprReference;
use syn::ExprStruct;
use syn::Item;
use syn::Lit;
use syn::Member;

#[derive(Clone)]
struct Group {
    mode: String,
    label: String,
    names: Vec<String>,
}

pub fn generate(registry: &Path, output: &Path) -> Result<(), String> {
    let source = fs::read_to_string(registry)
        .map_err(|error| format!("cannot read {}: {error}", registry.display()))?;
    let syntax = syn::parse_file(&source)
        .map_err(|error| format!("cannot parse {}: {error}", registry.display()))?;
    let mut consts = HashMap::new();
    for item in syntax.items {
        if let Item::Const(item) = item {
            consts.insert(item.ident.to_string(), *item.expr);
        }
    }

    let mut groups = Vec::new();
    for (mode, constant) in [
        ("HV", "HV_GROUPS"),
        ("CMD", "CMD_GROUPS"),
        ("ADM", "ADM_GROUPS"),
    ] {
        let expr = consts
            .get(constant)
            .ok_or_else(|| format!("registry is missing {constant}"))?;
        let array = expr_array(expr)
            .ok_or_else(|| format!("{constant} must be an array of NameGroup values"))?;
        for value in &array.elems {
            groups.push(parse_group(value, mode, &consts)?);
        }
    }
    let vme = consts
        .get("VME_GROUP")
        .ok_or_else(|| "registry is missing VME_GROUP".to_string())?;
    groups.push(parse_group(vme, "VMX", &consts)?);

    let mut generated = String::from("pub const SHELL3_GROUPS: &[(&str, &str, &[&str])] = &[\n");
    for group in groups {
        generated.push_str(&format!("    ({:?}, {:?}, &[", group.mode, group.label));
        for name in group.names {
            generated.push_str(&format!("{:?},", name));
        }
        generated.push_str("]),\n");
    }
    generated.push_str("];\n");
    fs::write(output, generated)
        .map_err(|error| format!("cannot write {}: {error}", output.display()))
}

fn parse_group(
    expression: &Expr,
    mode: &str,
    consts: &HashMap<String, Expr>,
) -> Result<Group, String> {
    let fields = match expression {
        Expr::Struct(ExprStruct { fields, .. }) => fields,
        other => {
            return Err(format!(
                "expected NameGroup struct, got {}",
                syntax_kind(other)
            ));
        }
    };
    let mut label = None;
    let mut names_expr = None;
    for field in fields {
        let Member::Named(name) = &field.member else {
            return Err("NameGroup uses a non-named field".into());
        };
        match name.to_string().as_str() {
            "name" => label = Some(string_literal(&field.expr, "NameGroup.name")?),
            "names" => names_expr = Some(&field.expr),
            _ => {}
        }
    }
    let label = label.ok_or_else(|| "NameGroup is missing name".to_string())?;
    let names_expr = names_expr.ok_or_else(|| "NameGroup is missing names".to_string())?;
    let names = resolve_names(names_expr, consts)?;
    Ok(Group {
        mode: mode.to_string(),
        label,
        names,
    })
}

fn resolve_names(expression: &Expr, consts: &HashMap<String, Expr>) -> Result<Vec<String>, String> {
    match expression {
        Expr::Reference(ExprReference { expr, .. }) => resolve_names(expr, consts),
        Expr::Array(array) => parse_name_entries(array),
        Expr::Path(path) if path.path.segments.len() == 1 => {
            let name = path.path.segments[0].ident.to_string();
            let value = consts
                .get(&name)
                .ok_or_else(|| format!("NameGroup references unknown registry constant {name}"))?;
            resolve_names(value, consts)
        }
        other => Err(format!(
            "unsupported NameGroup.names expression: {}",
            syntax_kind(other)
        )),
    }
}

fn parse_name_entries(array: &ExprArray) -> Result<Vec<String>, String> {
    array.elems.iter().map(parse_name_entry).collect()
}

fn parse_name_entry(expression: &Expr) -> Result<String, String> {
    match expression {
        Expr::Struct(ExprStruct { fields, .. }) => {
            for field in fields {
                if matches!(&field.member, Member::Named(name) if name == "name") {
                    return string_literal(&field.expr, "NameEntry.name");
                }
            }
            Err("NameEntry is missing name".into())
        }
        other => Err(format!(
            "expected NameEntry struct, got {}",
            syntax_kind(other)
        )),
    }
}

fn string_literal(expression: &Expr, field: &str) -> Result<String, String> {
    let Expr::Lit(ExprLit {
        lit: Lit::Str(value),
        ..
    }) = expression
    else {
        return Err(format!("{field} must be a string literal"));
    };
    Ok(value.value())
}

fn expr_array(expression: &Expr) -> Option<&ExprArray> {
    match expression {
        Expr::Array(array) => Some(array),
        Expr::Reference(reference) => expr_array(&reference.expr),
        _ => None,
    }
}

fn syntax_kind(expression: &Expr) -> &'static str {
    match expression {
        Expr::Array(_) => "array",
        Expr::Call(ExprCall { .. }) => "call",
        Expr::Path(_) => "path",
        Expr::Reference(_) => "reference",
        Expr::Struct(_) => "struct",
        Expr::Lit(_) => "literal",
        _ => "expression",
    }
}
