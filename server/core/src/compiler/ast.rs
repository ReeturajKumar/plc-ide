use super::function_blocks::FbKind;
use super::value::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum DataType {
    Bool,
    SInt,
    Int,
    DInt,
    LInt,
    USInt,
    UInt,
    UDInt,
    ULInt,
    Real,
    LReal,
    Byte,
    Word,
    DWord,
    LWord,
    /// Duration in milliseconds.
    Time,
    /// A day: milliseconds since 1970-01-01 to its midnight.
    Date,
    /// TIME_OF_DAY: milliseconds since midnight.
    Tod,
    /// DATE_AND_TIME: milliseconds since 1970-01-01 00:00.
    Dt,
    Char,
    WChar,
    String,
    WString,
    /// Composite values (`Value::Array`, `Value::Struct`); their exact type is a
    /// `VarType::Array` / `VarType::Struct`. Not in `ALL`: no plain variable has them.
    Array,
    Struct,
    /// A `Value::Pointer`: a POINTER TO or REFERENCE TO variable's own value.
    Pointer,
}

impl DataType {
    pub const ALL: [DataType; 23] = [
        DataType::Bool,
        DataType::SInt,
        DataType::Int,
        DataType::DInt,
        DataType::LInt,
        DataType::USInt,
        DataType::UInt,
        DataType::UDInt,
        DataType::ULInt,
        DataType::Real,
        DataType::LReal,
        DataType::Byte,
        DataType::Word,
        DataType::DWord,
        DataType::LWord,
        DataType::Time,
        DataType::Date,
        DataType::Tod,
        DataType::Dt,
        DataType::Char,
        DataType::WChar,
        DataType::String,
        DataType::WString,
    ];

    pub fn name(self) -> &'static str {
        match self {
            DataType::Bool => "BOOL",
            DataType::SInt => "SINT",
            DataType::Int => "INT",
            DataType::DInt => "DINT",
            DataType::LInt => "LINT",
            DataType::USInt => "USINT",
            DataType::UInt => "UINT",
            DataType::UDInt => "UDINT",
            DataType::ULInt => "ULINT",
            DataType::Real => "REAL",
            DataType::LReal => "LREAL",
            DataType::Byte => "BYTE",
            DataType::Word => "WORD",
            DataType::DWord => "DWORD",
            DataType::LWord => "LWORD",
            DataType::Time => "TIME",
            DataType::Date => "DATE",
            DataType::Tod => "TOD",
            DataType::Dt => "DT",
            DataType::Char => "CHAR",
            DataType::WChar => "WCHAR",
            DataType::String => "STRING",
            DataType::WString => "WSTRING",
            DataType::Array => "ARRAY",
            DataType::Struct => "STRUCT",
            DataType::Pointer => "POINTER",
        }
    }

    /// The type a (case-insensitive) name like "udint" or "time_of_day" denotes.
    pub fn from_name(name: &str) -> Option<DataType> {
        match name.to_ascii_uppercase().as_str() {
            "TIME_OF_DAY" => Some(DataType::Tod),
            "DATE_AND_TIME" => Some(DataType::Dt),
            upper => DataType::ALL.into_iter().find(|t| t.name() == upper),
        }
    }

    /// STRING and WSTRING.
    pub fn is_text(self) -> bool {
        matches!(self, DataType::String | DataType::WString)
    }

    /// TIME and the date types: all held as milliseconds, ordered, never implicit.
    pub fn is_time_like(self) -> bool {
        matches!(self, DataType::Time | DataType::Date | DataType::Tod | DataType::Dt)
    }

    /// Types that take part in `+ - * /`.
    pub fn is_numeric(self) -> bool {
        self.is_integer() || self.is_real()
    }

    /// SINT … ULINT (not the bit strings).
    pub fn is_integer(self) -> bool {
        use DataType::*;
        matches!(self, SInt | Int | DInt | LInt | USInt | UInt | UDInt | ULInt)
    }

    pub fn is_unsigned(self) -> bool {
        matches!(self, DataType::USInt | DataType::UInt | DataType::UDInt | DataType::ULInt)
    }

    pub fn is_real(self) -> bool {
        matches!(self, DataType::Real | DataType::LReal)
    }

    /// BYTE, WORD, DWORD, LWORD: bit patterns, with bitwise operators but no arithmetic.
    pub fn is_bit_string(self) -> bool {
        matches!(self, DataType::Byte | DataType::Word | DataType::DWord | DataType::LWord)
    }

    /// Size in bits of the integer, bit string and TIME types.
    pub fn bits(self) -> Option<u32> {
        use DataType::*;
        match self {
            SInt | USInt | Byte => Some(8),
            Int | UInt | Word => Some(16),
            DInt | UDInt | DWord => Some(32),
            LInt | ULInt | LWord | Time | Date | Tod | Dt => Some(64),
            Bool | Real | LReal | Char | WChar | String | WString | Array | Struct | Pointer => None,
        }
    }

    /// The smallest and largest value of the integer, bit string and TIME types.
    pub fn range(self) -> Option<(i128, i128)> {
        let bits = self.bits()?;
        Some(if self.is_unsigned() || self.is_bit_string() {
            (0, (1i128 << bits) - 1)
        } else {
            (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1)
        })
    }

    /// Whether a value of `self` may be used where `to` is expected: identity, or one of
    /// the IEC 61131-3 implicit conversions, which are all exact. Nothing narrows, and
    /// e.g. DINT → REAL or LINT → LREAL is excluded because it can lose precision.
    pub fn widens_to(self, to: DataType) -> bool {
        use DataType::*;
        let targets: &[DataType] = match self {
            SInt => &[Int, DInt, LInt, Real, LReal],
            Int => &[DInt, LInt, Real, LReal],
            DInt => &[LInt, LReal],
            USInt => &[UInt, UDInt, ULInt, Int, DInt, LInt, Real, LReal],
            UInt => &[UDInt, ULInt, DInt, LInt, Real, LReal],
            UDInt => &[ULInt, LInt, LReal],
            Real => &[LReal],
            Byte => &[Word, DWord, LWord],
            Word => &[DWord, LWord],
            DWord => &[LWord],
            _ => &[],
        };
        self == to || targets.contains(&to)
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
    /// An array or a structure: its index in the project's type table (`TypeDef`), which
    /// the checker builds.
    Array(usize),
    Struct(usize),
    /// `POINTER TO T` and `REFERENCE TO T`, also in the type table. A reference reads and
    /// writes its target wherever it is used.
    Pointer(usize),
    Reference(usize),
}

impl VarType {
    pub const UNRESOLVED: VarType = VarType::UserFb(usize::MAX);
}

/// A source file: any number of FUNCTION_BLOCKs and TYPE declarations, and at most one
/// PROGRAM.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SourceFile {
    pub program: Option<Program>,
    pub function_blocks: Vec<FunctionBlockDef>,
    pub types: Vec<TypeDecl>,
}

/// One declaration of a `TYPE … END_TYPE` block: `Point : STRUCT x : REAL; … END_STRUCT;`
/// or a name for another type, `Buffer : ARRAY[1..8] OF INT;`.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeDecl {
    pub name: String,
    pub body: TypeBody,
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeBody {
    Struct(Vec<VarDecl>),
    /// The aliased type, as a declaration (its name is the alias's).
    Alias(VarDecl),
}

/// An array or structure type of the project, resolved by the checker.
#[derive(Debug, Clone, PartialEq)]
pub enum TypeDef {
    /// One dimension; `ARRAY[1..3, 1..4] OF T` is an array of arrays.
    Array { lower: i128, upper: i128, of: VarType },
    /// Members with their resolved types and checked initial values.
    Struct { name: String, fields: Vec<VarDecl> },
    Pointer { to: VarType },
    Reference { to: VarType },
}

/// One step of a declared type, outermost first: `ARRAY[1..3] OF POINTER TO INT` is
/// `[Array(1, 3), Pointer]` around INT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Array(i128, i128),
    Pointer,
    Reference,
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
    /// The type as written, e.g. "MotorControl" (resolves `VarType::UNRESOLVED`); for an
    /// array, its element type.
    pub type_name: String,
    /// `ARRAY[…] OF`, `POINTER TO`, `REFERENCE TO` around the type, outermost first; empty
    /// for a plain type. The checker folds them into `var_type`.
    pub layers: Vec<Layer>,
    pub init: Option<Expr>,
    pub line: usize,
    pub column: usize,
}

/// Statements that carry `line`/`column` point at their keyword or target, for errors.
#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Assign { target: String, value: Expr, line: usize, column: usize },
    /// `Instance.Member := value;`: a structure's member, or (not allowed, but parsed so
    /// the checker can explain why) a function block's.
    AssignMember { instance: String, member: String, value: Expr, line: usize, column: usize },
    /// `a[i] := value;`, `p.pos.x := value;`, `p^ := value;`: `target` is an `Index`/
    /// `Field`/`Deref` chain on a variable.
    AssignPath { target: Expr, value: Expr, line: usize, column: usize },
    /// `r REF= x;`: bind the reference `r` to `x`. The checker lowers it to an `Assign`.
    RefAssign { target: String, value: Expr, line: usize, column: usize },
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
            | Stmt::AssignPath { line, column, .. }
            | Stmt::RefAssign { line, column, .. }
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
            Stmt::Assign { .. } | Stmt::AssignMember { .. } | Stmt::AssignPath { .. } | Stmt::RefAssign { .. } | Stmt::Call { .. } => Vec::new(),
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
    pub value: i128,
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
    IntLiteral(i128),
    RealLiteral(f64),
    Variable(String),
    /// `Instance.Member`, e.g. `RunTimer.Q`.
    Member { instance: String, member: String },
    /// A standard function call inside an expression, e.g. `ABS(x)`, `MAX(a, b)`.
    Call { name: String, args: Vec<Expr> },
    Unary { op: UnaryOp, operand: Box<Expr> },
    Binary { op: BinaryOp, left: Box<Expr>, right: Box<Expr> },
    /// `base[index]`. The checker fills in the bounds, which the runtime checks.
    Index { base: Box<Expr>, index: Box<Expr>, lower: i128, upper: i128 },
    /// `base.field` of a structure. The checker fills in the field's position.
    Field { base: Box<Expr>, field: String, index: usize },
    /// `p^`: what a pointer (or, after checking, a reference) points to.
    Deref(Box<Expr>),
    /// `ADR(x)` after checking: a pointer to the variable or element `x`.
    Address(Box<Expr>),
    /// `[1, 2, 3(0)]`, an array's initial value (repetitions expanded).
    ArrayInit(Vec<Expr>),
    /// `(x := 1.0, y := 2.0)`, a structure's initial value.
    StructInit(Vec<(String, Expr)>),
    /// A conversion: an implicit widening inserted by the checker (so it is explicit in
    /// the tree rather than hidden in the runtime), or a call such as `INT_TO_REAL(x)`.
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
