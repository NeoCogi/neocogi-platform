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

/// Tests whether an identifier is followed by a call, allowing intervening whitespace/comments.
fn is_call(source: &str, mut end: usize) -> bool {
    let bytes = source.as_bytes();
    loop {
        while end < bytes.len() && bytes[end].is_ascii_whitespace() {
            end += 1;
        }
        if bytes[end..].starts_with(b"//") {
            while end < bytes.len() && bytes[end] != b'\n' {
                end += 1;
            }
        } else if bytes[end..].starts_with(b"/*") {
            end += 2;
            while end < bytes.len() && !bytes[end..].starts_with(b"*/") {
                end += 1;
            }
            end = (end + 2).min(bytes.len());
        } else {
            return bytes.get(end) == Some(&b'(');
        }
    }
}

/// Normalizes window Y position and signed differential axes independently.
/// Canonical fragment Y increases downwards. Raw window GL derivatives have the opposite
/// axis, even when a shader uses only interpolated values and never reads gl_FragCoord.
/// Helpers use explicit precision and are injected after rewriting client calls, so their
/// native derivative calls cannot recursively rewrite themselves. Offscreen stays unchanged.
pub(super) fn fragment(source: &str, offscreen: bool) -> Result<(String, Option<String>), String> {
    if offscreen {
        return Ok((source.into(), None));
    }
    let mut result = source.to_owned();
    let mut declarations = String::new();
    for builtin in ["dFdy", "dFdyFine", "dFdyCoarse"] {
        let spans: Vec<_> = identifiers(&result)
            .into_iter()
            .filter(|&(a, b)| &result[a..b] == builtin && is_call(&result, b))
            .collect();
        if spans.is_empty() {
            continue;
        }
        let helper = fresh(&result, &format!("nc_window_{builtin}"))?;
        result = replace(&result, &spans, builtin, &helper);
        // Only wrap spellings emitted by the provider. ES300 normally lowers fine/coarse to dFdy.
        for ty in ["float", "vec2", "vec3", "vec4"] {
            declarations.push_str(&format!("highp {ty} {helper}(highp {ty} value) {{ return -{builtin}(value); }}\n"));
        }
    }
    let spans = identifiers(&result);
    let height = if spans.iter().any(|&(a, b)| &result[a..b] == "gl_FragCoord") {
        let name = fresh(&result, "nc_target_height")?;
        let expression = format!("vec4(gl_FragCoord.x, {name} - gl_FragCoord.y, gl_FragCoord.zw)");
        result = replace(&result, &spans, "gl_FragCoord", &expression);
        declarations.push_str(&format!("uniform highp float {name};\n"));
        Some(name)
    } else {
        None
    };
    let position = declarations_start(&result);
    result.insert_str(position, &declarations);
    Ok((result, height))
}
#[cfg(test)]
mod tests {
    use super::*;
    /// Signed derivatives require normalization without fragment position; comments and names stay intact.
    #[test]
    fn varying_only_derivatives_use_fresh_nonrecursive_overloads() {
        let source = "#version 300 es\n// dFdy(x)\nprecision highp float;\nfloat nc_window_dFdy_0(float x){return x;}\nin vec2 p;out vec4 color;void main(){float my_dFdy=1.;color=vec4(dFdy /* gap */ (p),my_dFdy,1.);}";
        let (adapted, height) = fragment(source, false).unwrap();
        assert_eq!(height, None);
        assert!(adapted.contains("nc_window_dFdy_1 /* gap */ (p)"));
        assert_eq!(adapted.matches("return -dFdy(value)").count(), 4);
        assert!(adapted.contains("// dFdy(x)"));
        assert!(adapted.contains("float my_dFdy=1."));
        assert_eq!(fragment(source, true).unwrap(), (source.into(), None));
        for builtin in ["dFdyFine", "dFdyCoarse"] {
            let source = format!("#version 310 es\nvoid main(){{{builtin}(vec2(1.));}}");
            let (adapted, _) = fragment(&source, false).unwrap();
            assert_eq!(adapted.matches(&format!("return -{builtin}(value)")).count(), 4);
        }
    }

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
