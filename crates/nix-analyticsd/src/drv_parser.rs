//! Parser for Nix ATerm .drv files.
//!
//! Extracts `inputDrvs` (direct dependency derivation paths) from a .drv file.
//! ATerm format: `Derive(outputs, inputDrvs, inputSrcs, platform, builder, args, env)`
//! We only need the second list.

/// Parse the `inputDrvs` section of a .drv file, returning the derivation paths.
///
/// Each element in inputDrvs is `("/nix/store/hash-name.drv",["out",...])`.
/// We extract just the path string from each tuple.
pub fn parse_input_drvs(content: &str) -> anyhow::Result<Vec<String>> {
    let bytes = content.as_bytes();
    let len = bytes.len();

    // Skip "Derive(" prefix
    let prefix = b"Derive(";
    if !bytes.starts_with(prefix) {
        anyhow::bail!("not a valid .drv file: missing Derive( prefix");
    }
    let mut pos = prefix.len();

    // Skip first top-level [...] (outputs) by tracking nesting depth + in-string state
    pos = skip_bracket_group(bytes, pos, len)?;

    // Skip comma
    pos = skip_whitespace_and_comma(bytes, pos, len)?;

    // Now we're at the second [...] (inputDrvs)
    if pos >= len || bytes[pos] != b'[' {
        anyhow::bail!("expected '[' at start of inputDrvs at pos {pos}");
    }
    pos += 1; // skip '['

    let mut result = Vec::new();

    // Parse each ("path",["out",...]) tuple within the inputDrvs list
    loop {
        pos = skip_whitespace(bytes, pos, len);
        if pos >= len {
            anyhow::bail!("unexpected end of input in inputDrvs");
        }
        if bytes[pos] == b']' {
            break; // end of inputDrvs list
        }
        if bytes[pos] == b',' {
            pos += 1;
            continue;
        }

        // Expect '(' starting a tuple
        if bytes[pos] != b'(' {
            anyhow::bail!("expected '(' at pos {pos}");
        }
        pos += 1;

        // Extract the first quoted string (the drv path)
        let (path, new_pos) = parse_quoted_string(bytes, pos, len)?;
        pos = new_pos;
        result.push(path);

        // Skip the rest of the tuple until matching ')'
        pos = skip_until_close_paren(bytes, pos, len)?;
    }

    Ok(result)
}

fn skip_whitespace(bytes: &[u8], mut pos: usize, len: usize) -> usize {
    while pos < len && (bytes[pos] == b' ' || bytes[pos] == b'\n' || bytes[pos] == b'\r' || bytes[pos] == b'\t') {
        pos += 1;
    }
    pos
}

fn skip_whitespace_and_comma(bytes: &[u8], mut pos: usize, len: usize) -> anyhow::Result<usize> {
    pos = skip_whitespace(bytes, pos, len);
    if pos < len && bytes[pos] == b',' {
        pos += 1;
    }
    Ok(skip_whitespace(bytes, pos, len))
}

/// Skip a top-level `[...]` group, handling nested brackets and quoted strings.
fn skip_bracket_group(bytes: &[u8], mut pos: usize, len: usize) -> anyhow::Result<usize> {
    pos = skip_whitespace(bytes, pos, len);
    if pos >= len || bytes[pos] != b'[' {
        anyhow::bail!("expected '[' at pos {pos}");
    }
    let mut depth: u32 = 1;
    pos += 1;
    while pos < len && depth > 0 {
        match bytes[pos] {
            b'"' => {
                pos += 1;
                while pos < len && bytes[pos] != b'"' {
                    if bytes[pos] == b'\\' {
                        pos += 1; // skip escaped char
                    }
                    pos += 1;
                }
                // skip closing quote
                if pos < len {
                    pos += 1;
                }
            }
            b'[' => {
                depth += 1;
                pos += 1;
            }
            b']' => {
                depth -= 1;
                pos += 1;
            }
            _ => {
                pos += 1;
            }
        }
    }
    if depth != 0 {
        anyhow::bail!("unbalanced brackets in .drv file");
    }
    Ok(pos)
}

/// Parse a quoted string starting at `pos`, returning the unescaped content and new position.
fn parse_quoted_string(bytes: &[u8], mut pos: usize, len: usize) -> anyhow::Result<(String, usize)> {
    pos = skip_whitespace(bytes, pos, len);
    if pos >= len || bytes[pos] != b'"' {
        anyhow::bail!("expected '\"' at pos {pos}");
    }
    pos += 1; // skip opening quote

    let mut result = Vec::new();
    while pos < len && bytes[pos] != b'"' {
        if bytes[pos] == b'\\' {
            pos += 1;
            if pos < len {
                match bytes[pos] {
                    b'n' => result.push(b'\n'),
                    b't' => result.push(b'\t'),
                    b'r' => result.push(b'\r'),
                    b'\\' => result.push(b'\\'),
                    b'"' => result.push(b'"'),
                    other => {
                        result.push(b'\\');
                        result.push(other);
                    }
                }
            }
        } else {
            result.push(bytes[pos]);
        }
        pos += 1;
    }
    if pos < len {
        pos += 1; // skip closing quote
    }

    Ok((String::from_utf8(result)?, pos))
}

/// Skip forward until we find the matching ')' for the current tuple, handling nesting and strings.
fn skip_until_close_paren(bytes: &[u8], mut pos: usize, len: usize) -> anyhow::Result<usize> {
    let mut depth: u32 = 1;
    while pos < len && depth > 0 {
        match bytes[pos] {
            b'"' => {
                pos += 1;
                while pos < len && bytes[pos] != b'"' {
                    if bytes[pos] == b'\\' {
                        pos += 1;
                    }
                    pos += 1;
                }
                if pos < len {
                    pos += 1; // skip closing quote
                }
            }
            b'(' => {
                depth += 1;
                pos += 1;
            }
            b')' => {
                depth -= 1;
                pos += 1;
            }
            _ => {
                pos += 1;
            }
        }
    }
    if depth != 0 {
        anyhow::bail!("unbalanced parentheses in .drv tuple");
    }
    Ok(pos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_drvs() {
        let drv = r#"Derive([("out","/nix/store/abc-foo","","")],[],["/nix/store/src"],"x86_64-linux","/bin/sh",["-e","builder.sh"],[])"#;
        let result = parse_input_drvs(drv).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn single_input_drv() {
        let drv = r#"Derive([("out","/nix/store/abc-foo","","")],[("/nix/store/xyz-bar.drv",["out"])],["/nix/store/src"],"x86_64-linux","/bin/sh",["-e","builder.sh"],[])"#;
        let result = parse_input_drvs(drv).unwrap();
        assert_eq!(result, vec!["/nix/store/xyz-bar.drv"]);
    }

    #[test]
    fn two_input_drvs() {
        let drv = r#"Derive([("out","/nix/store/abc-foo","","")],[("/nix/store/aaa-bar.drv",["out"]),("/nix/store/bbb-baz.drv",["out","dev"])],["/nix/store/src"],"x86_64-linux","/bin/sh",["-e","builder.sh"],[])"#;
        let result = parse_input_drvs(drv).unwrap();
        assert_eq!(result, vec![
            "/nix/store/aaa-bar.drv",
            "/nix/store/bbb-baz.drv",
        ]);
    }

    #[test]
    fn many_input_drvs() {
        let drv = r#"Derive([("out","/nix/store/abc-foo","","")],[("/nix/store/a.drv",["out"]),("/nix/store/b.drv",["out"]),("/nix/store/c.drv",["out"]),("/nix/store/d.drv",["out"]),("/nix/store/e.drv",["out"])],[],"x86_64-linux","/bin/sh",[],[])"#;
        let result = parse_input_drvs(drv).unwrap();
        assert_eq!(result.len(), 5);
        assert_eq!(result[0], "/nix/store/a.drv");
        assert_eq!(result[4], "/nix/store/e.drv");
    }

    #[test]
    fn escaped_quotes_in_env_vars() {
        // The env section (7th arg) can have escaped quotes. Our parser should handle
        // this because we skip strings properly in the outputs section.
        let drv = r#"Derive([("out","/nix/store/abc-foo","sha256","abcdef")],[("/nix/store/xyz-bar.drv",["out"])],[],"x86_64-linux","/bin/sh",["-e","builder.sh"],[("name","foo"),("configureFlags","--prefix=\"/opt\"")])"#;
        let result = parse_input_drvs(drv).unwrap();
        assert_eq!(result, vec!["/nix/store/xyz-bar.drv"]);
    }

    #[test]
    fn malformed_missing_prefix() {
        let result = parse_input_drvs("not a drv file");
        assert!(result.is_err());
    }

    #[test]
    fn malformed_unbalanced_brackets() {
        let result = parse_input_drvs("Derive([[");
        assert!(result.is_err());
    }

    #[test]
    fn real_world_simple_drv() {
        // Simplified but realistic .drv content
        let drv = concat!(
            r#"Derive([("out","/nix/store/9krlzvny65gdc8s7kpb6lkx8cd02c25b-hello-2.12.1","","")],"#,
            r#"[("/nix/store/1im07brhlxa9wghhqr56p6v02q43k3nm-bash-5.2p26.drv",["out"]),"#,
            r#"("/nix/store/4nwzjl0n4z1amd8fqj1yqihy82vhf8yf-stdenv-linux.drv",["out"]),"#,
            r#"("/nix/store/dkl33q2m56icy3k0ifxls3gca28c9p3b-hello-2.12.1.tar.gz.drv",["out"])],"#,
            r#"["/nix/store/v6x3cs394jgqfbi0a42pam708flxaphh-default-builder.sh"],"#,
            r#""x86_64-linux","#,
            r#""/nix/store/5lr5n3qa4day8l1ivbwlcby3razfl5la-bash-5.2p26/bin/bash","#,
            r#"["-e","/nix/store/v6x3cs394jgqfbi0a42pam708flxaphh-default-builder.sh"],"#,
            r#"[("buildInputs",""),("name","hello-2.12.1"),("out","/nix/store/9krlzvny65gdc8s7kpb6lkx8cd02c25b-hello-2.12.1"),("src","/nix/store/pa10z4ngm0g83kx9mssrqzz30s84vq7k-hello-2.12.1.tar.gz"),("system","x86_64-linux")])"#
        );
        let result = parse_input_drvs(drv).unwrap();
        assert_eq!(result.len(), 3);
        assert_eq!(result[0], "/nix/store/1im07brhlxa9wghhqr56p6v02q43k3nm-bash-5.2p26.drv");
        assert_eq!(result[1], "/nix/store/4nwzjl0n4z1amd8fqj1yqihy82vhf8yf-stdenv-linux.drv");
        assert_eq!(result[2], "/nix/store/dkl33q2m56icy3k0ifxls3gca28c9p3b-hello-2.12.1.tar.gz.drv");
    }

    #[test]
    fn multiple_outputs_in_input_drv() {
        let drv = r#"Derive([("out","/nix/store/abc-foo","","")],[("/nix/store/xyz-bar.drv",["out","dev","lib"])],[],"x86_64-linux","/bin/sh",[],[])"#;
        let result = parse_input_drvs(drv).unwrap();
        assert_eq!(result, vec!["/nix/store/xyz-bar.drv"]);
    }

}
