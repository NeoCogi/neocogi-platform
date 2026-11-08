//! Generate native LLDB commands without requiring its Python scripting bridge.
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{fs, io, path::Path};

/// Cargo's versioned inverse path map; unknown metadata cannot become debugger commands.
#[derive(Deserialize)]
struct Sidecar {
    /// Only the pinned Cargo version-1 format is supported.
    v: u32,
    /// Ordered mappings, appended after any explicit debugger settings.
    remaps: Vec<Mapping>,
}

/// One path prefix and its local source directory, preserving mapping direction.
#[derive(Deserialize)]
struct Mapping {
    /// Prefix stored in the executable's debug information.
    from: String,
    /// Local source location supplied to the debugger.
    to: String,
}

/// Quote one LLDB argument, rejecting command-file line breaks and other control characters.
fn quote(path: &str) -> Result<String> {
    ensure!(!path.is_empty() && !path.chars().any(char::is_control), "invalid Cargo source mapping path");
    Ok(format!("\"{}\"", path.replace('\\', "\\\\").replace('"', "\\\"")))
}

/// Read the adjacent Cargo sidecar; an ordinary untrimmed executable needs no commands.
pub(crate) fn commands(executable: &Path) -> Result<String> {
    let mut filename = executable.file_name().context("executable needs a filename")?.to_os_string();
    filename.push(".trim-paths.json");
    let sidecar = executable.with_file_name(filename);
    let contents = match fs::read(&sidecar) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(String::new()),
        Err(error) => return Err(error).with_context(|| format!("read {}", sidecar.display())),
    };
    let metadata: Sidecar = serde_json::from_slice(&contents).context("invalid Cargo source-map format")?;
    ensure!(metadata.v == 1, "unsupported Cargo source-map version: {}", metadata.v);
    let mut arguments = Vec::new();
    for mapping in metadata.remaps {
        arguments.push(quote(&mapping.from)?);
        arguments.push(quote(&mapping.to)?);
    }
    Ok(if arguments.is_empty() {
        String::new()
    } else {
        format!("settings append target.source-map {}\n", arguments.join(" "))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mapping order, direction and unusual local paths survive native LLDB quoting.
    #[test]
    fn missing_sidecar_and_quoted_mappings() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("app with spaces");
        assert_eq!(commands(&executable).unwrap(), "");
        fs::write(
            executable.with_file_name("app with spaces.trim-paths.json"),
            serde_json::json!({
                "v":1, "remaps":[{"from":".","to":"/source with spaces/author's \"checkout\""},
                    {"from":"/cargo/build-dir","to":r"C:\custom target"}]
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            commands(&executable).unwrap(),
            "settings append target.source-map \".\" \"/source with spaces/author's \\\"checkout\\\"\" \"/cargo/build-dir\" \"C:\\\\custom target\"\n"
        );
    }

    /// Malformed metadata and newline injection fail before emitting any debugger commands.
    #[test]
    fn rejects_corrupt_unsupported_and_injected_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("app");
        for contents in [
            "{",
            "[]",
            r#"{"v":2,"remaps":[]}"#,
            r#"{"v":1}"#,
            r#"{"v":1,"remaps":[{"from":"."}]}"#,
            r#"{"v":1,"remaps":[{"from":".","to":false}]}"#,
            r#"{"v":1,"remaps":[{"from":".","to":""}]}"#,
            r#"{"v":1,"remaps":[{"from":".","to":"a\nquit"}]}"#,
        ] {
            fs::write(executable.with_file_name("app.trim-paths.json"), contents).unwrap();
            assert!(commands(&executable).is_err(), "{contents}");
        }
    }
}
