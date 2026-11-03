//! Read the exact macro dependency recorded for this integration-test executable.
//! No output-directory scan, timestamp choice or nested Cargo invocation is involved.
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

/// Resolves the compiler-selected host library from rustc's binary-dependency rules.
/// Cargo's `binary-dep-depinfo` option writes a phony `path:` rule for each actual
/// dependency. A missing/ambiguous record fails explicitly instead of selecting a
/// similarly named library from another build. The recorded file must still exist.
pub(super) fn selected_macro(executable: &Path) -> Result<PathBuf, String> {
    let dep_info = executable.with_extension("d");
    let text =
        fs::read_to_string(&dep_info).map_err(|error| format!("cannot read {}: {error}; rebuild with Cargo binary-dep-depinfo enabled", dep_info.display()))?;
    let prefix = format!("{}neocogi_native_layout-", std::env::consts::DLL_PREFIX);
    let mut libraries = BTreeSet::new();
    for line in text.lines() {
        let Some(rule) = line.strip_suffix(':') else { continue };
        let path = make_path(rule)?;
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(std::env::consts::DLL_SUFFIX))
        {
            libraries.insert(path);
        }
    }
    if libraries.len() != 1 {
        return Err(format!(
            "expected exactly one compiler-selected native-layout macro in {}; found {}; rebuild with Cargo binary-dep-depinfo enabled",
            dep_info.display(),
            libraries.len()
        ));
    }
    let path = libraries.pop_first().expect("exactly one recorded dependency");
    if !path.is_absolute() || !path.is_file() {
        return Err(format!(
            "compiler-selected macro is not an existing absolute file: {}; rebuild this integration test",
            path.display()
        ));
    }
    Ok(path)
}

/// Decodes one rustc Makefile target, preserving Windows separators and escaped spaces.
/// Phony rules contain a single path; unescaped whitespace is rejected rather than
/// treating several prerequisites as one filename. Dollar signs use Make's `$$` spelling.
fn make_path(rule: &str) -> Result<PathBuf, String> {
    let mut chars = rule.chars().peekable();
    let mut path = String::new();
    while let Some(character) = chars.next() {
        match character {
            '\\' if chars.peek().is_some_and(|next| matches!(next, ' ' | '\t' | '#' | '\\' | ':')) => path.push(chars.next().expect("peeked escape")),
            '$' if chars.peek() == Some(&'$') => {
                chars.next();
                path.push('$');
            }
            c if c.is_whitespace() => return Err("invalid unescaped whitespace in compiler dependency path".into()),
            c => path.push(c),
        }
    }
    Ok(path.into())
}

/// Escaped paths and literal Windows separators remain exact, not shell-decoded guesses.
#[test]
fn dependency_paths_preserve_spaces_hashes_dollars_and_separators() {
    assert_eq!(
        make_path(r"/a\ folder/\#money$$/macro.so").unwrap(),
        PathBuf::from("/a folder/#money$/macro.so")
    );
    assert_eq!(make_path(r"C:\build\macro.dll").unwrap(), PathBuf::from(r"C:\build\macro.dll"));
    assert!(make_path("two words").is_err());
}

/// Missing, ambiguous and removed artifacts produce errors even if another matching file exists nearby.
#[test]
fn dependency_record_requires_one_live_selected_artifact() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("consumer");
    assert!(selected_macro(&executable).unwrap_err().contains("cannot read"));
    let library = |name| {
        directory.path().join(format!(
            "{}neocogi_native_layout-{name}{}",
            std::env::consts::DLL_PREFIX,
            std::env::consts::DLL_SUFFIX
        ))
    };
    let a = library("a");
    let b = library("b");
    fs::write(&a, []).unwrap();
    fs::write(&b, []).unwrap();
    let dep_info = executable.with_extension("d");
    fs::write(&dep_info, "source.rs:\n").unwrap();
    assert!(selected_macro(&executable).unwrap_err().contains("found 0"));
    fs::write(&dep_info, format!("{}:\n{}:\n", a.display(), b.display())).unwrap();
    assert!(selected_macro(&executable).unwrap_err().contains("found 2"));
    fs::write(&dep_info, format!("{}:\n{}:\n", a.display(), a.display())).unwrap();
    assert_eq!(selected_macro(&executable).unwrap(), a);
    fs::remove_file(&a).unwrap();
    assert!(selected_macro(&executable).unwrap_err().contains("not an existing absolute file"));
}
