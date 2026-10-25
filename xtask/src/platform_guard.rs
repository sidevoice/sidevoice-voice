//! A token check, not a Rust type checker: every cfg branch is inspected, on every host.
use std::collections::BTreeSet;
use std::path::Path;

use proc_macro2::{Delimiter, TokenStream, TokenTree};

use crate::{read, repo, run_in, Result};

pub fn check() -> Result<()> {
    let root = repo();
    // Include untracked sources locally too; ignored build output is never scanned.
    let files = run_in(
        &root,
        "git ls-files --cached --others --exclude-standard -z",
        &[],
    )?;
    // Resolve cfg_aliases definitions in build scripts, including aliases of aliases.
    let mut aliases = BTreeSet::new();
    let builds: Vec<TokenStream> = files
        .split('\0')
        .filter(|path| {
            Path::new(path)
                .file_name()
                .is_some_and(|name| name == "build.rs")
        })
        .map(|path| {
            let source = String::from_utf8(read(&root.join(path))?).map_err(|e| e.to_string())?;
            source.parse().map_err(|e| format!("{path}: {e}"))
        })
        .collect::<Result<_>>()?;
    loop {
        let before = aliases.len();
        for build in &builds {
            collect_aliases(build.clone(), &mut aliases);
        }
        if aliases.len() == before {
            break;
        }
    }
    let mut errors = Vec::new();
    for path in files.split('\0').filter(|path| !path.is_empty()) {
        if exempt(Path::new(path)) || !path.ends_with(".rs") {
            continue;
        }
        let source =
            String::from_utf8(read(&root.join(path))?).map_err(|error| error.to_string())?;
        let tokens: TokenStream = source.parse().map_err(|error| format!("{path}: {error}"))?;
        let count = occurrences(tokens, &aliases);
        if count > 0 {
            errors.push(format!(
                "{path}: {count} OS-specific occurrences; move OS code into platform"
            ));
        }
    }
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    println!("platform boundary: no OS-specific code outside platform");
    Ok(())
}

fn exempt(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == "platform.rs")
        || path.components().any(|part| part.as_os_str() == "platform")
}

fn ident(token: Option<&TokenTree>, name: &str) -> bool {
    matches!(token, Some(TokenTree::Ident(value)) if value == name)
}

fn colon(token: Option<&TokenTree>) -> bool {
    matches!(token, Some(TokenTree::Punct(value)) if value.as_char() == ':')
}

/// One occurrence per OS-sensitive cfg/cfg_attr/cfg! invocation or OS API path/identifier.
/// Tokenization ignores comments and literal contents, and handles multiline attributes.
fn occurrences(stream: TokenStream, aliases: &BTreeSet<String>) -> usize {
    let tokens: Vec<_> = stream.into_iter().collect();
    let mut count = 0;
    let mut import_end = 0;
    for (i, token) in tokens.iter().enumerate() {
        if i < import_end {
            continue;
        }
        if ident(Some(token), "use") {
            if let Some(end) = tokens[i..]
                .iter()
                .position(|t| matches!(t, TokenTree::Punct(p) if p.as_char() == ';'))
            {
                import_end = i + end + 1;
                let item: syn::ItemUse =
                    syn::parse2(tokens[i..import_end].iter().cloned().collect())
                        .expect("valid Rust use tree");
                count += import_occurrences(&item.tree, false);
                continue;
            }
        }
        if let TokenTree::Group(group) = token {
            count += occurrences(group.stream(), aliases);
        }
        if ident(Some(token), "cfg") || ident(Some(token), "cfg_attr") {
            let next = if matches!(tokens.get(i + 1), Some(TokenTree::Punct(p)) if p.as_char() == '!')
            {
                i + 2
            } else {
                i + 1
            };
            if let Some(TokenTree::Group(group)) = tokens.get(next) {
                if group.delimiter() == Delimiter::Parenthesis
                    && os_condition(group.stream(), aliases)
                {
                    count += 1;
                }
            }
        }
        let path = colon(tokens.get(i + 1)) && colon(tokens.get(i + 2));
        if (ident(Some(token), "std")
            && path
            && ident(tokens.get(i + 3), "os")
            && colon(tokens.get(i + 4))
            && colon(tokens.get(i + 5)))
            || (path
                && ["libc", "rustix", "windows_sys", "windows"]
                    .iter()
                    .any(|name| ident(Some(token), name)))
            || ["objc2", "core_foundation", "coreaudio"]
                .iter()
                .any(|name| ident(Some(token), name))
        {
            count += 1;
        }
    }
    count
}

/// Walk grouped imports and original names in renamed imports; aliases are not API uses.
fn import_occurrences(tree: &syn::UseTree, in_std: bool) -> usize {
    match tree {
        syn::UseTree::Group(group) => group
            .items
            .iter()
            .map(|t| import_occurrences(t, in_std))
            .sum(),
        syn::UseTree::Glob(_) => 0,
        _ => {
            let name = match tree {
                syn::UseTree::Path(p) => &p.ident,
                syn::UseTree::Name(p) => &p.ident,
                syn::UseTree::Rename(p) => &p.ident,
                _ => unreachable!(),
            }
            .to_string();
            let hit = usize::from(
                (in_std && name == "os")
                    || [
                        "libc",
                        "rustix",
                        "windows_sys",
                        "windows",
                        "objc2",
                        "core_foundation",
                        "coreaudio",
                    ]
                    .contains(&name.as_str()),
            );
            hit + match tree {
                syn::UseTree::Path(p) => import_occurrences(&p.tree, in_std || name == "std"),
                _ => 0,
            }
        }
    }
}

/// cfg_aliases uses `name: { condition }`; only OS conditions seed the closure.
fn collect_aliases(stream: TokenStream, aliases: &mut BTreeSet<String>) {
    let tokens: Vec<_> = stream.into_iter().collect();
    for (i, token) in tokens.iter().enumerate() {
        if let TokenTree::Group(group) = token {
            collect_aliases(group.stream(), aliases);
        }
        if let (TokenTree::Ident(name), Some(TokenTree::Group(body))) = (token, tokens.get(i + 2)) {
            if colon(tokens.get(i + 1))
                && body.delimiter() == Delimiter::Brace
                && os_condition(body.stream(), aliases)
            {
                aliases.insert(name.to_string());
            }
        }
    }
}

fn os_condition(stream: TokenStream, aliases: &BTreeSet<String>) -> bool {
    let tokens: Vec<_> = stream.into_iter().collect();
    tokens.iter().enumerate().any(|(i, token)| {
        if let TokenTree::Group(group) = token {
            return os_condition(group.stream(), aliases);
        }
        matches!(token, TokenTree::Ident(name) if aliases.contains(&name.to_string()))
            || ["target_os", "unix", "windows"].iter().any(|name| ident(Some(token), name))
            || (ident(Some(token), "target_family")
                && !matches!(tokens.get(i + 2), Some(TokenTree::Literal(value)) if value.to_string() == "\"wasm\""))
    })
}

#[cfg(test)]
mod tests;
