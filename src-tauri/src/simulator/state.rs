use serde::Serialize;

use crate::io::IoPoint;
use crate::st::error::StError;
use crate::st::runtime::{FunctionBlockSnapshot, Location, VariableSnapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum RuntimeStatus {
    Stopped,
    Running,
    Paused,
}

/// Snapshot of the simulated PLC, returned by every execution command.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeState {
    pub status: RuntimeStatus,
    /// The loaded programs, in execution order.
    pub programs: Vec<String>,
    pub cycle_count: u64,
    pub scan_time_ms: u64,
    /// Plain variables.
    pub variables: Vec<VariableSnapshot>,
    /// Function block instances, listed separately with all their members.
    pub function_blocks: Vec<FunctionBlockSnapshot>,
    /// Set when a scan failed and stopped the runtime.
    pub error: Option<StError>,
    /// The program that `error` happened in.
    pub error_program: Option<String>,
    /// The simulated I/O: inputs as set by the user, outputs as written by the last scan.
    pub io: Vec<IoPoint>,
    /// Debugger: the statement the PLC is paused before, if a breakpoint or step stopped it
    /// mid-scan (None when paused between scans).
    pub location: Option<Location>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::IoKind;
    use crate::st::error::ErrorKind;
    use crate::st::runtime::MemberSnapshot;
    use crate::st::value::Value;

    /// Guards the JSON shape that `src/types/runtime.ts` relies on.
    #[test]
    fn serializes_to_frontend_contract() {
        let state = RuntimeState {
            status: RuntimeStatus::Running,
            programs: vec!["main".into()],
            cycle_count: 25,
            scan_time_ms: 100,
            variables: vec![
                VariableSnapshot { program: "main".into(), name: "Start".into(), data_type: "BOOL".into(), value: Value::Bool(true), is_input: true, io: Some("DI0".into()) },
                VariableSnapshot { program: "main".into(), name: "Result".into(), data_type: "INT".into(), value: Value::Int(-30), is_input: false, io: None },
                VariableSnapshot { program: "main".into(), name: "Total".into(), data_type: "DINT".into(), value: Value::DInt(100_000), is_input: false, io: None },
                VariableSnapshot { program: "main".into(), name: "Temp".into(), data_type: "REAL".into(), value: Value::Real(24.5), is_input: false, io: None },
            ],
            function_blocks: vec![FunctionBlockSnapshot {
                program: "main".into(),
                name: "Timer".into(),
                block_type: "TON".into(),
                members: vec![
                    MemberSnapshot { name: "PT".into(), data_type: "TIME".into(), value: Value::Time(2000), is_output: false, is_internal: false },
                    MemberSnapshot { name: "Q".into(), data_type: "BOOL".into(), value: Value::Bool(true), is_output: true, is_internal: false },
                ],
            }],
            error: Some(StError::new(ErrorKind::Runtime, 8, 5, "Division by zero")),
            error_program: Some("main".into()),
            io: vec![IoPoint { address: "AO0".into(), kind: IoKind::AnalogOutput, value: Value::Real(80.0) }],
            location: Some(Location {
                program: "main".into(),
                file: "CounterFB".into(),
                line: 12,
                column: 5,
                function_block: Some("CounterFB".into()),
                instance: Some("Counter1".into()),
            }),
        };
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "status": "RUNNING",
                "programs": ["main"],
                "cycleCount": 25,
                "scanTimeMs": 100,
                "variables": [
                    { "program": "main", "name": "Start", "dataType": "BOOL", "value": true, "isInput": true, "io": "DI0" },
                    { "program": "main", "name": "Result", "dataType": "INT", "value": -30, "isInput": false, "io": null },
                    { "program": "main", "name": "Total", "dataType": "DINT", "value": 100000, "isInput": false, "io": null },
                    { "program": "main", "name": "Temp", "dataType": "REAL", "value": 24.5, "isInput": false, "io": null }
                ],
                "functionBlocks": [{
                    "program": "main",
                    "name": "Timer",
                    "blockType": "TON",
                    "members": [
                        { "name": "PT", "dataType": "TIME", "value": 2000, "isOutput": false, "isInternal": false },
                        { "name": "Q", "dataType": "BOOL", "value": true, "isOutput": true, "isInternal": false }
                    ]
                }],
                "error": { "kind": "Runtime", "line": 8, "column": 5, "message": "Division by zero" },
                "errorProgram": "main",
                "io": [{ "address": "AO0", "kind": "AnalogOutput", "value": 80.0 }],
                "location": { "program": "main", "file": "CounterFB", "line": 12, "column": 5, "functionBlock": "CounterFB", "instance": "Counter1" }
            })
        );
    }
}
