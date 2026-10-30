//! Small GLSL token adapter shared by native Glow and its WebGL executor. It
//! changes standard entry/builtin identifiers, never compiler-specific epilogues.

/// Identifier spans outside comments/quoted preprocessor text. Generated ES300
/// identifiers are ASCII; untouched slices retain all whitespace and directives.
fn identifiers(source: &str) -> Vec<(usize, usize)> {
    let bytes = source.as_bytes();
    let mut i = 0;
    let mut spans = Vec::new();
    while i < bytes.len() {
        if bytes[i..].starts_with(b"//") {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if bytes[i..].starts_with(b"/*") {
            i += 2;
            while i < bytes.len() && !bytes[i..].starts_with(b"*/") {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
        } else if bytes[i] == b'"' {
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i = (i + 2).min(bytes.len());
                } else if bytes[i] == b'"' {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
        } else if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            let start = i;
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            spans.push((start, i));
        } else {
            i += 1;
        }
    }
    spans
}
/// Fresh renderer-private identifier, excluding every spelling in the input.
fn fresh(source: &str, prefix: &str) -> Result<String, String> {
    for ordinal in 0..=u32::MAX {
        let name = format!("{prefix}_{ordinal}");
        if !source.contains(&name) {
            return Ok(name);
        }
    }
    Err("renderer shader identifier space exhausted".into())
}
/// Applies a complete-identifier replacement, preserving comments and substrings.
fn replace(source: &str, spans: &[(usize, usize)], target: &str, replacement: &str) -> String {
    let mut result = String::new();
    let mut first = 0;
    for &(start, end) in spans {
        if &source[start..end] == target {
            result.push_str(&source[first..start]);
            result.push_str(replacement);
            first = end;
        }
    }
    result.push_str(&source[first..]);
    result
}
/// Presentation restores Y after standard main returns. Offscreen preserves the
/// compiler's (x,-y,2z-w,w) convention, without inspecting its generated spelling.
pub(super) fn vertex(source: &str, offscreen: bool) -> Result<String, String> {
    if offscreen {
        return Ok(source.into());
    }
    let spans = identifiers(source);
    if !spans.iter().any(|&(a, b)| &source[a..b] == "main") {
        return Err("compiled GLSL must expose standard main".into());
    }
    let entry = fresh(source, "nc_vertex_entry")?;
    let mut result = replace(source, &spans, "main", &entry);
    result.push_str(&format!("\nvoid main() {{ {entry}(); gl_Position.y = -gl_Position.y; }}\n"));
    Ok(result)
}
/// Finds the first declaration after whitespace, comments and preprocessor lines.
/// Uniforms use explicit highp, so no default precision declaration is required.
fn declarations_start(source: &str) -> usize {
    let bytes = source.as_bytes();
    let mut i = 0;
    loop {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if bytes[i..].starts_with(b"//") {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if bytes[i..].starts_with(b"/*") {
            i += 2;
            while i < bytes.len() && !bytes[i..].starts_with(b"*/") {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
        } else if i < bytes.len() && bytes[i] == b'#' {
            // Inject outside conditional preambles: a declaration inside #if 0
            // must not hide the private uniform from the active entry point.
            let directive = source[i + 1..].trim_start();
            if directive.starts_with("if") {
                return i;
            }
            loop {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                let continued = i > 0 && bytes[i - 1] == b'\\';
                if i < bytes.len() {
                    i += 1;
                }
                if !continued || i == bytes.len() {
                    break;
                }
            }
        } else {
            return i;
        }
    }
}

/// Direct presentation normalizes raw GL fragment position. The private uniform
/// precedes precision declarations but follows version/extension directives.
pub(super) fn fragment(source: &str, offscreen: bool) -> Result<(String, Option<String>), String> {
    if offscreen {
        return Ok((source.into(), None));
    }
    let spans = identifiers(source);
    if !spans.iter().any(|&(a, b)| &source[a..b] == "gl_FragCoord") {
        return Ok((source.into(), None));
    }
    let name = fresh(source, "nc_target_height")?;
    let expression = format!("vec4(gl_FragCoord.x, {name} - gl_FragCoord.y, gl_FragCoord.zw)");
    let mut result = replace(source, &spans, "gl_FragCoord", &expression);
    let position = declarations_start(&result);
    result.insert_str(position, &format!("uniform highp float {name};\n"));
    Ok((result, Some(name)))
}
#[cfg(test)]
mod tests {
    use super::*;
    /// Explicit precision and a standard-main prototype are valid compiler outputs.
    #[test]
    fn accepts_explicit_precision_and_entry_prototype() {
        let source =
            "#version 300 es\n#if 0\nprecision highp float;\n#endif\nlayout(location=0) out highp vec4 color;\nvoid main();\nvoid main(){color=gl_FragCoord;}";
        let (fragment, height) = fragment(source, false).unwrap();
        assert!(height.is_some());
        assert!(fragment.starts_with("#version 300 es\nuniform highp float"));
        let source = "#version 300 es\nvoid main();\nvoid main(){gl_Position=vec4(0.,0.,0.,1.);}";
        assert!(vertex(source, false).unwrap().contains("nc_vertex_entry_0(); gl_Position.y"));
    }

    /// Whitespace, comments, private-name collisions and identifier substrings are independent of Naga spelling.
    #[test]
    fn adapts_standard_tokens_and_preserves_comments() {
        let source = "#version 300 es\n// main gl_FragCoord\nprecision\n highp float;\nvoid /* gap */ main ( void ) { float nc_vertex_entry_0=0.; gl_Position=vec4(0.,.75,.25,1.); }";
        let result = vertex(source, false).unwrap();
        assert!(result.contains("void /* gap */ nc_vertex_entry_1 ( void )"));
        assert!(result.contains("// main gl_FragCoord"));
        assert!(result.contains("nc_vertex_entry_1(); gl_Position.y = -gl_Position.y;"));
        assert_eq!(vertex(source, true).unwrap(), source);
        let source =
            "#version 300 es\n/* gl_FragCoord */\nprecision\n highp float;\nvoid main(){ float nc_target_height_0=1.; vec4 gl_FragCoord_backup=gl_FragCoord; }";
        let (result, name) = fragment(source, false).unwrap();
        assert_eq!(name.as_deref(), Some("nc_target_height_1"));
        assert!(result.contains("/* gl_FragCoord */"));
        assert!(result.contains("gl_FragCoord_backup=vec4(gl_FragCoord.x, nc_target_height_1 - gl_FragCoord.y, gl_FragCoord.zw)"));
    }
}
