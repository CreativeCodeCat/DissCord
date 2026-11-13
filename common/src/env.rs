//! Loads settings from a `.env` file in the current directory.
//!
//! Each line is `NAME=value`. Blank lines and lines starting with `#` are ignored. Values may be
//! wrapped in single or double quotes. A setting that is already set in the shell is left alone,
//! so `NAME=value cargo run` still wins over the file.

/// Reads `.env` in the current directory, if it exists, and sets any variables not already set.
/// Call this once, at the start of `main`, before any threads are started.
pub fn load() {
    let Ok(text) = std::fs::read_to_string(".env") else {
        return;
    };
    for (name, value) in parse(&text) {
        if std::env::var_os(&name).is_none() {
            std::env::set_var(name, value);
        }
    }
}

/// Splits `.env` text into (name, value) pairs, in order.
pub fn parse(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let line = line.strip_prefix("export ").unwrap_or(line);
            let (name, value) = line.split_once('=')?;
            let name = name.trim();
            if name.is_empty() {
                return None;
            }
            Some((name.to_string(), value_of(value.trim())))
        })
        .collect()
}

/// The value part of a line: quotes are removed, and an unquoted `# comment` is dropped.
fn value_of(raw: &str) -> String {
    for quote in ['"', '\''] {
        if let Some(inner) = raw.strip_prefix(quote).and_then(|r| r.strip_suffix(quote)) {
            return inner.to_string();
        }
    }
    match raw.find(" #") {
        Some(at) => raw[..at].trim_end().to_string(),
        None => raw.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn reads_names_values_quotes_and_comments() {
        let text = "# a comment\n\nDISSCORD_PEERS=127.0.0.1:7878,203.0.113.5:7878 # two servers\n\
                    export DISSCORD_AUTH=\"127.0.0.1:7900\"\nNAME='x y'\nEMPTY=\nnot a setting\n";
        assert_eq!(
            parse(text),
            vec![
                ("DISSCORD_PEERS".to_string(), "127.0.0.1:7878,203.0.113.5:7878".to_string()),
                ("DISSCORD_AUTH".to_string(), "127.0.0.1:7900".to_string()),
                ("NAME".to_string(), "x y".to_string()),
                ("EMPTY".to_string(), String::new()),
            ]
        );
    }
}
