use super::*;

pub fn tokenize_exec(exec: &str) -> FlameResult<Vec<String>> {
    let mut result = Vec::new();
    let mut token = String::new();
    let mut quote = None;
    let mut chars = exec.chars().peekable();
    while let Some(ch) = chars.next() {
        match quote {
            Some('"') => match ch {
                '"' => quote = None,
                '\\' => token.push(
                    chars
                        .next()
                        .ok_or_else(|| FlameError::invalid("unterminated Exec escape"))?,
                ),
                _ => token.push(ch),
            },
            Some('\'') => match ch {
                '\'' => quote = None,
                _ => token.push(ch),
            },
            None => match ch {
                '\'' | '"' => quote = Some(ch),
                '\\' => token.push(
                    chars
                        .next()
                        .ok_or_else(|| FlameError::invalid("unterminated Exec escape"))?,
                ),
                c if c.is_whitespace() => {
                    if !token.is_empty() {
                        result.push(std::mem::take(&mut token));
                    }
                }
                _ => token.push(ch),
            },
            _ => unreachable!(),
        }
    }
    if quote.is_some() {
        return Err(FlameError::invalid("unterminated Exec quote"));
    }
    if !token.is_empty() {
        result.push(token);
    }
    if result.is_empty() {
        return Err(FlameError::invalid("empty Exec"));
    }
    Ok(result)
}

pub fn expand_exec(
    exec: &str,
    app: &DesktopApplication,
    options: &ApplicationLaunchOptions,
) -> FlameResult<Vec<String>> {
    expand_exec_with_path(exec, app, Path::new(""), options)
}

pub(super) fn expand_exec_with_path(
    exec: &str,
    app: &DesktopApplication,
    desktop_path: &Path,
    options: &ApplicationLaunchOptions,
) -> FlameResult<Vec<String>> {
    let tokens = tokenize_exec(exec)?;
    let mut argv = Vec::new();
    for token in tokens {
        let mut output = String::new();
        let mut chars = token.chars();
        while let Some(ch) = chars.next() {
            if ch != '%' {
                output.push(ch);
                continue;
            }
            match chars.next() {
                Some('%') => output.push('%'),
                Some('c') => output.push_str(&app.name),
                Some('k') => output.push_str(&desktop_path.to_string_lossy()),
                Some('i') => {
                    if !app.icon_name.is_empty() {
                        argv.push("--icon".to_owned());
                        argv.push(app.icon_name.clone());
                    }
                }
                Some('f' | 'u') => {
                    if let Some(value) = options.uris.first() {
                        output.push_str(value);
                    }
                }
                Some(code @ ('F' | 'U')) => {
                    if token != format!("%{code}") {
                        return Err(FlameError::invalid(
                            "list Exec field code must be standalone",
                        ));
                    }
                    argv.extend(options.uris.iter().cloned());
                }
                // Deprecated by the XDG Desktop Entry spec: must be ignored (removed).
                Some('d' | 'D' | 'n' | 'N' | 'v' | 'm') => {}
                Some(code) => {
                    return Err(FlameError::invalid(format!(
                        "unsupported Exec field code %{code}"
                    )));
                }
                None => return Err(FlameError::invalid("trailing Exec field marker")),
            }
        }
        if !output.is_empty() {
            argv.push(output);
        }
    }
    argv.extend(options.extra_args.iter().cloned());
    Ok(argv)
}
