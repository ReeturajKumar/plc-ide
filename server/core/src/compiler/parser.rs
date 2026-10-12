use super::ast::{
    BinaryOp, DataType, CallArg, CaseBranch, CaseLabel, Expr, ExprKind, FunctionBlockDef, IfBranch, Layer, Program, SourceFile,
    Stmt, TypeBody, TypeDecl, UnaryOp, VarDecl, VarType,
};
use super::error::{ErrorKind, StError, StResult};
use super::function_blocks::FbKind;
use super::lexer::{tokenize, Token, TokenKind};
use super::value::Value;

/// Limit on expression/statement depth. Deeply nested input must produce an error, not a
/// stack overflow that kills the IDE: the parser, checker and runtime all recurse over the
/// tree. Hosts run them on threads sized for this limit (`crate::ENGINE_STACK_BYTES`).
pub const MAX_NESTING: usize = 100;

/// Limit on the elements of one variable (arrays, repeated initial values), so a typo like
/// ARRAY[0..1000000000] is an error rather than gigabytes of memory.
pub const MAX_ELEMENTS: i128 = 1_000_000;

/// Parse a project source file: FUNCTION_BLOCKs, TYPEs and at most one PROGRAM, in any order.
pub fn parse_file(source: &str) -> StResult<SourceFile> {
    let mut parser = Parser { tokens: tokenize(source), pos: 0, depth: 0 };
    let mut file = SourceFile::default();
    loop {
        match parser.peek().kind {
            TokenKind::FunctionBlock => file.function_blocks.push(parser.function_block()?),
            TokenKind::TypeBlock => file.types.extend(parser.type_block()?),
            TokenKind::Program if file.program.is_some() => {
                let token = parser.peek().clone();
                return Err(parser.error_at(&token, "A file can contain only one PROGRAM; put the other one in its own file"));
            }
            TokenKind::Program => file.program = Some(parser.program()?),
            TokenKind::Eof if file.program.is_some() || !file.function_blocks.is_empty() || !file.types.is_empty() => {
                return Ok(file)
            }
            _ => return Err(parser.error_expected("PROGRAM, FUNCTION_BLOCK or TYPE")),
        }
    }
}

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
        let decl = self.typed_decl(name, line, column)?;
        self.expect(TokenKind::Semicolon, "';'")?;
        Ok(decl)
    }

    /// After `name :`, the type (any `ARRAY[…] OF`, `POINTER TO`, `REFERENCE TO` around a
    /// type name) and the optional `:= initial value`.
    fn typed_decl(&mut self, name: String, line: usize, column: usize) -> StResult<VarDecl> {
        let mut layers = Vec::new();
        loop {
            match self.peek().kind {
                TokenKind::Array => {
                    self.advance();
                    self.expect(TokenKind::LBracket, "'[' after ARRAY")?;
                    loop {
                        let lower = self.bound()?;
                        self.expect(TokenKind::Dot, "'..' between the array bounds, e.g. ARRAY[1..10]")?;
                        self.expect(TokenKind::Dot, "'..' between the array bounds, e.g. ARRAY[1..10]")?;
                        let upper = self.bound()?;
                        layers.push(Layer::Array(lower, upper));
                        if !self.check(&TokenKind::Comma) {
                            break;
                        }
                        self.advance();
                    }
                    self.expect(TokenKind::RBracket, "',' or ']'")?;
                    self.expect(TokenKind::Of, "OF")?;
                }
                TokenKind::Pointer | TokenKind::Reference => {
                    let reference = self.advance().kind == TokenKind::Reference;
                    self.expect(TokenKind::To, "TO")?;
                    layers.push(if reference { Layer::Reference } else { Layer::Pointer });
                }
                _ => break,
            }
        }
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
        Ok(VarDecl { name, var_type, type_name, layers, init, line, column })
    }

    /// An array bound: an integer literal, optionally negative.
    fn bound(&mut self) -> StResult<i128> {
        let negative = self.check(&TokenKind::Minus);
        if negative {
            self.advance();
        }
        match self.peek().kind {
            TokenKind::IntLiteral(n) => {
                self.advance();
                Ok(if negative { -n } else { n })
            }
            _ => Err(self.error_expected("an integer array bound")),
        }
    }

    /// `TYPE Name : STRUCT … END_STRUCT; Other : ARRAY[1..4] OF INT; END_TYPE`
    fn type_block(&mut self) -> StResult<Vec<TypeDecl>> {
        self.advance(); // TYPE
        let mut types = Vec::new();
        while !self.check(&TokenKind::EndType) {
            let (name, line, column) = self.identifier("a type name or END_TYPE")?;
            self.expect(TokenKind::Colon, "':'")?;
            let body = if self.check(&TokenKind::Struct) {
                self.advance();
                let mut fields = Vec::new();
                while !self.check(&TokenKind::EndStruct) {
                    fields.push(self.var_decl()?);
                }
                self.advance(); // END_STRUCT
                self.optional_semicolon();
                TypeBody::Struct(fields)
            } else {
                let alias = self.typed_decl(name.clone(), line, column)?;
                self.expect(TokenKind::Semicolon, "';'")?;
                TypeBody::Alias(alias)
            };
            types.push(TypeDecl { name, body, line, column });
        }
        self.advance(); // END_TYPE
        self.optional_semicolon();
        Ok(types)
    }

    fn var_type(&mut self) -> StResult<VarType> {
        let token = self.peek().clone();
        let var_type = match &token.kind {
            TokenKind::Type(data_type) if data_type.is_text() => {
                self.advance();
                self.string_length()?;
                return Ok(VarType::Elementary(*data_type));
            }
            TokenKind::Type(data_type) => VarType::Elementary(*data_type),
            TokenKind::Identifier(name) => match FbKind::from_name(name) {
                Some(kind) => VarType::FunctionBlock(kind),
                // Maybe a FUNCTION_BLOCK or TYPE of the project; the checker resolves it.
                None => VarType::UNRESOLVED,
            },
            _ => return Err(self.error_expected("a data type")),
        };
        self.advance();
        Ok(var_type)
    }

    /// The optional maximum length after STRING/WSTRING: `STRING[20]` or `STRING(20)`.
    // ponytail: accepted but not enforced, strings grow as needed; truncate on assignment
    // if a program relies on the limit.
    fn string_length(&mut self) -> StResult<()> {
        let close = match self.peek().kind {
            TokenKind::LBracket => TokenKind::RBracket,
            TokenKind::LParen => TokenKind::RParen,
            _ => return Ok(()),
        };
        self.advance();
        match self.peek().kind {
            TokenKind::IntLiteral(n) if n > 0 => self.advance(),
            _ => return Err(self.error_expected("a string length, e.g. STRING[80]")),
        };
        let closer = close.describe();
        self.expect(close, &closer)?;
        Ok(())
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
                Some(TokenKind::RefAssign) => {
                    let (target, line, column) = self.identifier("a reference")?;
                    self.advance(); // REF=
                    let value = self.expression()?;
                    self.expect(TokenKind::Semicolon, "';'")?;
                    Ok(Stmt::RefAssign { target, value, line, column })
                }
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

    /// `x := …;`, `Instance.Member := …;`, `a[i].x := …;`
    fn assignment(&mut self) -> StResult<Stmt> {
        let (name, line, column) = self.identifier("a variable name")?;
        let target = self.postfix(Expr { kind: ExprKind::Variable(name), line, column })?;
        self.expect(TokenKind::Assign, "':='")?;
        let value = self.expression()?;
        self.expect(TokenKind::Semicolon, "';'")?;
        Ok(match target.kind {
            ExprKind::Variable(target) => Stmt::Assign { target, value, line, column },
            ExprKind::Member { instance, member } => Stmt::AssignMember { instance, member, value, line, column },
            _ => Stmt::AssignPath { target, value, line, column },
        })
    }

    /// `[i, j]`, `^` and `.name` after a variable, any number of times. `Name.Member` stays a
    /// `Member` (a function block's member or a structure's field; the checker knows which).
    fn postfix(&mut self, mut expr: Expr) -> StResult<Expr> {
        let mut steps = 0;
        loop {
            if self.check(&TokenKind::LBracket) {
                self.advance();
                loop {
                    self.enter()?;
                    steps += 1;
                    let index = self.expression()?;
                    let (line, column) = (expr.line, expr.column);
                    let kind = ExprKind::Index { base: Box::new(expr), index: Box::new(index), lower: 0, upper: 0 };
                    expr = Expr { kind, line, column };
                    if !self.check(&TokenKind::Comma) {
                        break;
                    }
                    self.advance();
                }
                self.expect(TokenKind::RBracket, "',' or ']'")?;
            } else if self.check(&TokenKind::Caret) {
                self.advance();
                self.enter()?;
                steps += 1;
                let (line, column) = (expr.line, expr.column);
                expr = Expr { kind: ExprKind::Deref(Box::new(expr)), line, column };
            } else if self.check(&TokenKind::Dot) {
                self.advance();
                self.enter()?;
                steps += 1;
                let (field, _, _) = self.identifier("a member name")?;
                let (line, column) = (expr.line, expr.column);
                let kind = match expr.kind {
                    ExprKind::Variable(instance) => ExprKind::Member { instance, member: field },
                    _ => ExprKind::Field { base: Box::new(expr), field, index: usize::MAX },
                };
                expr = Expr { kind, line, column };
            } else {
                break;
            }
        }
        self.depth -= steps;
        Ok(expr)
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
            TokenKind::StringLiteral(data_type, ref text) => ExprKind::Const(Value::text(text.clone(), data_type)),
            TokenKind::DateLiteral(date_type, ms) => ExprKind::Const(Value::integer(ms.into(), date_type).map_err(|m| self.error_at(&token, m))?),
            TokenKind::Type(data_type) if matches!(self.tokens.get(self.pos + 1).map(|t| &t.kind), Some(TokenKind::Hash)) => {
                self.advance(); // type
                self.advance(); // #
                let value = self.typed_literal(data_type, &token)?;
                return Ok(Expr { kind: ExprKind::Const(value), line: token.line, column: token.column });
            }
            TokenKind::Identifier(name) => {
                self.advance();
                let at = |kind| Expr { kind, line: token.line, column: token.column };
                if self.check(&TokenKind::LParen) {
                    // Which names are functions is the checker's business; any `name(…)`
                    // in an expression is a call.
                    return Ok(at(ExprKind::Call { name, args: self.call_arguments()? }));
                }
                return self.postfix(at(ExprKind::Variable(name)));
            }
            // `[1, 2, 3(0)]`: an array's initial value.
            TokenKind::LBracket => {
                self.advance();
                let mut items = Vec::new();
                while !self.check(&TokenKind::RBracket) {
                    let repeat = match (&self.peek().kind, self.tokens.get(self.pos + 1).map(|t| &t.kind)) {
                        (TokenKind::IntLiteral(n), Some(TokenKind::LParen)) => Some(*n),
                        _ => None,
                    };
                    match repeat {
                        Some(n) => {
                            let count = self.advance();
                            self.advance(); // (
                            let item = self.expression()?;
                            self.expect(TokenKind::RParen, "')'")?;
                            if !(0..=MAX_ELEMENTS).contains(&n) {
                                return Err(self.error_at(&count, format!("Repeat count {n} is too large")));
                            }
                            items.extend(std::iter::repeat_n(item, n as usize));
                        }
                        None => items.push(self.expression()?),
                    }
                    if !self.check(&TokenKind::Comma) {
                        break;
                    }
                    self.advance();
                }
                self.expect(TokenKind::RBracket, "',' or ']'")?;
                return Ok(Expr { kind: ExprKind::ArrayInit(items), line: token.line, column: token.column });
            }
            // `(x := 1.0, y := 2.0)`: a structure's initial value.
            TokenKind::LParen
                if matches!(self.tokens.get(self.pos + 1).map(|t| &t.kind), Some(TokenKind::Identifier(_)))
                    && matches!(self.tokens.get(self.pos + 2).map(|t| &t.kind), Some(TokenKind::Assign)) =>
            {
                self.advance();
                let mut fields = Vec::new();
                loop {
                    let (name, _, _) = self.identifier("a member name")?;
                    self.expect(TokenKind::Assign, "':='")?;
                    fields.push((name, self.expression()?));
                    if !self.check(&TokenKind::Comma) {
                        break;
                    }
                    self.advance();
                }
                self.expect(TokenKind::RParen, "',' or ')'")?;
                return Ok(Expr { kind: ExprKind::StructInit(fields), line: token.line, column: token.column });
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

    /// The value of `TYPE#value` (after the `#`), e.g. `UINT#7`, `WORD#16#FF`, `LREAL#-1.5`.
    fn typed_literal(&mut self, data_type: DataType, start: &Token) -> StResult<Value> {
        let negative = self.check(&TokenKind::Minus);
        if negative {
            self.advance();
        }
        let sign: i128 = if negative { -1 } else { 1 };
        let value = match self.peek().kind {
            TokenKind::IntLiteral(n) if data_type.is_real() => Ok(Value::real((sign * n) as f64, data_type)),
            TokenKind::IntLiteral(n) if data_type.is_integer() || data_type.is_bit_string() => {
                Value::integer(sign * n, data_type).map_err(|_| format!("Value {} is out of range for {}", sign * n, data_type.name()))
            }
            TokenKind::RealLiteral(x) if data_type.is_real() => Ok(Value::real(if negative { -x } else { x }, data_type)),
            // `CHAR#'A'`, `WCHAR#"Ω"`: exactly one character.
            TokenKind::StringLiteral(_, ref text) if matches!(data_type, DataType::Char | DataType::WChar) && !negative => {
                let mut chars = text.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => Value::integer(u32::from(c).into(), data_type)
                        .map_err(|_| format!("'{c}' is not a {} character", data_type.name())),
                    _ => Err(format!("A {} literal holds exactly one character", data_type.name())),
                }
            }
            _ => return Err(self.error_expected(&format!("a {} value after '{}#'", data_type.name(), data_type.name()))),
        };
        self.advance();
        value.map_err(|message| StError::new(ErrorKind::TypeMismatch, start.line, start.column, message))
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

