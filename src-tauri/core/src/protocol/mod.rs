//! The MyPLC UI ↔ runtime protocol: every message either side may send, independent of
//! the transport (Tauri, WebSocket, TCP, pipes, stdin/stdout…), of the UI and of any host.
//! Messages are plain data that serialize to JSON:
//!
//! ```text
//! UI → runtime    request    {"id":"7","command":"SET_INPUT","payload":{"address":"DI0","value":true}}
//!                            {"id":"8","command":"STOP"}                       (no payload)
//! runtime → UI    response   {"type":"RESPONSE","id":"7","success":true,"data":{"state":{…}}}
//!                            {"type":"RESPONSE","id":"8","success":false,
//!                             "error":{"code":"REJECTED","message":"The program is not running."}}
//!                 event      {"type":"STATE_UPDATE","state":{…}}
//! ```
//!
//! Every request carries an `id`; its response repeats it, so several requests can be in
//! flight. Every runtime → UI message carries a `type`: only a `RESPONSE` answers a
//! request; events (`STATE_UPDATE`) arrive on their own.
//!
//! The state is the engine's factual state (what a variable is, whether a program writes
//! it, which address it is mapped to); how to show it is the UI's business.
//!
//! The Rust types here are the source of truth; `src/types/protocol.ts` mirrors them for
//! the UI and must be updated with them.

use serde::{Deserialize, Serialize};

use crate::compiler::ast::DataType;
use crate::compiler::error::StError;
use crate::compiler::project::{CompileReport, ProgramSource};
use crate::compiler::value::Value;
use crate::io::{InputValue, IoPoint, Mapping};
use crate::runtime::{self, EngineState};

/// Chosen by the UI, unique among its requests in flight.
pub type RequestId = String;

// ---------------------------------------------------------------- UI → runtime

/// A command for the runtime, with the id its response will carry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub id: RequestId,
    #[serde(flatten)]
    pub command: Command,
}

/// What the UI can ask of the runtime: user intents, one per existing capability.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", content = "payload", rename_all = "SCREAMING_SNAKE_CASE", rename_all_fields = "camelCase")]
pub enum Command {
    /// Compile the sources and load the project: every variable and function block at its
    /// initial value, the PLC STOPPED. Replaces any loaded project. A project that doesn't
    /// compile is refused with `COMPILATION_ERROR`, and then nothing is loaded.
    LoadProject {
        /// Every source file, in execution order (files with only FUNCTION_BLOCKs included).
        programs: Vec<ProgramSource>,
        #[serde(default)]
        mappings: Vec<Mapping>,
        /// PLC scan interval; the runtime's default when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scan_time_ms: Option<u64>,
    },
    /// Start scanning the loaded project from its initial state: RUNNING, or PAUSED before
    /// the first scan when `paused` (ready for STEP). The payload is optional.
    Run(Option<RunOptions>),
    /// Stop scanning: variables back to their initial values, outputs to their safe state.
    Stop,
    Pause,
    Resume,
    /// While PAUSED: finish the scan the debugger stopped, or run one complete scan.
    StepScan,
    /// While PAUSED: run the next statement and stop before the one after.
    StepStatement,
    /// Set a simulated input: a digital one ("DI0") to true/false, an analog one ("AI0")
    /// to a number. Allowed in any state; the next scan reads it.
    SetInput { address: String, value: InputValue },
    /// Set a BOOL variable that `program` declares.
    SetVariable { program: String, name: String, value: bool },
    /// Replace every breakpoint.
    SetBreakpoints { breakpoints: Vec<Breakpoint> },
    /// Re-map the loaded project's I/O from its next scan; invalid mappings are refused
    /// with `IO_MAPPING_ERROR` (the current ones stay).
    ApplyIoMappings { mappings: Vec<Mapping> },
    GetState,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RunOptions {
    pub paused: bool,
}

/// A source line where execution stops. `file` is the source file's name in the project
/// (the same name `Location::file` reports).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Breakpoint {
    pub file: String,
    pub line: usize,
}

// ---------------------------------------------------------------- runtime → UI

/// Anything the runtime sends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RuntimeMessage {
    /// The answer to one request.
    Response(Response),
    /// The state changed (e.g. a scan completed). Not an answer to any request.
    StateUpdate { state: RuntimeState },
}

/// The answer to the request with the same `id`: the state after the command, or why it
/// was refused.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(into = "WireResponse", try_from = "WireResponse")]
pub struct Response {
    /// The request's id; None only when the request was too malformed to read one.
    pub id: Option<RequestId>,
    pub result: Result<ResponseData, ProtocolError>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResponseData {
    pub state: RuntimeState,
}

impl Response {
    pub fn ok(id: RequestId, state: RuntimeState) -> Self {
        Self { id: Some(id), result: Ok(ResponseData { state }) }
    }

    pub fn error(id: Option<RequestId>, error: ProtocolError) -> Self {
        Self { id, result: Err(error) }
    }
}

/// `success` with exactly one of `data` / `error`, as it goes on the wire.
#[derive(Serialize, Deserialize)]
struct WireResponse {
    id: Option<RequestId>,
    success: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    data: Option<ResponseData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<ProtocolError>,
}

impl From<Response> for WireResponse {
    fn from(r: Response) -> Self {
        match r.result {
            Ok(data) => WireResponse { id: r.id, success: true, data: Some(data), error: None },
            Err(error) => WireResponse { id: r.id, success: false, data: None, error: Some(error) },
        }
    }
}

impl TryFrom<WireResponse> for Response {
    type Error = String;

    fn try_from(w: WireResponse) -> Result<Self, String> {
        let result = match (w.success, w.data, w.error) {
            (true, Some(data), None) => Ok(data),
            (false, None, Some(error)) => Err(error),
            _ => return Err("a response has `data` when it succeeded and `error` when it didn't".into()),
        };
        Ok(Response { id: w.id, result })
    }
}

/// Why a request was refused.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProtocolError {
    #[serde(flatten)]
    pub code: ErrorCode,
    /// One sentence for the user.
    pub message: String,
}

/// The error category, with the details each one carries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "code", content = "details", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    /// The project doesn't compile: every file's errors and the mapping errors. Nothing runs.
    CompilationError(CompileReport),
    /// A scan failed: the PLC stopped.
    RuntimeError(ScanError),
    /// The I/O mappings are invalid.
    IoMappingError(Vec<StError>),
    /// Not possible now (STEP while running), or not valid (an unknown variable, an
    /// output address, a malformed request).
    Rejected,
    /// The runtime can't be reached or can't serve requests.
    Unavailable,
}

impl ProtocolError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }

    /// A request that couldn't be decoded (`reason` from the decoder).
    pub fn invalid_request(reason: impl std::fmt::Display) -> Self {
        Self::new(ErrorCode::Rejected, format!("Invalid request: {reason}"))
    }
}

// ---------------------------------------------------------------- state

/// The PLC lifecycle: STOPPED (with or without a loaded project; a scan failure also
/// stops it, see `RuntimeState::error`), RUNNING (scanning), PAUSED (not scanning, maybe
/// mid-scan at a debugger stop). Loading and stopping complete within their request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum RuntimeStatus {
    Stopped,
    Running,
    Paused,
}

/// Everything the runtime reports about the PLC.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeState {
    pub status: RuntimeStatus,
    /// Loaded programs, in execution order (empty without a project).
    pub programs: Vec<String>,
    /// Completed scans since RUN.
    pub cycle_count: u64,
    pub scan_time_ms: u64,
    pub variables: Vec<Variable>,
    pub function_blocks: Vec<FunctionBlock>,
    /// Every I/O point: inputs as last set, outputs as the last scan wrote them.
    pub io: Vec<IoPoint>,
    pub debugger: DebuggerState,
    /// The scan failure that stopped the PLC, until the next RUN.
    pub error: Option<ScanError>,
}

/// A program variable (function block instances are in `function_blocks`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", from = "WireVariable")]
pub struct Variable {
    pub program: String,
    pub name: String,
    pub data_type: DataType,
    pub value: Value,
    /// Some program assigns it; otherwise its value only changes from outside.
    pub written_by_program: bool,
    /// The I/O address it is mapped to.
    pub io_address: Option<String>,
}

/// A function block instance and the current value of each member.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionBlock {
    pub program: String,
    pub instance: String,
    /// TON, CTU, … or the project FUNCTION_BLOCK's name.
    pub type_name: String,
    /// A standard block, as opposed to a project FUNCTION_BLOCK.
    pub standard: bool,
    pub inputs: Vec<Member>,
    pub outputs: Vec<Member>,
    /// A project block's internal variables.
    pub internals: Vec<Member>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", from = "WireMember")]
pub struct Member {
    pub name: String,
    pub data_type: DataType,
    pub value: Value,
}

// A JSON number doesn't say INT, DINT or TIME: values are decoded as their `dataType`.

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireVariable {
    program: String,
    name: String,
    data_type: DataType,
    value: Value,
    written_by_program: bool,
    io_address: Option<String>,
}

impl From<WireVariable> for Variable {
    fn from(v: WireVariable) -> Self {
        let value = typed(v.value, v.data_type);
        Self { program: v.program, name: v.name, data_type: v.data_type, value, written_by_program: v.written_by_program, io_address: v.io_address }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireMember {
    name: String,
    data_type: DataType,
    value: Value,
}

impl From<WireMember> for Member {
    fn from(m: WireMember) -> Self {
        Self { name: m.name, data_type: m.data_type, value: typed(m.value, m.data_type) }
    }
}

fn typed(value: Value, data_type: DataType) -> Value {
    let integer = match value {
        Value::Int(n) => i64::from(n),
        Value::DInt(n) => i64::from(n),
        Value::Time(ms) => ms,
        _ => return value,
    };
    match data_type {
        DataType::Int => i16::try_from(integer).map_or(value, Value::Int),
        DataType::DInt => i32::try_from(integer).map_or(value, Value::DInt),
        DataType::Time => Value::Time(integer),
        DataType::Real => Value::Real(integer as f64),
        DataType::Bool => value,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DebuggerState {
    /// The statement the PLC is PAUSED before, when a breakpoint or step stopped it
    /// mid-scan; None otherwise (also when paused between scans).
    pub location: Option<Location>,
    /// The breakpoints in effect (those on files the project has).
    pub breakpoints: Vec<Breakpoint>,
}

/// A statement in the source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    /// The program whose scan is running (the caller, when inside a function block).
    pub program: String,
    /// The source file's name in the project: the program's, or the function block's.
    pub file: String,
    pub line: usize,
    pub column: usize,
    /// Set when the statement is inside a project function block…
    pub function_block: Option<String>,
    /// …run by this instance, e.g. "Motor1" (nested: "Station1.Blinker").
    pub instance: Option<String>,
}

/// A failed scan: the program it failed in and the error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScanError {
    pub program: String,
    pub error: StError,
}

impl RuntimeState {
    /// The state of a host's runtime: the engine's facts plus what the host knows (its
    /// lifecycle, scan interval, I/O image and the last scan failure).
    pub fn from_engine(status: RuntimeStatus, scan_time_ms: u64, engine: EngineState, io: Vec<IoPoint>, error: Option<ScanError>) -> Self {
        RuntimeState {
            status,
            programs: engine.programs,
            cycle_count: engine.cycle_count,
            scan_time_ms,
            variables: engine.variables.into_iter().map(Variable::from).collect(),
            function_blocks: engine.function_blocks.into_iter().map(FunctionBlock::from).collect(),
            io,
            debugger: DebuggerState {
                location: engine.location.map(Location::from),
                breakpoints: engine.breakpoints.into_iter().map(|(file, line)| Breakpoint { file, line }).collect(),
            },
            error,
        }
    }
}

impl From<runtime::VariableState> for Variable {
    fn from(v: runtime::VariableState) -> Self {
        Self { program: v.program, name: v.name, data_type: v.data_type, value: v.value, written_by_program: v.written_by_program, io_address: v.io_address }
    }
}

impl From<runtime::MemberState> for Member {
    fn from(m: runtime::MemberState) -> Self {
        Self { name: m.name, data_type: m.data_type, value: m.value }
    }
}

impl From<runtime::FunctionBlockState> for FunctionBlock {
    fn from(b: runtime::FunctionBlockState) -> Self {
        let members = |list: Vec<runtime::MemberState>| list.into_iter().map(Member::from).collect();
        Self {
            program: b.program,
            instance: b.instance,
            type_name: b.type_name,
            standard: b.standard,
            inputs: members(b.inputs),
            outputs: members(b.outputs),
            internals: members(b.internals),
        }
    }
}

impl From<runtime::Location> for Location {
    fn from(l: runtime::Location) -> Self {
        Self { program: l.program, file: l.file, line: l.line, column: l.column, function_block: l.function_block, instance: l.instance }
    }
}
