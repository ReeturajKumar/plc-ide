use super::ast::{
    BinaryOp, CallArg, CaseBranch, CaseLabel, Expr, ExprKind, FunctionBlockDef, IfBranch, Program, SourceFile, Stmt,
    UnaryOp, VarDecl, VarType,
};
use super::error::{ErrorKind, StError, StResult};
use super::function_blocks::FbKind;
use super::lexer::{tokenize, Token, TokenKind};
use super::value::Value;

/// Limit on expression/statement depth. Deeply nested input must produce an error, not a
/// stack overflow that kills the IDE: the parser, checker and runtime all recurse over the
/// tree. The simulator runs them on threads sized for this limit (`ENGINE_STACK_BYTES`).
pub const MAX_NESTING: usize = 100;

/// Parse ST source that is exactly one PROGRAM.
#[cfg(test)]
pub fn parse(source: &str) -> StResult<Program> {
    let mut parser = Parser { tokens: tokenize(source), pos: 0, depth: 0 };
    let program = parser.program()?;
    parser.expect(TokenKind::Eof, "end of file after END_PROGRAM")?;
    Ok(program)
}

/// Parse a project source file: FUNCTION_BLOCKs and at most one PROGRAM, in any order.
pub fn parse_file(source: &str) -> StResult<SourceFile> {
    let mut parser = Parser { tokens: tokenize(source), pos: 0, depth: 0 };
    let mut file = SourceFile::default();
    loop {
        match parser.peek().kind {
            TokenKind::FunctionBlock => file.function_blocks.push(parser.function_block()?),
            TokenKind::Program if file.program.is_some() => {
                let token = parser.peek().clone();
                return Err(parser.error_at(&token, "A file can contain only one PROGRAM; put the other one in its own file"));
            }
            TokenKind::Program => file.program = Some(parser.program()?),
            TokenKind::Eof if file.program.is_some() || !file.function_blocks.is_empty() => return Ok(file),
            _ => return Err(parser.error_expected("PROGRAM or FUNCTION_BLOCK")),
        }
    }
}

/// IEC elementary type names this engine doesn't support yet. They are reserved, so they
/// can't name a FUNCTION_BLOCK: say so right away instead of "unknown type".
const UNSUPPORTED_TYPES: &[&str] = &[
    "STRING", "WSTRING", "BYTE", "WORD", "DWORD", "LWORD", "SINT", "USINT", "UINT", "UDINT", "LINT", "ULINT", "LREAL",
    "DATE", "TOD", "TIME_OF_DAY", "DT", "DATE_AND_TIME", "CHAR", "WCHAR", "ARRAY", "STRUCT", "POINTER", "REFERENCE",
];

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    depth: usize,
}

impl Parser {
    // --- token helpers (the token list always ends in Eof, so indexing is safe) ---

    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn advance(&mut self) -> Token {
        let token = self.tokens[self.pos].clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        token
    }

    fn check(&self, kind: &TokenKind) -> bool {
        &self.peek().kind == kind
    }

    fn expect(&mut self, kind: TokenKind, expected: &str) -> StResult<Token> {
        if self.check(&kind) {
            Ok(self.advance())
        } else {
            Err(self.error_expected(expected))
        }
    }

    fn error_expected(&self, expected: &str) -> StError {
        let t = self.peek();
        // Text the lexer couldn't read is the real problem, whatever was expected here.
        let (kind, message) = match &t.kind {
            TokenKind::Invalid(message) => (ErrorKind::Syntax, message.clone()),
            TokenKind::InvalidTime(message) => (ErrorKind::InvalidTimeValue, message.clone()),
            other => (ErrorKind::Syntax, format!("Expected {expected} but found {}", other.describe())),
        };
        StError::new(kind, t.line, t.column, message)
    }

    fn error_at(&self, token: &Token, message: impl Into<String>) -> StError {
        StError::new(ErrorKind::Syntax, token.line, token.column, message)
    }

    fn identifier(&mut self, expected: &str) -> StResult<(String, usize, usize)> {
        let token = self.peek().clone();
        match token.kind {
            TokenKind::Identifier(name) => {
                self.advance();
                Ok((name, token.line, token.column))
            }
            _ => Err(self.error_expected(expected)),
        }
    }

    fn enter(&mut self) -> StResult<()> {
        self.depth += 1;
        if self.depth > MAX_NESTING {
            let t = self.peek();
            return Err(StError::new(ErrorKind::Syntax, t.line, t.column, "Code is nested too deeply"));
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.depth -= 1;
    }

    // --- program structure ---

    fn program(&mut self) -> StResult<Program> {
        self.expect(TokenKind::Program, "PROGRAM")?;
        let (name, _, _) = self.identifier("a program name")?;
        let mut vars = Vec::new();
        while self.check(&TokenKind::Var) {
            self.var_block(&mut vars)?;
        }
        let body = self.block()?;
        self.expect(TokenKind::EndProgram, "END_PROGRAM")?;
        Ok(Program { name, vars, body })
    }

    fn function_block(&mut self) -> StResult<FunctionBlockDef> {
        let keyword = self.advance(); // FUNCTION_BLOCK
        let (name, _, _) = self.identifier("a function block name")?;
        let (mut inputs, mut outputs, mut vars) = (Vec::new(), Vec::new(), Vec::new());
        loop {
            match self.peek().kind {
                TokenKind::VarInput => self.var_block(&mut inputs)?,
                TokenKind::VarOutput => self.var_block(&mut outputs)?,
                TokenKind::Var => self.var_block(&mut vars)?,
                _ => break,
            }
        }
        let body = self.block()?;
        self.expect(TokenKind::EndFunctionBlock, "END_FUNCTION_BLOCK")?;
        Ok(FunctionBlockDef { name, inputs, outputs, vars, body, line: keyword.line, column: keyword.column, file: String::new() })
    }

    fn var_block(&mut self, vars: &mut Vec<VarDecl>) -> StResult<()> {
        self.advance(); // VAR
        while !self.check(&TokenKind::EndVar) {
            vars.push(self.var_decl()?);
        }
        self.advance(); // END_VAR
        Ok(())
    }

    fn var_decl(&mut self) -> StResult<VarDecl> {
        let (name, line, column) = self.identifier("a variable name or END_VAR")?;
        self.expect(TokenKind::Colon, "':'")?;
        let type_name = match &self.peek().kind {
            TokenKind::Identifier(name) => name.clone(),
            other => other.describe(),
        };
        let var_type = self.var_type()?;
        let init = if self.check(&TokenKind::Assign) {
            self.advance();
            Some(self.expression()?)
        } else {
            None
        };
        self.expect(TokenKind::Semicolon, "';'")?;
        Ok(VarDecl { name, var_type, type_name, init, line, column })
    }

    fn var_type(&mut self) -> StResult<VarType> {
        let token = self.peek().clone();
        let var_type = match &token.kind {
            TokenKind::Type(data_type) => VarType::Elementary(*data_type),
            TokenKind::Identifier(name) => match FbKind::from_name(name) {
                Some(kind) => VarType::FunctionBlock(kind),
                None if UNSUPPORTED_TYPES.contains(&name.to_ascii_uppercase().as_str()) => {
                    return Err(self.error_at(
                        &token,
                        format!(
                            "Type '{name}' is not supported yet (supported: BOOL, INT, DINT, REAL, TIME, the standard function blocks and the project's FUNCTION_BLOCKs)"
                        ),
                    ))
                }
                // Maybe a FUNCTION_BLOCK of the project; the checker resolves it.
                None => VarType::UNRESOLVED,
            },
            _ => return Err(self.error_expected("a data type")),
        };
        self.advance();
        Ok(var_type)
    }

    // --- statements ---

    /// Statements up to the next block-closing keyword. Stopping at *any* closer (not just
    /// the expected one) lets the enclosing construct report what it was waiting for,
    /// e.g. "Expected END_IF but found END_PROGRAM" rather than "Expected a statement".
    fn block(&mut self) -> StResult<Vec<Stmt>> {
        let mut stmts = Vec::new();
        while !self.at_block_end() {
            stmts.push(self.statement()?);
        }
        Ok(stmts)
    }

    fn at_block_end(&self) -> bool {
        matches!(
            self.peek().kind,
            TokenKind::EndProgram
                | TokenKind::EndFunctionBlock
                | TokenKind::EndIf
                | TokenKind::Elsif
                | TokenKind::Else
                | TokenKind::EndFor
                | TokenKind::EndWhile
                | TokenKind::Until
                | TokenKind::EndRepeat
                | TokenKind::EndCase
                | TokenKind::Eof
        )
    }

    fn statement(&mut self) -> StResult<Stmt> {
        match self.peek().kind.clone() {
            TokenKind::If => self.if_statement(),
            TokenKind::For => self.for_statement(),
            TokenKind::While => self.while_statement(),
            TokenKind::Repeat => self.repeat_statement(),
            TokenKind::Case => self.case_statement(),
            TokenKind::Identifier(_) => match self.tokens.get(self.pos + 1).map(|t| &t.kind) {
                Some(TokenKind::LParen) => self.call(),
                Some(TokenKind::Dot) => self.member_assignment(),
                _ => self.assignment(),
            },
            _ => Err(self.error_expected("a statement")),
        }
    }

    /// Accept the optional `;` after END_IF, END_FOR, … (the standard writes it, many
    /// editors omit it).
    fn optional_semicolon(&mut self) {
        if self.check(&TokenKind::Semicolon) {
            self.advance();
        }
    }

    fn assignment(&mut self) -> StResult<Stmt> {
        let (target, line, column) = self.identifier("a variable name")?;
        self.expect(TokenKind::Assign, "':='")?;
        let value = self.expression()?;
        self.expect(TokenKind::Semicolon, "';'")?;
        Ok(Stmt::Assign { target, value, line, column })
    }

    /// `Instance.Member := expr;` — never valid, but parsed so the checker can say why.
    fn member_assignment(&mut self) -> StResult<Stmt> {
        let (instance, line, column) = self.identifier("a function block instance")?;
        self.advance(); // .
        let (member, _, _) = self.identifier("a member name")?;
        self.expect(TokenKind::Assign, "':='")?;
        let value = self.expression()?;
        self.expect(TokenKind::Semicolon, "';'")?;
        Ok(Stmt::AssignMember { instance, member, value, line, column })
    }

    /// `Instance(Name := expr, …);`
    fn call(&mut self) -> StResult<Stmt> {
        let (instance, line, column) = self.identifier("a function block instance")?;
        self.advance(); // (
        let mut args = Vec::new();
        if !self.check(&TokenKind::RParen) {
            loop {
                let (name, arg_line, arg_column) = self.identifier("an input name, e.g. IN := …")?;
                self.expect(TokenKind::Assign, "':='")?;
                let value = self.expression()?;
                args.push(CallArg { name, value, line: arg_line, column: arg_column });
                if !self.check(&TokenKind::Comma) {
                    break;
                }
                self.advance();
            }
        }
        self.expect(TokenKind::RParen, "',' or ')'")?;
        self.expect(TokenKind::Semicolon, "';'")?;
        Ok(Stmt::Call { instance, args, line, column })
    }

    fn if_statement(&mut self) -> StResult<Stmt> {
        self.enter()?;
        let keyword = self.advance(); // IF
        let (line, column) = (keyword.line, keyword.column);
        let mut branches = vec![self.if_branch()?];
        while self.check(&TokenKind::Elsif) {
            self.advance();
            branches.push(self.if_branch()?);
        }
        let else_branch = if self.check(&TokenKind::Else) {
            self.advance();
            self.block()?
        } else {
            Vec::new()
        };
        self.expect(TokenKind::EndIf, "END_IF")?;
        self.optional_semicolon();
        self.leave();
        Ok(Stmt::If { branches, else_branch, line, column })
    }

    /// `<condition> THEN <statements>` — shared by IF and ELSIF.
    fn if_branch(&mut self) -> StResult<IfBranch> {
        let condition = self.expression()?;
        self.expect(TokenKind::Then, "THEN")?;
        Ok(IfBranch { condition, body: self.block()? })
    }

    /// `FOR var := start TO end [BY step] DO … END_FOR`
    fn for_statement(&mut self) -> StResult<Stmt> {
        self.enter()?;
        let keyword = self.advance(); // FOR
        let (var, _, _) = self.identifier("a loop variable")?;
        self.expect(TokenKind::Assign, "':='")?;
        let start = self.expression()?;
        self.expect(TokenKind::To, "TO")?;
        let end = self.expression()?;
        let step = if self.check(&TokenKind::By) {
            self.advance();
            Some(self.expression()?)
        } else {
            None
        };
        self.expect(TokenKind::Do, "DO")?;
        let body = self.block()?;
        self.expect(TokenKind::EndFor, "END_FOR")?;
        self.optional_semicolon();
        self.leave();
        Ok(Stmt::For { var, start, end, step, body, line: keyword.line, column: keyword.column })
    }

    fn while_statement(&mut self) -> StResult<Stmt> {
        self.enter()?;
        let keyword = self.advance(); // WHILE
        let condition = self.expression()?;
        self.expect(TokenKind::Do, "DO")?;
        let body = self.block()?;
        self.expect(TokenKind::EndWhile, "END_WHILE")?;
        self.optional_semicolon();
        self.leave();
        Ok(Stmt::While { condition, body, line: keyword.line, column: keyword.column })
    }

    fn repeat_statement(&mut self) -> StResult<Stmt> {
        self.enter()?;
        let keyword = self.advance(); // REPEAT
        let body = self.block()?;
        self.expect(TokenKind::Until, "UNTIL")?;
        let condition = self.expression()?;
        self.expect(TokenKind::EndRepeat, "END_REPEAT")?;
        self.optional_semicolon();
        self.leave();
        Ok(Stmt::Repeat { body, condition, line: keyword.line, column: keyword.column })
    }

    /// `CASE selector OF 1: … 2, 3: … ELSE … END_CASE`
    fn case_statement(&mut self) -> StResult<Stmt> {
        self.enter()?;
        let keyword = self.advance(); // CASE
        let selector = self.expression()?;
        self.expect(TokenKind::Of, "OF")?;
        let mut branches = Vec::new();
        while self.at_case_label() {
            let mut labels = vec![self.case_label()?];
            while self.check(&TokenKind::Comma) {
                self.advance();
                labels.push(self.case_label()?);
            }
            self.expect(TokenKind::Colon, "',' or ':' after the CASE label")?;
            // A branch runs until the next label, ELSE or END_CASE.
            let mut body = Vec::new();
            while !self.at_block_end() && !self.at_case_label() {
                body.push(self.statement()?);
            }
            branches.push(CaseBranch { labels, body });
        }
        if branches.is_empty() {
            return Err(self.error_expected("a CASE label such as 0:"));
        }
        let else_branch = if self.check(&TokenKind::Else) {
            self.advance();
            self.block()?
        } else {
            Vec::new()
        };
        self.expect(TokenKind::EndCase, "a CASE label, ELSE or END_CASE")?;
        self.optional_semicolon();
        self.leave();
        Ok(Stmt::Case { selector, branches, else_branch, line: keyword.line, column: keyword.column })
    }

    /// Labels are integer literals, optionally negative. No statement can start with
    /// either, so this also tells a new label apart from the previous branch's body.
    fn at_case_label(&self) -> bool {
        match self.peek().kind {
            TokenKind::IntLiteral(_) => true,
            TokenKind::Minus => matches!(self.tokens.get(self.pos + 1).map(|t| &t.kind), Some(TokenKind::IntLiteral(_))),
            _ => false,
        }
    }

    fn case_label(&mut self) -> StResult<CaseLabel> {
        let start = self.peek().clone();
        let negative = self.check(&TokenKind::Minus);
        if negative {
            self.advance();
        }
        let token = self.peek().clone();
        let TokenKind::IntLiteral(n) = token.kind else {
            return Err(self.error_expected("an integer CASE label"));
        };
        self.advance();
        if self.check(&TokenKind::Dot) {
            return Err(self.error_at(&token, "CASE ranges (e.g. 1..5) are not supported yet; list the values: 1, 2, 3:"));
        }
        Ok(CaseLabel { value: if negative { -n } else { n }, line: start.line, column: start.column })
    }

    // --- expressions: IEC 61131-3 precedence, loosest first ---
    //   OR < XOR < AND < = <> < < > <= >= < + - < * / MOD < unary NOT, - < ( )

    fn expression(&mut self) -> StResult<Expr> {
        self.or_expr()
    }

    /// One left-associative precedence level: `next { op next }`.
    fn binary_level(
        &mut self,
        next: fn(&mut Self) -> StResult<Expr>,
        op_for: fn(&TokenKind) -> Option<BinaryOp>,
    ) -> StResult<Expr> {
        let mut left = next(self)?;
        let mut chained = 0;
        while let Some(op) = op_for(&self.peek().kind) {
            // Each operator deepens the (left-leaning) tree even though parsing it is a
            // loop, and later passes recurse over that tree, so it counts toward the limit.
            self.enter()?;
            chained += 1;
            let token = self.advance();
            let right = next(self)?;
            left = Expr {
                kind: ExprKind::Binary { op, left: Box::new(left), right: Box::new(right) },
                line: token.line,
                column: token.column,
            };
        }
        self.depth -= chained;
        Ok(left)
    }

    fn or_expr(&mut self) -> StResult<Expr> {
        self.binary_level(Self::xor_expr, |k| matches!(k, TokenKind::Or).then_some(BinaryOp::Or))
    }

    fn xor_expr(&mut self) -> StResult<Expr> {
        self.binary_level(Self::and_expr, |k| matches!(k, TokenKind::Xor).then_some(BinaryOp::Xor))
    }

    fn and_expr(&mut self) -> StResult<Expr> {
        self.binary_level(Self::equality, |k| matches!(k, TokenKind::And).then_some(BinaryOp::And))
    }

    fn equality(&mut self) -> StResult<Expr> {
        self.binary_level(Self::relational, |k| match k {
            TokenKind::Equal => Some(BinaryOp::Equal),
            TokenKind::NotEqual => Some(BinaryOp::NotEqual),
            _ => None,
        })
    }

    fn relational(&mut self) -> StResult<Expr> {
        self.binary_level(Self::additive, |k| match k {
            TokenKind::Less => Some(BinaryOp::Less),
            TokenKind::Greater => Some(BinaryOp::Greater),
            TokenKind::LessEqual => Some(BinaryOp::LessEqual),
            TokenKind::GreaterEqual => Some(BinaryOp::GreaterEqual),
            _ => None,
        })
    }

    fn additive(&mut self) -> StResult<Expr> {
        self.binary_level(Self::multiplicative, |k| match k {
            TokenKind::Plus => Some(BinaryOp::Add),
            TokenKind::Minus => Some(BinaryOp::Sub),
            _ => None,
        })
    }

    fn multiplicative(&mut self) -> StResult<Expr> {
        self.binary_level(Self::unary, |k| match k {
            TokenKind::Star => Some(BinaryOp::Mul),
            TokenKind::Slash => Some(BinaryOp::Div),
            TokenKind::Mod => Some(BinaryOp::Mod),
            _ => None,
        })
    }

    fn unary(&mut self) -> StResult<Expr> {
        self.enter()?;
        let token = self.peek().clone();
        let expr = match token.kind {
            TokenKind::Not => {
                self.advance();
                let operand = self.unary()?;
                Expr { kind: ExprKind::Unary { op: UnaryOp::Not, operand: Box::new(operand) }, line: token.line, column: token.column }
            }
            TokenKind::Minus => {
                self.advance();
                let operand = self.unary()?;
                // Fold `-literal` into a negative literal so e.g. `-32768` is a valid INT
                // (negating +32768 would overflow INT before the sign applied).
                let kind = match operand.kind {
                    ExprKind::IntLiteral(n) => ExprKind::IntLiteral(-n),
                    ExprKind::RealLiteral(x) => ExprKind::RealLiteral(-x),
                    _ => ExprKind::Unary { op: UnaryOp::Neg, operand: Box::new(operand) },
                };
                Expr { kind, line: token.line, column: token.column }
            }
            _ => self.primary()?,
        };
        self.leave();
        Ok(expr)
    }

    fn primary(&mut self) -> StResult<Expr> {
        let token = self.peek().clone();
        let kind = match token.kind {
            TokenKind::True => ExprKind::Const(Value::Bool(true)),
            TokenKind::False => ExprKind::Const(Value::Bool(false)),
            TokenKind::IntLiteral(n) => ExprKind::IntLiteral(n),
            TokenKind::RealLiteral(x) => ExprKind::RealLiteral(x),
            TokenKind::TimeLiteral(ms) => ExprKind::Const(Value::Time(ms)),
            TokenKind::Identifier(name) => {
                self.advance();
                let kind = if self.check(&TokenKind::LParen) {
                    // Which names are functions is the checker's business; any `name(…)`
                    // in an expression is a call.
                    ExprKind::Call { name, args: self.call_arguments()? }
                } else if self.check(&TokenKind::Dot) {
                    self.advance(); // .
                    let (member, _, _) = self.identifier("a member name, e.g. Q")?;
                    ExprKind::Member { instance: name, member }
                } else {
                    ExprKind::Variable(name)
                };
                return Ok(Expr { kind, line: token.line, column: token.column });
            }
            TokenKind::LParen => {
                self.advance();
                let expr = self.expression()?;
                self.expect(TokenKind::RParen, "')'")?;
                return Ok(expr);
            }
            _ => return Err(self.error_expected("an expression")),
        };
        self.advance();
        Ok(Expr { kind, line: token.line, column: token.column })
    }

    /// `( expr, expr, … )` — positional function arguments, possibly none.
    fn call_arguments(&mut self) -> StResult<Vec<Expr>> {
        self.advance(); // (
        let mut args = Vec::new();
        if !self.check(&TokenKind::RParen) {
            loop {
                args.push(self.expression()?);
                if !self.check(&TokenKind::Comma) {
                    break;
                }
                self.advance();
            }
        }
        self.expect(TokenKind::RParen, "',' or ')'")?;
        Ok(args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::st::ast::DataType;

    /// Tree shape without positions, for structural comparisons.
    #[derive(Debug, PartialEq)]
    enum T {
        Const(Value),
        Int(i64),
        Real(f64),
        Var(String),
        Member(String, String),
        Call(String, Vec<T>),
        Not(Box<T>),
        Neg(Box<T>),
        Bin(BinaryOp, Box<T>, Box<T>),
    }

    fn shape(e: &Expr) -> T {
        match &e.kind {
            ExprKind::Const(v) => T::Const(*v),
            ExprKind::IntLiteral(n) => T::Int(*n),
            ExprKind::RealLiteral(x) => T::Real(*x),
            ExprKind::Variable(name) => T::Var(name.clone()),
            ExprKind::Member { instance, member } => T::Member(instance.clone(), member.clone()),
            ExprKind::Unary { op: UnaryOp::Not, operand } => T::Not(Box::new(shape(operand))),
            ExprKind::Unary { op: UnaryOp::Neg, operand } => T::Neg(Box::new(shape(operand))),
            ExprKind::Binary { op, left, right } => T::Bin(*op, Box::new(shape(left)), Box::new(shape(right))),
            ExprKind::Call { name, args } => T::Call(name.clone(), args.iter().map(shape).collect()),
            ExprKind::Convert { .. } => panic!("the parser never produces Convert"),
        }
    }

    fn v(name: &str) -> T {
        T::Var(name.to_string())
    }

    fn bin(op: BinaryOp, l: T, r: T) -> T {
        T::Bin(op, Box::new(l), Box::new(r))
    }

    /// Parse a single expression via `x := <expr>;`.
    fn expr(src: &str) -> T {
        let program = parse(&format!("PROGRAM p x := {src}; END_PROGRAM")).unwrap();
        match &program.body[0] {
            Stmt::Assign { value, .. } => shape(value),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_variable_declarations_and_bool_init() {
        let program = parse(
            "PROGRAM main VAR Start : BOOL := FALSE; Motor : BOOL := TRUE; Idle : BOOL; END_VAR END_PROGRAM",
        )
        .unwrap();
        assert_eq!(program.name, "main");
        assert_eq!(program.vars.len(), 3);
        assert_eq!(program.vars[0].name, "Start");
        assert_eq!(program.vars[0].var_type, VarType::Elementary(DataType::Bool));
        assert_eq!(shape(program.vars[0].init.as_ref().unwrap()), T::Const(Value::Bool(false)));
        assert_eq!(shape(program.vars[1].init.as_ref().unwrap()), T::Const(Value::Bool(true)));
        assert_eq!(program.vars[2].init, None);
    }

    #[test]
    fn parses_numeric_declarations() {
        let program = parse(
            "PROGRAM p VAR c : INT := 10; d : DINT := -10; t : REAL := 24.5; END_VAR END_PROGRAM",
        )
        .unwrap();
        let types: Vec<_> = program.vars.iter().map(|v| v.var_type).collect();
        let elementary = |t| VarType::Elementary(t);
        assert_eq!(types, vec![elementary(DataType::Int), elementary(DataType::DInt), elementary(DataType::Real)]);
        assert_eq!(shape(program.vars[0].init.as_ref().unwrap()), T::Int(10));
        assert_eq!(shape(program.vars[1].init.as_ref().unwrap()), T::Int(-10));
        assert_eq!(shape(program.vars[2].init.as_ref().unwrap()), T::Real(24.5));
    }

    #[test]
    fn parses_assignment_with_position() {
        let program = parse("PROGRAM p Motor := TRUE; END_PROGRAM").unwrap();
        match &program.body[0] {
            Stmt::Assign { target, value, line, column } => {
                assert_eq!((target.as_str(), *line, *column), ("Motor", 1, 11));
                assert_eq!(shape(value), T::Const(Value::Bool(true)));
            }
            other => panic!("expected assignment, got {other:?}"),
        }
    }

    #[test]
    fn parses_if_with_and_without_else() {
        let program = parse(
            "PROGRAM p IF Start THEN Motor := TRUE; END_IF; IF Start THEN Motor := TRUE; ELSE Motor := FALSE; END_IF END_PROGRAM",
        )
        .unwrap();
        match &program.body[0] {
            Stmt::If { branches, else_branch, .. } => {
                assert_eq!(branches.len(), 1);
                assert_eq!(branches[0].body.len(), 1);
                assert!(else_branch.is_empty());
            }
            other => panic!("expected IF, got {other:?}"),
        }
        match &program.body[1] {
            Stmt::If { branches, else_branch, .. } => {
                assert_eq!(branches[0].body.len(), 1);
                assert_eq!(else_branch.len(), 1);
            }
            other => panic!("expected IF, got {other:?}"),
        }
    }

    #[test]
    fn parses_elsif_chain() {
        let program = parse(
            "PROGRAM p IF a THEN x := 1; ELSIF b THEN x := 2; ELSIF c THEN x := 3; x := 4; ELSE x := 5; END_IF; END_PROGRAM",
        )
        .unwrap();
        match &program.body[0] {
            Stmt::If { branches, else_branch, .. } => {
                let conditions: Vec<_> = branches.iter().map(|b| shape(&b.condition)).collect();
                assert_eq!(conditions, vec![v("a"), v("b"), v("c")]);
                let sizes: Vec<_> = branches.iter().map(|b| b.body.len()).collect();
                assert_eq!(sizes, vec![1, 1, 2]);
                assert_eq!(else_branch.len(), 1);
            }
            other => panic!("expected IF, got {other:?}"),
        }
        let err = parse("PROGRAM p IF a THEN x := 1; ELSE x := 2; ELSIF b THEN x := 3; END_IF; END_PROGRAM").unwrap_err();
        assert_eq!(err.message, "Expected END_IF but found ELSIF", "ELSIF can't follow ELSE");
    }

    #[test]
    fn parses_function_blocks_and_time() {
        let program = parse(
            "PROGRAM p VAR t : TON; c : ctu; d : TIME := T#1m30s; END_VAR
             t(IN := a AND b, PT := T#2S);
             c();
             x := t.Q AND c.cv > 3;
             END_PROGRAM",
        )
        .unwrap();
        let types: Vec<_> = program.vars.iter().map(|v| v.var_type).collect();
        assert_eq!(
            types,
            vec![
                VarType::FunctionBlock(FbKind::Ton),
                VarType::FunctionBlock(FbKind::Ctu),
                VarType::Elementary(DataType::Time)
            ]
        );
        assert_eq!(shape(program.vars[2].init.as_ref().unwrap()), T::Const(Value::Time(90_000)));

        match &program.body[0] {
            Stmt::Call { instance, args, line, column } => {
                assert_eq!((instance.as_str(), *line, *column), ("t", 2, 14));
                let names: Vec<_> = args.iter().map(|a| a.name.as_str()).collect();
                assert_eq!(names, vec!["IN", "PT"]);
                assert_eq!(shape(&args[0].value), bin(BinaryOp::And, v("a"), v("b")));
                assert_eq!(shape(&args[1].value), T::Const(Value::Time(2_000)));
            }
            other => panic!("expected call, got {other:?}"),
        }
        assert!(matches!(&program.body[1], Stmt::Call { args, .. } if args.is_empty()));
        assert_eq!(
            expr("t.Q AND c.cv > 3"),
            bin(
                BinaryOp::And,
                T::Member("t".into(), "Q".into()),
                bin(BinaryOp::Greater, T::Member("c".into(), "cv".into()), T::Int(3))
            )
        );
    }

    #[test]
    fn reports_call_syntax_errors() {
        let err = parse("PROGRAM p t(IN := TRUE PT := T#1s); END_PROGRAM").unwrap_err();
        assert!(err.message.contains("Expected ',' or ')'"), "{}", err.message);
        let err = parse("PROGRAM p t(TRUE); END_PROGRAM").unwrap_err();
        assert!(err.message.contains("an input name"), "{}", err.message);
        // Member assignment parses; the checker explains why it isn't allowed.
        let program = parse("PROGRAM p t.Q := TRUE; END_PROGRAM").unwrap();
        assert!(matches!(&program.body[0], Stmt::AssignMember { instance, member, .. } if instance == "t" && member == "Q"));
    }

    #[test]
    fn mod_and_xor_precedence() {
        use BinaryOp::*;
        // MOD binds like * and /.
        assert_eq!(expr("a + b MOD c"), bin(Add, v("a"), bin(Mod, v("b"), v("c"))));
        // IEC: AND binds tighter than XOR, which binds tighter than OR.
        assert_eq!(expr("a OR b XOR c AND d"), bin(Or, v("a"), bin(Xor, v("b"), bin(And, v("c"), v("d")))));
        assert_eq!(expr("a XOR b XOR c"), bin(Xor, bin(Xor, v("a"), v("b")), v("c")));
    }

    /// Parse a program body and return its statements.
    fn body(src: &str) -> Vec<Stmt> {
        parse(&format!("PROGRAM p {src} END_PROGRAM")).unwrap().body
    }

    #[test]
    fn parses_loops() {
        match &body("FOR i := 1 TO 10 BY 2 DO s := s + i; END_FOR;")[0] {
            Stmt::For { var, start, end, step, body, line, column } => {
                assert_eq!((var.as_str(), *line, *column), ("i", 1, 11));
                assert_eq!((shape(start), shape(end)), (T::Int(1), T::Int(10)));
                assert_eq!(step.as_ref().map(shape), Some(T::Int(2)));
                assert_eq!(body.len(), 1);
            }
            other => panic!("expected FOR, got {other:?}"),
        }
        assert!(matches!(&body("FOR i := 5 TO 1 BY -1 DO END_FOR")[0], Stmt::For { step: Some(s), .. } if shape(s) == T::Int(-1)));
        assert!(matches!(&body("FOR i := 1 TO 3 DO END_FOR;")[0], Stmt::For { step: None, .. }));
        assert!(matches!(&body("WHILE c < 5 DO c := c + 1; END_WHILE;")[0], Stmt::While { body, .. } if body.len() == 1));
        assert!(matches!(
            &body("REPEAT c := c + 1; UNTIL c >= 5 END_REPEAT;")[0],
            Stmt::Repeat { body, condition, .. } if body.len() == 1 && matches!(shape(condition), T::Bin(BinaryOp::GreaterEqual, _, _))
        ));
    }

    #[test]
    fn parses_case() {
        let stmts = body(
            "CASE Mode OF
                0: Motor := FALSE;
                1, 2, -3: Motor := TRUE; Fan := TRUE;
                4:
             ELSE
                Motor := FALSE;
             END_CASE;",
        );
        match &stmts[0] {
            Stmt::Case { selector, branches, else_branch, .. } => {
                assert_eq!(shape(selector), v("Mode"));
                let labels: Vec<Vec<i64>> = branches.iter().map(|b| b.labels.iter().map(|l| l.value).collect()).collect();
                assert_eq!(labels, vec![vec![0], vec![1, 2, -3], vec![4]]);
                let sizes: Vec<_> = branches.iter().map(|b| b.body.len()).collect();
                assert_eq!(sizes, vec![1, 2, 0]);
                assert_eq!(else_branch.len(), 1);
            }
            other => panic!("expected CASE, got {other:?}"),
        }
    }

    #[test]
    fn reports_loop_and_case_syntax_errors() {
        for (src, fragment) in [
            ("FOR i = 1 TO 5 DO END_FOR;", "Expected ':='"),
            ("FOR i := 1 5 DO END_FOR;", "Expected TO"),
            ("FOR i := 1 TO 5 x := 1; END_FOR;", "Expected DO"),
            ("FOR i := 1 TO 5 DO x := 1;", "Expected END_FOR"),
            ("WHILE TRUE x := 1; END_WHILE;", "Expected DO"),
            ("REPEAT x := 1; END_REPEAT;", "Expected UNTIL"),
            ("CASE x OF END_CASE;", "Expected a CASE label"),
            ("CASE x OF 1 x := 1; END_CASE;", "Expected ',' or ':'"),
            ("CASE x OF 1..3: x := 1; END_CASE;", "CASE ranges"),
            ("CASE x OF 1: x := 1; END_IF;", "Expected a CASE label, ELSE or END_CASE"),
            ("CASE x OF a: x := 1; END_CASE;", "Expected a CASE label"),
        ] {
            let err = parse(&format!("PROGRAM p {src} END_PROGRAM")).unwrap_err();
            assert!(err.message.contains(fragment), "{src}: {}", err.message);
        }
    }

    #[test]
    fn reports_the_first_problem_in_source_order() {
        // Regression: a character the lexer didn't know on line 3 used to hide the
        // unsupported type on line 2.
        let err = parse("PROGRAM p\nVAR x : STRING; END_VAR\nx(a := 1, b := 2);\nEND_PROGRAM").unwrap_err();
        assert_eq!((err.line, err.column), (2, 9));
        assert!(err.message.starts_with("Type 'STRING' is not supported"));

        let err = parse("PROGRAM p\nx := 1 @ 2;\nEND_PROGRAM").unwrap_err();
        assert_eq!((err.line, err.column, err.message.as_str()), (2, 8, "Unexpected character '@'"));
        let err = parse("PROGRAM p\nt(PT := T#5x);\nEND_PROGRAM").unwrap_err();
        assert!(err.message.starts_with("Invalid TIME literal 'T#5x'"), "{}", err.message);
    }

    #[test]
    fn boolean_operator_precedence() {
        use BinaryOp::*;
        assert_eq!(
            expr("a OR b AND NOT c"),
            bin(Or, v("a"), bin(And, v("b"), T::Not(Box::new(v("c")))))
        );
        assert_eq!(
            expr("a = b AND c <> d"),
            bin(And, bin(Equal, v("a"), v("b")), bin(NotEqual, v("c"), v("d")))
        );
        assert_eq!(expr("(a OR b) AND c"), bin(And, bin(Or, v("a"), v("b")), v("c")));
    }

    #[test]
    fn arithmetic_precedence_and_parentheses() {
        use BinaryOp::*;
        assert_eq!(expr("10 + 5 * 2"), bin(Add, T::Int(10), bin(Mul, T::Int(5), T::Int(2))));
        assert_eq!(expr("(10 + 5) * 2"), bin(Mul, bin(Add, T::Int(10), T::Int(5)), T::Int(2)));
        // Left associativity: 20 - 5 - 3 is (20 - 5) - 3.
        assert_eq!(expr("20 - 5 - 3"), bin(Sub, bin(Sub, T::Int(20), T::Int(5)), T::Int(3)));
        assert_eq!(expr("a / b * c"), bin(Mul, bin(Div, v("a"), v("b")), v("c")));
    }

    #[test]
    fn comparison_precedence() {
        use BinaryOp::*;
        // Arithmetic binds tighter than comparison, which binds tighter than AND.
        assert_eq!(
            expr("a + 1 < b AND c >= 2"),
            bin(And, bin(Less, bin(Add, v("a"), T::Int(1)), v("b")), bin(GreaterEqual, v("c"), T::Int(2)))
        );
        // Relational binds tighter than equality.
        assert_eq!(expr("a < b = c > d"), bin(Equal, bin(Less, v("a"), v("b")), bin(Greater, v("c"), v("d"))));
    }

    #[test]
    fn unary_minus() {
        use BinaryOp::*;
        assert_eq!(expr("-10"), T::Int(-10));
        assert_eq!(expr("-2.75"), T::Real(-2.75));
        assert_eq!(expr("-x"), T::Neg(Box::new(v("x"))));
        assert_eq!(expr("a - -2"), bin(Sub, v("a"), T::Int(-2)));
        assert_eq!(expr("-a * b"), bin(Mul, T::Neg(Box::new(v("a"))), v("b")));
    }

    #[test]
    fn reports_missing_end_if_with_position() {
        let err = parse("PROGRAM p\nIF Start THEN\n  Motor := TRUE;\nEND_PROGRAM").unwrap_err();
        assert_eq!((err.line, err.column), (4, 1));
        assert!(err.message.starts_with("Expected END_IF"), "{}", err.message);
    }

    #[test]
    fn reports_other_syntax_errors() {
        assert!(parse("PROGRAM p Motor = TRUE; END_PROGRAM").unwrap_err().message.contains("':='"));
        assert!(parse("PROGRAM p Motor := TRUE END_PROGRAM").unwrap_err().message.contains("';'"));
        assert!(parse("PROGRAM p VAR x : STRING; END_VAR END_PROGRAM").unwrap_err().message.contains("not supported"));
        assert!(parse("PROGRAM p x := TRUE;").unwrap_err().message.contains("END_PROGRAM"));
        assert!(parse("x := TRUE;").unwrap_err().message.contains("PROGRAM"));
        assert!(parse("PROGRAM p x := 1 + ; END_PROGRAM").unwrap_err().message.contains("an expression"));
        assert_eq!(
            parse("PROGRAM p ELSE END_PROGRAM").unwrap_err().message,
            "Expected END_PROGRAM but found ELSE"
        );
    }

    #[test]
    fn parses_function_calls() {
        use BinaryOp::*;
        let call = |name: &str, args: Vec<T>| T::Call(name.into(), args);
        assert_eq!(expr("ABS(x)"), call("ABS", vec![v("x")]));
        assert_eq!(expr("MAX(a, b, 3)"), call("MAX", vec![v("a"), v("b"), T::Int(3)]));
        assert_eq!(expr("f()"), call("f", vec![]), "the checker, not the parser, rejects bad calls");
        // A call is a primary: it binds tighter than any operator, and nests.
        assert_eq!(expr("ABS(a - b) * 2"), bin(Mul, call("ABS", vec![bin(Sub, v("a"), v("b"))]), T::Int(2)));
        assert_eq!(expr("-SQRT(x)"), T::Neg(Box::new(call("SQRT", vec![v("x")]))));
        assert_eq!(
            expr("MIN(ABS(a), ROUND(b + 0.5))"),
            call("MIN", vec![call("ABS", vec![v("a")]), call("ROUND", vec![bin(Add, v("b"), T::Real(0.5))])])
        );
        let err = parse("PROGRAM p x := MAX(a b); END_PROGRAM").unwrap_err();
        assert!(err.message.contains("Expected ',' or ')'"), "{}", err.message);
        let err = parse("PROGRAM p x := MAX(a,); END_PROGRAM").unwrap_err();
        assert!(err.message.contains("an expression"), "{}", err.message);
    }

    #[test]
    fn deep_nesting_is_an_error_not_a_crash() {
        let too_deep = [
            format!("x := {}TRUE{};", "(".repeat(10_000), ")".repeat(10_000)),
            format!("x := {}1;", "-".repeat(10_000)),
            format!("x := {}1;", "NOT ".repeat(10_000)),
            format!("x := 1{};", " + 1".repeat(10_000)),
            format!("{} END_IF;", "IF a THEN ".repeat(10_000)),
            format!("{} END_WHILE;", "WHILE a DO ".repeat(10_000)),
            format!("{} UNTIL a END_REPEAT;", "REPEAT ".repeat(10_000)),
            format!("{} END_FOR;", "FOR i := 1 TO 2 DO ".repeat(10_000)),
            format!("{} END_CASE;", "CASE i OF 1: ".repeat(10_000)),
            format!("x := {}1{};", "ABS(".repeat(10_000), ")".repeat(10_000)),
        ];
        for body in too_deep {
            let err = parse(&format!("PROGRAM p {body} END_PROGRAM")).unwrap_err();
            assert!(err.message.contains("nested too deeply"), "{}", err.message);
        }
    }
}
