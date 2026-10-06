use super::function_blocks::FbKind;
use super::value::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataType {
    Bool,
    Int,
    DInt,
    Real,
    /// Duration in milliseconds.
    Time,
}

impl DataType {
    pub fn name(self) -> &'static str {
        match self {
            DataType::Bool => "BOOL",
            DataType::Int => "INT",
            DataType::DInt => "DINT",
            DataType::Real => "REAL",
            DataType::Time => "TIME",
        }
    }

    /// Types that take part in `* /` and accept numeric literals.
    pub fn is_numeric(self) -> bool {
        matches!(self, DataType::Int | DataType::DInt | DataType::Real)
    }

    pub fn is_integer(self) -> bool {
        matches!(self, DataType::Int | DataType::DInt)
    }

    /// Whether a value of `self` may be used where `to` is expected. Besides identity, only
    /// the exact IEC 61131-3 widenings INT → DINT and INT → REAL are implicit. Nothing
    /// narrows, and DINT → REAL is excluded because it can lose precision.
    pub fn widens_to(self, to: DataType) -> bool {
        self == to || matches!((self, to), (DataType::Int, DataType::DInt) | (DataType::Int, DataType::Real))
    }
}

/// What a variable holds: a plain value, or an instance of a function block with its
/// own state that persists across scans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarType {
    Elementary(DataType),
    /// A standard function block (TON, CTU, …).
    FunctionBlock(FbKind),
    /// A FUNCTION_BLOCK defined in the project: its index in the project's registry.
    /// The parser can't know the index; it writes `UNRESOLVED` and the checker resolves
    /// `VarDecl::type_name`.
    UserFb(usize),
}

impl VarType {
    pub const UNRESOLVED: VarType = VarType::UserFb(usize::MAX);
}

/// A source file: any number of FUNCTION_BLOCKs and at most one PROGRAM.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SourceFile {
    pub program: Option<Program>,
    pub function_blocks: Vec<FunctionBlockDef>,
}

/// `FUNCTION_BLOCK name VAR_INPUT … VAR_OUTPUT … VAR … <body> END_FUNCTION_BLOCK`.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionBlockDef {
    pub name: String,
    pub inputs: Vec<VarDecl>,
    pub outputs: Vec<VarDecl>,
    /// Internal variables: private to each instance.
    pub vars: Vec<VarDecl>,
    pub body: Vec<Stmt>,
    pub line: usize,
    pub column: usize,
    /// The project file it is defined in (set by the loader; empty in tests).
    pub file: String,
}

impl FunctionBlockDef {
    /// Inputs, outputs, then internal variables: the order of an instance's members.
    pub fn members(&self) -> impl Iterator<Item = &VarDecl> {
        self.inputs.iter().chain(&self.outputs).chain(&self.vars)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub name: String,
    pub vars: Vec<VarDecl>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VarDecl {
    pub name: String,
    pub var_type: VarType,
    /// The type as written, e.g. "MotorControl" (resolves `VarType::UNRESOLVED`).
    pub type_name: String,
    pub init: Option<Expr>,
    pub line: usize,
    pub column: usize,
}

/// Statements that carry `line`/`column` point at their keyword or target, for errors.
#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Assign { target: String, value: Expr, line: usize, column: usize },
    /// `Instance.Member := value;`. Parsed so the checker can explain why it's not allowed.
    AssignMember { instance: String, member: String, value: Expr, line: usize, column: usize },
    /// `IF … ELSIF … ELSE … END_IF`: the first branch whose condition holds runs.
    If { branches: Vec<IfBranch>, else_branch: Vec<Stmt>, line: usize, column: usize },
    /// `FOR var := start TO end [BY step] DO … END_FOR`
    For { var: String, start: Expr, end: Expr, step: Option<Expr>, body: Vec<Stmt>, line: usize, column: usize },
    While { condition: Expr, body: Vec<Stmt>, line: usize, column: usize },
    /// `REPEAT … UNTIL condition END_REPEAT`: the body runs at least once.
    Repeat { body: Vec<Stmt>, condition: Expr, line: usize, column: usize },
    Case { selector: Expr, branches: Vec<CaseBranch>, else_branch: Vec<Stmt>, line: usize, column: usize },
    /// `Instance(Input := expr, …);` — runs one function block instance.
    Call { instance: String, args: Vec<CallArg>, line: usize, column: usize },
}

impl Stmt {
    /// Where the statement starts (its keyword or target), used by errors and the debugger.
    pub fn location(&self) -> (usize, usize) {
        match self {
            Stmt::Assign { line, column, .. }
            | Stmt::AssignMember { line, column, .. }
            | Stmt::If { line, column, .. }
            | Stmt::For { line, column, .. }
            | Stmt::While { line, column, .. }
            | Stmt::Repeat { line, column, .. }
            | Stmt::Case { line, column, .. }
            | Stmt::Call { line, column, .. } => (*line, *column),
        }
    }

    fn children(&self) -> Vec<&Stmt> {
        match self {
            Stmt::If { branches, else_branch, .. } => branches.iter().flat_map(|b| &b.body).chain(else_branch).collect(),
            Stmt::For { body, .. } | Stmt::While { body, .. } | Stmt::Repeat { body, .. } => body.iter().collect(),
            Stmt::Case { branches, else_branch, .. } => branches.iter().flat_map(|b| &b.body).chain(else_branch).collect(),
            Stmt::Assign { .. } | Stmt::AssignMember { .. } | Stmt::Call { .. } => Vec::new(),
        }
    }
}

/// The lines where a statement starts, sorted: where a breakpoint can stop execution.
pub fn statement_lines(body: &[Stmt]) -> Vec<usize> {
    fn walk(body: &[&Stmt], lines: &mut Vec<usize>) {
        for stmt in body {
            lines.push(stmt.location().0);
            walk(&stmt.children(), lines);
        }
    }
    let mut lines = Vec::new();
    walk(&body.iter().collect::<Vec<_>>(), &mut lines);
    lines.sort_unstable();
    lines.dedup();
    lines
}

#[derive(Debug, Clone, PartialEq)]
pub struct IfBranch {
    pub condition: Expr,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CaseBranch {
    pub labels: Vec<CaseLabel>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CaseLabel {
    pub value: i64,
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CallArg {
    pub name: String,
    pub value: Expr,
    pub line: usize,
    pub column: usize,
}

/// An expression and the source position used in its error messages
/// (the operator for unary/binary expressions).
#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    /// A typed constant: TRUE/FALSE and TIME literals from the parser, or a numeric
    /// literal after type checking.
    Const(Value),
    /// Numeric literals as written. Their IEC type depends on context, so the checker
    /// replaces them with `Const` before the program can run.
    IntLiteral(i64),
    RealLiteral(f64),
    Variable(String),
    /// `Instance.Member`, e.g. `RunTimer.Q`.
    Member { instance: String, member: String },
    /// A standard function call inside an expression, e.g. `ABS(x)`, `MAX(a, b)`.
    Call { name: String, args: Vec<Expr> },
    Unary { op: UnaryOp, operand: Box<Expr> },
    Binary { op: BinaryOp, left: Box<Expr>, right: Box<Expr> },
    /// An implicit widening (INT → DINT or INT → REAL), inserted by the checker so the
    /// conversion is explicit in the tree rather than hidden in the runtime.
    Convert { to: DataType, operand: Box<Expr> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Not,
    Neg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    And,
    Or,
    Xor,
    Equal,
    NotEqual,
    Less,
    Greater,
    LessEqual,
    GreaterEqual,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

impl BinaryOp {
    pub fn symbol(self) -> &'static str {
        match self {
            BinaryOp::And => "AND",
            BinaryOp::Or => "OR",
            BinaryOp::Xor => "XOR",
            BinaryOp::Equal => "=",
            BinaryOp::NotEqual => "<>",
            BinaryOp::Less => "<",
            BinaryOp::Greater => ">",
            BinaryOp::LessEqual => "<=",
            BinaryOp::GreaterEqual => ">=",
            BinaryOp::Add => "+",
            BinaryOp::Sub => "-",
            BinaryOp::Mul => "*",
            BinaryOp::Div => "/",
            BinaryOp::Mod => "MOD",
        }
    }
}
