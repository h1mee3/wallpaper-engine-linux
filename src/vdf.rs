use std::collections::HashMap;

/// A node in a Valve KeyValues (VDF) tree.
#[derive(Debug, Clone, PartialEq)]
pub enum VdfValue {
    String(String),
    Object(HashMap<String, VdfValue>),
}

impl VdfValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            VdfValue::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&HashMap<String, VdfValue>> {
        match self {
            VdfValue::Object(map) => Some(map),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&VdfValue> {
        self.as_object().and_then(|map| {
            // Case-insensitive key lookup for VDF compatibility
            map.get(key).or_else(|| {
                map.iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(key))
                    .map(|(_, v)| v)
            })
        })
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(|v| v.as_str())
    }
}

/// Tokenizer for VDF syntax.
#[derive(Debug, PartialEq)]
enum Token {
    String(String),
    OpenBrace,
    CloseBrace,
}

fn tokenize(input: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    let len = chars.len();

    while i < len {
        let c = chars[i];

        // Skip whitespace
        if c.is_whitespace() {
            i += 1;
            continue;
        }

        // Skip line comments
        if c == '/' && i + 1 < len && chars[i + 1] == '/' {
            while i < len && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }

        if c == '{' {
            tokens.push(Token::OpenBrace);
            i += 1;
            continue;
        }

        if c == '}' {
            tokens.push(Token::CloseBrace);
            i += 1;
            continue;
        }

        // Quoted string
        if c == '"' {
            i += 1;
            let mut s = String::new();
            while i < len && chars[i] != '"' {
                if chars[i] == '\\' && i + 1 < len {
                    i += 1;
                    match chars[i] {
                        'n' => s.push('\n'),
                        't' => s.push('\t'),
                        '\\' => s.push('\\'),
                        '"' => s.push('"'),
                        other => s.push(other),
                    }
                } else {
                    s.push(chars[i]);
                }
                i += 1;
            }
            if i < len && chars[i] == '"' {
                i += 1;
            }
            tokens.push(Token::String(s));
            continue;
        }

        // Unquoted bare token
        let mut s = String::new();
        while i < len && !chars[i].is_whitespace() && chars[i] != '{' && chars[i] != '}' && chars[i] != '"' {
            s.push(chars[i]);
            i += 1;
        }
        if !s.is_empty() {
            tokens.push(Token::String(s));
        }
    }

    tokens
}

/// Parse VDF string into a top-level VdfValue::Object.
pub fn parse_vdf(input: &str) -> Result<VdfValue, String> {
    let tokens = tokenize(input);
    let mut pos = 0;
    let mut root = HashMap::new();

    while pos < tokens.len() {
        if let Token::String(key) = &tokens[pos] {
            pos += 1;
            let val = parse_val(&tokens, &mut pos)?;
            root.insert(key.clone(), val);
        } else {
            return Err(format!("Expected key string at token position {}", pos));
        }
    }

    Ok(VdfValue::Object(root))
}

fn parse_val(tokens: &[Token], pos: &mut usize) -> Result<VdfValue, String> {
    if *pos >= tokens.len() {
        return Err("Unexpected end of tokens".to_string());
    }

    match &tokens[*pos] {
        Token::String(s) => {
            let val = VdfValue::String(s.clone());
            *pos += 1;
            Ok(val)
        }
        Token::OpenBrace => {
            *pos += 1;
            let mut map = HashMap::new();
            while *pos < tokens.len() {
                if tokens[*pos] == Token::CloseBrace {
                    *pos += 1;
                    return Ok(VdfValue::Object(map));
                }
                if let Token::String(k) = &tokens[*pos] {
                    *pos += 1;
                    let v = parse_val(tokens, pos)?;
                    map.insert(k.clone(), v);
                } else {
                    return Err(format!("Expected key string or '}}' at position {}", *pos));
                }
            }
            Err("Unclosed brace".to_string())
        }
        Token::CloseBrace => Err(format!("Unexpected '}}' at position {}", *pos)),
    }
}

/// Extract all library folder paths from a parsed `libraryfolders.vdf`.
pub fn extract_library_paths(vdf: &VdfValue) -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();

    let root = match vdf.get("libraryfolders") {
        Some(v) => v,
        None => vdf,
    };

    if let Some(map) = root.as_object() {
        for (_idx, folder_val) in map {
            if let Some(folder_obj) = folder_val.as_object() {
                if let Some(VdfValue::String(p)) = folder_obj.get("path") {
                    let path = std::path::PathBuf::from(p);
                    if path.exists() && !paths.contains(&path) {
                        paths.push(path);
                    }
                }
            }
        }
    }

    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_vdf() {
        let input = r#"
        "libraryfolders"
        {
            "0"
            {
                "path" "/home/user/.steam"
                "apps"
                {
                    "431960" "12345"
                }
            }
            "1"
            {
                "path" "/mnt/games/SteamLibrary"
            }
        }
        "#;

        let parsed = parse_vdf(input).expect("Failed to parse VDF");
        let root = parsed.get("libraryfolders").expect("missing libraryfolders");
        let zero = root.get("0").expect("missing 0");
        assert_eq!(zero.get_str("path"), Some("/home/user/.steam"));
        let apps = zero.get("apps").expect("missing apps");
        assert_eq!(apps.get_str("431960"), Some("12345"));
    }
}
