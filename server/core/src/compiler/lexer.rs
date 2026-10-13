use super::ast::DataType;
use super::value::{days_from_civil, format_date, format_time, DAY_MS};

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    Program,
    EndProgram,
    FunctionBlock,
    EndFunctionBlock,
    Var,
    VarInput,
    VarOutput,
    EndVar,
    /// `TYPE … END_TYPE`, `STRUCT … END_STRUCT`, `ARRAY[…] OF`.
    TypeBlock,
    EndType,
    Struct,
    EndStruct,
    Array,
    If,
    Then,
    Elsif,
    Else,
    EndIf,
    For,
    To,
    By,
    Do,
    EndFor,
    While,
    EndWhile,
    Repeat,
    Until,
    EndRepeat,
    Case,
    Of,
    EndCase,
    Type(DataType),
    True,
    False,
    And,
    Or,
    Not,
    Xor,
    Mod,
    Identifier(String),
    IntLiteral(i128),
    RealLiteral(f64),
    /// `T#…` / `TIME#…`, in milliseconds.
    TimeLiteral(i64),
    /// `D#…`, `TOD#…`, `DT#…`: the type and its milliseconds.
    DateLiteral(DataType, i64),
    /// `'text'` (STRING) or `"text"` (WSTRING), escapes resolved.
    StringLiteral(DataType, String),
    /// `[` and `]`, only for `STRING[80]`.
    LBracket,
    RBracket,
    Assign,
    Equal,
    NotEqual,
    Less,
    Greater,
    LessEqual,
    GreaterEqual,
    Plus,
    Minus,
    Star,
    Slash,
    LParen,
    RParen,
    Comma,
    Dot,
    Semicolon,
    Colon,
    /// The `#` of a typed literal such as `BYTE#16#FF`.
    Hash,
    /// `^`, dereferencing a pointer.
    Caret,
    /// `REF=`, binding a reference.
    RefAssign,
    /// `POINTER TO` / `REFERENCE TO` (each followed by TO).
    Pointer,
    Reference,
    /// Text that isn't valid ST, with the message to show. The lexer never fails on its
    /// own: the parser reports this when it reaches it, so errors come out in source order.
    Invalid(String),
    /// A `T#…` literal that isn't a valid duration, with the message to show.
    InvalidTime(String),
    Eof,
}

impl TokenKind {
    /// How the token is shown in error messages.
    pub fn describe(&self) -> String {
        let text = match self {
            TokenKind::Identifier(name) => return format!("'{name}'"),
            TokenKind::IntLiteral(n) => return format!("'{n}'"),
            TokenKind::RealLiteral(x) => return format!("'{x}'"),
            TokenKind::TimeLiteral(ms) => return format_time(*ms),
            TokenKind::DateLiteral(t, ms) => return format_date(*t, *ms),
            TokenKind::StringLiteral(DataType::WString, s) => return format!("\"{s}\""),
            TokenKind::StringLiteral(_, s) => return format!("'{s}'"),
            TokenKind::LBracket => "'['",
            TokenKind::RBracket => "']'",
            TokenKind::Invalid(message) | TokenKind::InvalidTime(message) => return message.clone(),
            TokenKind::Type(data_type) => data_type.name(),
            TokenKind::Eof => "end of file",
            TokenKind::Program => "PROGRAM",
            TokenKind::EndProgram => "END_PROGRAM",
            TokenKind::FunctionBlock => "FUNCTION_BLOCK",
            TokenKind::EndFunctionBlock => "END_FUNCTION_BLOCK",
            TokenKind::Var => "VAR",
            TokenKind::VarInput => "VAR_INPUT",
            TokenKind::VarOutput => "VAR_OUTPUT",
            TokenKind::EndVar => "END_VAR",
            TokenKind::TypeBlock => "TYPE",
            TokenKind::EndType => "END_TYPE",
            TokenKind::Struct => "STRUCT",
            TokenKind::EndStruct => "END_STRUCT",
            TokenKind::Array => "ARRAY",
            TokenKind::If => "IF",
            TokenKind::Then => "THEN",
            TokenKind::Elsif => "ELSIF",
            TokenKind::Else => "ELSE",
            TokenKind::EndIf => "END_IF",
            TokenKind::For => "FOR",
            TokenKind::To => "TO",
            TokenKind::By => "BY",
            TokenKind::Do => "DO",
            TokenKind::EndFor => "END_FOR",
            TokenKind::While => "WHILE",
            TokenKind::EndWhile => "END_WHILE",
            TokenKind::Repeat => "REPEAT",
            TokenKind::Until => "UNTIL",
            TokenKind::EndRepeat => "END_REPEAT",
            TokenKind::Case => "CASE",
            TokenKind::Of => "OF",
            TokenKind::EndCase => "END_CASE",
            TokenKind::True => "TRUE",
            TokenKind::False => "FALSE",
            TokenKind::And => "AND",
            TokenKind::Or => "OR",
            TokenKind::Not => "NOT",
            TokenKind::Xor => "XOR",
            TokenKind::Mod => "MOD",
            TokenKind::Assign => "':='",
            TokenKind::Equal => "'='",
            TokenKind::NotEqual => "'<>'",
            TokenKind::Less => "'<'",
            TokenKind::Greater => "'>'",
            TokenKind::LessEqual => "'<='",
            TokenKind::GreaterEqual => "'>='",
            TokenKind::Plus => "'+'",
            TokenKind::Minus => "'-'",
            TokenKind::Star => "'*'",
            TokenKind::Slash => "'/'",
            TokenKind::LParen => "'('",
            TokenKind::RParen => "')'",
            TokenKind::Comma => "','",
            TokenKind::Dot => "'.'",
            TokenKind::Semicolon => "';'",
            TokenKind::Colon => "':'",
            TokenKind::Hash => "'#'",
            TokenKind::Caret => "'^'",
            TokenKind::RefAssign => "'REF='",
            TokenKind::Pointer => "POINTER",
            TokenKind::Reference => "REFERENCE",
        };
        text.to_string()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub line: usize,
    pub column: usize,
}

/// IEC 61131-3 keywords are case-insensitive.
fn keyword(word: &str) -> Option<TokenKind> {
    let kind = match word.to_ascii_uppercase().as_str() {
        "PROGRAM" => TokenKind::Program,
        "END_PROGRAM" => TokenKind::EndProgram,
        "FUNCTION_BLOCK" => TokenKind::FunctionBlock,
        "END_FUNCTION_BLOCK" => TokenKind::EndFunctionBlock,
        "VAR" => TokenKind::Var,
        "VAR_INPUT" => TokenKind::VarInput,
        "VAR_OUTPUT" => TokenKind::VarOutput,
        "END_VAR" => TokenKind::EndVar,
        "TYPE" => TokenKind::TypeBlock,
        "END_TYPE" => TokenKind::EndType,
        "STRUCT" => TokenKind::Struct,
        "END_STRUCT" => TokenKind::EndStruct,
        "ARRAY" => TokenKind::Array,
        "POINTER" => TokenKind::Pointer,
        "REFERENCE" => TokenKind::Reference,
        "IF" => TokenKind::If,
        "THEN" => TokenKind::Then,
        "ELSIF" => TokenKind::Elsif,
        "ELSE" => TokenKind::Else,
        "END_IF" => TokenKind::EndIf,
        "FOR" => TokenKind::For,
        "TO" => TokenKind::To,
        "BY" => TokenKind::By,
        "DO" => TokenKind::Do,
        "END_FOR" => TokenKind::EndFor,
        "WHILE" => TokenKind::While,
        "END_WHILE" => TokenKind::EndWhile,
        "REPEAT" => TokenKind::Repeat,
        "UNTIL" => TokenKind::Until,
        "END_REPEAT" => TokenKind::EndRepeat,
        "CASE" => TokenKind::Case,
        "OF" => TokenKind::Of,
        "END_CASE" => TokenKind::EndCase,
        "TRUE" => TokenKind::True,
        "FALSE" => TokenKind::False,
        "AND" => TokenKind::And,
        "OR" => TokenKind::Or,
        "NOT" => TokenKind::Not,
        "XOR" => TokenKind::Xor,
        "MOD" => TokenKind::Mod,
        upper => return DataType::from_name(upper).map(TokenKind::Type),
    };
    Some(kind)
}

/// Parse a duration body such as `2s`, `1m30s`, `1.5s`, `500ms`, `1h_30m` into ms.
fn parse_duration(body: &str) -> Result<i64, String> {
    let text = body.to_ascii_lowercase().replace('_', "");
    let invalid = || format!("Invalid TIME literal 'T#{body}' (use e.g. T#2s, T#500ms, T#1m30s)");
    if text.is_empty() {
        return Err(invalid());
    }
    let mut total_ms = 0.0_f64;
    let mut rest = text.as_str();
    while !rest.is_empty() {
        let number_len = rest.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(rest.len());
        let number: f64 = rest[..number_len].parse().map_err(|_| invalid())?;
        rest = &rest[number_len..];
        let unit_len = rest.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(rest.len());
        let unit_ms = match &rest[..unit_len] {
            "d" => 86_400_000.0,
            "h" => 3_600_000.0,
            "m" => 60_000.0,
            "s" => 1_000.0,
            "ms" => 1.0,
            _ => return Err(invalid()),
        };
        rest = &rest[unit_len..];
        total_ms += number * unit_ms;
    }
    // Bound well inside i64 so later TIME arithmetic can't start out overflowed.
    if total_ms > 1e15 {
        return Err(format!("TIME literal 'T#{body}' is too large"));
    }
    Ok(total_ms.round() as i64)
}

/// The date type a literal prefix denotes: `D#`/`DATE#`, `TOD#`/`TIME_OF_DAY#`, `DT#`/`DATE_AND_TIME#`.
fn date_prefix(word: &str) -> Option<DataType> {
    match word.to_ascii_uppercase().as_str() {
        "D" | "DATE" => Some(DataType::Date),
        "TOD" | "TIME_OF_DAY" => Some(DataType::Tod),
        "DT" | "DATE_AND_TIME" => Some(DataType::Dt),
        _ => None,
    }
}

/// `2024-01-31` → ms at its midnight.
fn parse_day(text: &str) -> Option<i64> {
    let mut parts = text.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, d) = (parts.next()??, parts.next()??, parts.next()??);
    if parts.next().is_some() || !(1..=9999).contains(&y) {
        return None;
    }
    Some(days_from_civil(y, m, d)? * DAY_MS as i64)
}

/// `12:30`, `12:30:05`, `12:30:05.25` → ms since midnight.
fn parse_time_of_day(text: &str) -> Option<i64> {
    let mut parts = text.split(':');
    let (h, m) = (parts.next()?.parse::<i64>().ok()?, parts.next()?.parse::<i64>().ok()?);
    let s: f64 = parts.next().map_or(Some(0.0), |s| s.parse().ok())?;
    if parts.next().is_some() || !(0..24).contains(&h) || !(0..60).contains(&m) || !(0.0..60.0).contains(&s) {
        return None;
    }
    Some(h * 3_600_000 + m * 60_000 + (s * 1000.0).round() as i64)
}

fn parse_date_literal(data_type: DataType, body: &str) -> Result<i64, String> {
    let text = body.replace('_', "");
    let (example, value) = match data_type {
        DataType::Date => ("D#2024-01-31", parse_day(&text)),
        DataType::Tod => ("TOD#12:30:00", parse_time_of_day(&text)),
        _ => {
            // The date and the time are joined by the third '-'.
            let split = text.match_indices('-').nth(2).map(|(i, _)| i);
            let value = split.and_then(|i| Some(parse_day(&text[..i])? + parse_time_of_day(&text[i + 1..])?));
            ("DT#2024-01-31-12:30:00", value)
        }
    };
    value.ok_or_else(|| format!("Invalid {} literal '{body}' (use e.g. {example})", data_type.name()))
}

/// The text of a string literal, from just after its opening quote: the IEC escapes
/// `$$ $' $" $L $N $P $R $T` and `$hh` (`$hhhh` in a WSTRING) resolved. Returns the text
/// and how many characters were read, closing quote included.
fn string_literal(chars: &[char], quote: char) -> Result<(String, usize), String> {
    let hex_digits = if quote == '"' { 4 } else { 2 };
    let mut text = String::new();
    let mut i = 0;
    loop {
        match chars.get(i) {
            None | Some('\n') => return Err(format!("Unterminated string: missing closing {quote}")),
            Some(&c) if c == quote => return Ok((text, i + 1)),
            Some('$') => {
                let escaped = match chars.get(i + 1).map(|c| c.to_ascii_uppercase()) {
                    Some('$') => '$',
                    Some('\'') => '\'',
                    Some('"') => '"',
                    Some('L' | 'N') => '\n',
                    Some('P') => '\x0C',
                    Some('R') => '\r',
                    Some('T') => '\t',
                    _ => {
                        let hex: String = chars.iter().skip(i + 1).take(hex_digits).collect();
                        let code = (hex.len() == hex_digits).then(|| u32::from_str_radix(&hex, 16).ok()).flatten();
                        let c = code.and_then(char::from_u32).ok_or_else(|| {
                            format!("Invalid escape '${hex}' in a string (use $$, $', $N, $T or ${} hex digits)", hex_digits)
                        })?;
                        text.push(c);
                        i += 1 + hex_digits;
                        continue;
                    }
                };
                text.push(escaped);
                i += 2;
            }
            Some(&c) => {
                text.push(c);
                i += 1;
            }
        }
    }
}

/// Turn source text into tokens, skipping whitespace, `//` and `(* *)` comments.
/// Never fails: invalid text becomes an `Invalid` token. Always ends with `Eof`.
pub fn tokenize(source: &str) -> Vec<Token> {
    let chars: Vec<char> = source.chars().collect();
    let mut tokens = Vec::new();
    let (mut i, mut line, mut column) = (0usize, 1usize, 1usize);

    while i < chars.len() {
        let c = chars[i];
        let (start_line, start_column) = (line, column);
        let mut push = |kind| tokens.push(Token { kind, line: start_line, column: start_column });

        if c == '\n' {
            i += 1;
            line += 1;
            column = 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            column += 1;
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
                column += 1;
            }
            continue;
        }
        if c == '(' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            column += 2;
            loop {
                if i >= chars.len() {
                    push(TokenKind::Invalid("Unterminated comment: missing '*)'".into()));
                    break;
                }
                if chars[i] == '*' && chars.get(i + 1) == Some(&')') {
                    i += 2;
                    column += 2;
                    break;
                }
                if chars[i] == '\n' {
                    line += 1;
                    column = 1;
                } else {
                    column += 1;
                }
                i += 1;
            }
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            column += i - start;
            let is_time_prefix = word.eq_ignore_ascii_case("T") || word.eq_ignore_ascii_case("TIME");
            if is_time_prefix && chars.get(i) == Some(&'#') {
                let body_start = i + 1;
                i = body_start;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_' || chars[i] == '.') {
                    i += 1;
                }
                let body: String = chars[body_start..i].iter().collect();
                column += i - (body_start - 1);
                push(match parse_duration(&body) {
                    Ok(ms) => TokenKind::TimeLiteral(ms),
                    Err(message) => TokenKind::InvalidTime(message),
                });
                continue;
            }
            if let Some(data_type) = date_prefix(&word).filter(|_| chars.get(i) == Some(&'#')) {
                let body_start = i + 1;
                i = body_start;
                while i < chars.len() && (chars[i].is_ascii_digit() || matches!(chars[i], '-' | ':' | '.' | '_')) {
                    i += 1;
                }
                let body: String = chars[body_start..i].iter().collect();
                column += i - (body_start - 1);
                push(match parse_date_literal(data_type, &body) {
                    Ok(ms) => TokenKind::DateLiteral(data_type, ms),
                    Err(message) => TokenKind::Invalid(message),
                });
                continue;
            }
            if word.eq_ignore_ascii_case("REF") && chars.get(i) == Some(&'=') {
                push(TokenKind::RefAssign);
                i += 1;
                column += 1;
                continue;
            }
            let kind = keyword(&word).unwrap_or(TokenKind::Identifier(word));
            // `INT#5`, `BYTE#16#FF`, `LREAL#1.5`: the parser reads the value after the `#`.
            if matches!(kind, TokenKind::Type(_)) && chars.get(i) == Some(&'#') {
                push(kind);
                tokens.push(Token { kind: TokenKind::Hash, line, column });
                i += 1;
                column += 1;
                continue;
            }
            push(kind);
            continue;
        }
        if c.is_ascii_digit() {
            let start = i;
            let digits = |i: &mut usize, ok: fn(char) -> bool| {
                while *i < chars.len() && (ok(chars[*i]) || chars[*i] == '_') {
                    *i += 1;
                }
            };
            digits(&mut i, |d| d.is_ascii_digit());
            let whole: String = chars[start..i].iter().filter(|&&d| d != '_').collect();
            // `16#FF`, `8#17`, `2#1010_0101`
            if chars.get(i) == Some(&'#') && matches!(whole.as_str(), "2" | "8" | "16") {
                let body_start = i + 1;
                i = body_start;
                digits(&mut i, |d| d.is_ascii_hexdigit());
                let body: String = chars[body_start..i].iter().filter(|&&d| d != '_').collect();
                let text: String = chars[start..i].iter().collect();
                column += i - start;
                let radix = whole.parse().unwrap_or(10);
                push(match i128::from_str_radix(&body, radix) {
                    Ok(n) if n <= u64::MAX.into() => TokenKind::IntLiteral(n),
                    Ok(_) => TokenKind::Invalid(format!("Number '{text}' is too large")),
                    Err(_) => TokenKind::Invalid(format!("Invalid base-{radix} number '{text}'")),
                });
                continue;
            }
            // A '.' only makes a REAL when digits follow it: `3.5`, not `3.`.
            let is_real = chars.get(i) == Some(&'.') && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit());
            if is_real {
                i += 1;
                digits(&mut i, |d| d.is_ascii_digit());
                // Exponent: `1.5E3`, `2.0e-7`.
                if matches!(chars.get(i), Some('e' | 'E')) {
                    let sign = usize::from(matches!(chars.get(i + 1), Some('+' | '-')));
                    if chars.get(i + 1 + sign).is_some_and(|d| d.is_ascii_digit()) {
                        i += 1 + sign;
                        digits(&mut i, |d| d.is_ascii_digit());
                    }
                }
            }
            let text: String = chars[start..i].iter().collect();
            let clean = text.replace('_', "");
            column += i - start;
            let too_large = || TokenKind::Invalid(format!("Number '{text}' is too large"));
            push(if is_real {
                clean.parse::<f64>().ok().filter(|x| x.is_finite()).map(TokenKind::RealLiteral).unwrap_or_else(too_large)
            } else {
                clean.parse().ok().filter(|&n: &i128| n <= u64::MAX.into()).map(TokenKind::IntLiteral).unwrap_or_else(too_large)
            });
            continue;
        }

        if c == '\'' || c == '"' {
            let data_type = if c == '"' { DataType::WString } else { DataType::String };
            match string_literal(&chars[i + 1..], c) {
                Ok((text, len)) => {
                    push(if data_type == DataType::String && text.chars().any(|c| u32::from(c) > 0xFF) {
                        TokenKind::Invalid("A STRING holds only Latin-1 characters; use a WSTRING (\"…\") for others".into())
                    } else {
                        TokenKind::StringLiteral(data_type, text)
                    });
                    i += 1 + len;
                    column += 1 + len;
                }
                Err(message) => {
                    push(TokenKind::Invalid(message));
                    i = chars.len();
                }
            }
            continue;
        }

        let (kind, len) = match (c, chars.get(i + 1)) {
            (':', Some('=')) => (TokenKind::Assign, 2),
            ('<', Some('>')) => (TokenKind::NotEqual, 2),
            ('<', Some('=')) => (TokenKind::LessEqual, 2),
            ('>', Some('=')) => (TokenKind::GreaterEqual, 2),
            (':', _) => (TokenKind::Colon, 1),
            ('=', _) => (TokenKind::Equal, 1),
            ('<', _) => (TokenKind::Less, 1),
            ('>', _) => (TokenKind::Greater, 1),
            ('+', _) => (TokenKind::Plus, 1),
            ('-', _) => (TokenKind::Minus, 1),
            ('*', _) => (TokenKind::Star, 1),
            ('/', _) => (TokenKind::Slash, 1),
            ('(', _) => (TokenKind::LParen, 1),
            (')', _) => (TokenKind::RParen, 1),
            (',', _) => (TokenKind::Comma, 1),
            ('.', _) => (TokenKind::Dot, 1),
            (';', _) => (TokenKind::Semicolon, 1),
            ('[', _) => (TokenKind::LBracket, 1),
            ('^', _) => (TokenKind::Caret, 1),
            ('&', _) => (TokenKind::And, 1),
            (']', _) => (TokenKind::RBracket, 1),
            _ => (TokenKind::Invalid(format!("Unexpected character '{c}'")), 1),
        };
        push(kind);
        i += len;
        column += len;
    }

    tokens.push(Token { kind: TokenKind::Eof, line, column });
    tokens
}

