//! Procedural expansion for typed Rust foundation-library adapters.

use proc_macro::{Delimiter, Group, TokenStream, TokenTree};
use std::collections::BTreeMap;

#[proc_macro_attribute]
pub fn poplib(attribute: TokenStream, item: TokenStream) -> TokenStream {
    expand(attribute, &item).unwrap_or_else(|message| compile_error(&message))
}

fn expand(attribute: TokenStream, item: &TokenStream) -> Result<TokenStream, String> {
    let fields = parse_fields(attribute)?;
    let bubble = required_single(&fields, "bubble")?;
    if bubble != "Standard" && bubble != "Internal" {
        return Err("`bubble` must be `Standard` or `Internal`".to_owned());
    }
    let namespace = required_string(&fields, "namespace")?;
    let binding_name = required_string(&fields, "name")?;
    let parameters = required_list(&fields, "parameters")?;
    let results = required_list(&fields, "results")?;
    let effects = required_list(&fields, "effects")?;
    if results.len() > 1 {
        return Err("the initial `poplib` ABI supports at most one result".to_owned());
    }

    let parameter_types = parameters
        .iter()
        .map(|name| rust_type(name))
        .collect::<Result<Vec<_>, _>>()?;
    let result_type = results
        .first()
        .map(|name| rust_type(name))
        .transpose()?
        .unwrap_or("()");
    for effect in &effects {
        validate_effect(effect)?;
    }

    let function_name = validate_function(item)?;
    let descriptor_name = format!("{}_POPLIB_EXPORT", function_name.to_ascii_uppercase());
    let parameter_descriptors = descriptor_values("PopAbiType", &parameters);
    let result_descriptors = descriptor_values("PopAbiType", &results);
    let effect_descriptors = descriptor_values("NativeEffect", &effects);
    let assertion_parameters = parameter_types.join(", ");
    let item_text = item.to_string();
    let expansion = format!(
        "#[unsafe(no_mangle)]\n{item_text}\n\
         #[doc(hidden)]\n\
         pub const {descriptor_name}: ::pop_library_bridge::NativeExport = \
         ::pop_library_bridge::NativeExport::new(\
             ::pop_library_bridge::FoundationBubble::{bubble}, \
             {namespace}, {binding_name}, \"{function_name}\", \
             &[{parameter_descriptors}], &[{result_descriptors}], &[{effect_descriptors}]\
         );\n\
         const _: extern \"C\" fn({assertion_parameters}) -> {result_type} = {function_name};"
    );
    expansion
        .parse()
        .map_err(|_| "failed to generate the typed `poplib` descriptor".to_owned())
}

#[derive(Clone, Debug)]
enum FieldValue {
    Single(String),
    List(Vec<String>),
}

fn parse_fields(attribute: TokenStream) -> Result<BTreeMap<String, FieldValue>, String> {
    let mut tokens = attribute.into_iter().peekable();
    let mut fields = BTreeMap::new();
    while let Some(token) = tokens.next() {
        let TokenTree::Ident(key) = token else {
            return Err("expected a `poplib` field name".to_owned());
        };
        let key = key.to_string();
        let value = match tokens.next() {
            Some(TokenTree::Punct(punctuation)) if punctuation.as_char() == '=' => {
                let value = tokens
                    .next()
                    .ok_or_else(|| format!("missing value for `{key}`"))?;
                FieldValue::Single(value.to_string())
            }
            Some(TokenTree::Group(group)) if group.delimiter() == Delimiter::Parenthesis => {
                FieldValue::List(parse_list(&group)?)
            }
            _ => return Err(format!("expected `=` or `(...)` after `{key}`")),
        };
        if fields.insert(key.clone(), value).is_some() {
            return Err(format!("duplicate `poplib` field `{key}`"));
        }
        match tokens.next() {
            Some(TokenTree::Punct(punctuation)) if punctuation.as_char() == ',' => {}
            Some(_) => return Err("expected `,` between `poplib` fields".to_owned()),
            None => break,
        }
    }

    for field in fields.keys() {
        if !matches!(
            field.as_str(),
            "bubble" | "namespace" | "name" | "parameters" | "results" | "effects"
        ) {
            return Err(format!("unknown `poplib` field `{field}`"));
        }
    }
    Ok(fields)
}

fn parse_list(group: &Group) -> Result<Vec<String>, String> {
    let mut values = Vec::new();
    let mut expect_value = true;
    for token in group.stream() {
        match token {
            TokenTree::Ident(value) if expect_value => {
                values.push(value.to_string());
                expect_value = false;
            }
            TokenTree::Punct(punctuation) if !expect_value && punctuation.as_char() == ',' => {
                expect_value = true;
            }
            _ => return Err("descriptor lists contain only comma-separated names".to_owned()),
        }
    }
    if expect_value && !values.is_empty() {
        return Ok(values);
    }
    if !expect_value || values.is_empty() {
        Ok(values)
    } else {
        Err("malformed descriptor list".to_owned())
    }
}

fn required_single(fields: &BTreeMap<String, FieldValue>, name: &str) -> Result<String, String> {
    match fields.get(name) {
        Some(FieldValue::Single(value)) => Ok(value.clone()),
        Some(FieldValue::List(_)) => Err(format!("`{name}` requires `= value`")),
        None => Err(format!("missing required `poplib` field `{name}`")),
    }
}

fn required_string(fields: &BTreeMap<String, FieldValue>, name: &str) -> Result<String, String> {
    let value = required_single(fields, name)?;
    if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
        Ok(value)
    } else {
        Err(format!("`{name}` requires a string literal"))
    }
}

fn required_list(fields: &BTreeMap<String, FieldValue>, name: &str) -> Result<Vec<String>, String> {
    match fields.get(name) {
        Some(FieldValue::List(values)) => Ok(values.clone()),
        Some(FieldValue::Single(_)) => Err(format!("`{name}` requires `(...)`")),
        None => Err(format!("missing required `poplib` field `{name}`")),
    }
}

fn validate_function(item: &TokenStream) -> Result<String, String> {
    let tokens = item.clone().into_iter().collect::<Vec<_>>();
    let function_index = tokens
        .iter()
        .position(|token| matches!(token, TokenTree::Ident(value) if value.to_string() == "fn"))
        .ok_or_else(|| "`#[poplib]` can only annotate a function".to_owned())?;
    let prefix = &tokens[..function_index];
    if !prefix
        .iter()
        .any(|token| matches!(token, TokenTree::Ident(value) if value.to_string() == "pub"))
    {
        return Err("a `poplib` adapter must be `pub`".to_owned());
    }
    if prefix
        .windows(2)
        .any(|pair| matches!(&pair[0], TokenTree::Ident(value) if value.to_string() == "pub")
            && matches!(&pair[1], TokenTree::Group(group) if group.delimiter() == Delimiter::Parenthesis))
    {
        return Err("a `poplib` adapter must have unrestricted `pub` visibility".to_owned());
    }
    let has_c_abi = prefix.windows(2).any(|pair| {
        matches!(&pair[0], TokenTree::Ident(value) if value.to_string() == "extern")
            && matches!(&pair[1], TokenTree::Literal(value) if value.to_string() == "\"C\"")
    });
    if !has_c_abi {
        return Err("a `poplib` adapter must use `extern \"C\"`".to_owned());
    }
    if prefix
        .iter()
        .any(|token| matches!(token, TokenTree::Ident(value) if value.to_string() == "async"))
    {
        return Err("a `poplib` adapter cannot be async".to_owned());
    }
    if item.to_string().contains("no_mangle") {
        return Err("`#[poplib]` supplies `no_mangle`; remove the duplicate attribute".to_owned());
    }
    let Some(TokenTree::Ident(name)) = tokens.get(function_index + 1) else {
        return Err("missing `poplib` function name".to_owned());
    };
    if !matches!(tokens.get(function_index + 2), Some(TokenTree::Group(group)) if group.delimiter() == Delimiter::Parenthesis)
    {
        return Err("a `poplib` adapter cannot be generic".to_owned());
    }
    Ok(name.to_string())
}

fn rust_type(name: &str) -> Result<&'static str, String> {
    match name {
        "Int" | "Int64" => Ok("i64"),
        "UInt64" | "String" | "ManagedReference" => Ok("u64"),
        "Float" => Ok("f64"),
        "Boolean" => Ok("bool"),
        "Byte" => Ok("u8"),
        _ => Err(format!("unsupported `poplib` ABI type `{name}`")),
    }
}

fn validate_effect(name: &str) -> Result<(), String> {
    if matches!(
        name,
        "Allocates"
            | "WritesManagedReference"
            | "MayTrap"
            | "MayUnwind"
            | "Suspends"
            | "Blocks"
            | "UnsafeMemory"
            | "ForeignFunction"
            | "AmbientIo"
            | "CompilerQuery"
            | "GcSafePoint"
            | "Roots"
    ) {
        Ok(())
    } else {
        Err(format!("unsupported `poplib` effect `{name}`"))
    }
}

fn descriptor_values(kind: &str, values: &[String]) -> String {
    values
        .iter()
        .map(|value| format!("::pop_library_bridge::{kind}::{value}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn compile_error(message: &str) -> TokenStream {
    let escaped = message.replace('\\', "\\\\").replace('"', "\\\"");
    format!("compile_error!(\"{escaped}\");")
        .parse()
        .expect("compile_error expansion is valid Rust")
}
