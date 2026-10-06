use super::ast::DataType;
use super::value::format_time;

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
    IntLiteral(i64),
    RealLiteral(f64),
    /// `T#…` / `TIME#…`, in milliseconds.
    TimeLiteral(i64),
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
        "BOOL" => TokenKind::Type(DataType::Bool),
        "INT" => TokenKind::Type(DataType::Int),
        "DINT" => TokenKind::Type(DataType::DInt),
        "REAL" => TokenKind::Type(DataType::Real),
        "TIME" => TokenKind::Type(DataType::Time),
        "TRUE" => TokenKind::True,
        "FALSE" => TokenKind::False,
        "AND" => TokenKind::And,
        "OR" => TokenKind::Or,
        "NOT" => TokenKind::Not,
        "XOR" => TokenKind::Xor,
        "MOD" => TokenKind::Mod,
        _ => return None,
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
            push(keyword(&word).unwrap_or(TokenKind::Identifier(word)));
            continue;
        }
        if c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            // A '.' only makes a REAL when digits follow it: `3.5`, not `3.`.
            let is_real = chars.get(i) == Some(&'.') && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit());
            if is_real {
                i += 1;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
            }
            let text: String = chars[start..i].iter().collect();
            column += i - start;
            let too_large = || TokenKind::Invalid(format!("Number '{text}' is too large"));
            push(if is_real {
                text.parse().map(TokenKind::RealLiteral).unwrap_or_else(|_| too_large())
            } else {
                text.parse().map(TokenKind::IntLiteral).unwrap_or_else(|_| too_large())
            });
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
            _ => (TokenKind::Invalid(format!("Unexpected character '{c}'")), 1),
        };
        push(kind);
        i += len;
        column += len;
    }

    tokens.push(Token { kind: TokenKind::Eof, line, column });
    tokens
}

