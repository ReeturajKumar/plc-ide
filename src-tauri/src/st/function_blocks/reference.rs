//! Read-only Structured Text definitions of the standard function blocks, shown in the
//! IDE's library. The blocks run as Rust (the sibling modules); these sources describe the
//! same behavior in ST. The counters and edge detectors are plain ST, checked against the
//! Rust blocks by the tests below. The timers need the PLC scan clock, which ST has no
//! keyword for: their source shows it as the pseudo-variables `SCAN_START` / `SCAN_END`.

use super::FbKind;

const HEADER: &str = "(* Standard function block - read-only reference.
   It runs as built-in code; this is the same behavior written in Structured Text. *)
";

const TIMER_CLOCK: &str = "(* SCAN_START / SCAN_END: PLC time at the start and end of the current scan.
   Timers run on scan time: each scan advances it by exactly one scan interval. *)
";

const TON: &str = "FUNCTION_BLOCK TON
VAR_INPUT
    IN : BOOL;      (* start timing while TRUE *)
    PT : TIME;      (* preset time *)
END_VAR
VAR_OUTPUT
    Q  : BOOL;      (* TRUE once IN has been TRUE for PT *)
    ET : TIME;      (* elapsed time, 0 .. PT *)
END_VAR
VAR
    Running   : BOOL;
    StartedAt : TIME;
END_VAR

IF IN THEN
    IF NOT Running THEN
        Running := TRUE;
        StartedAt := SCAN_START;    (* IN is taken to rise at the scan's start *)
    END_IF;
    ET := MIN(SCAN_END - StartedAt, PT);
    Q := ET >= PT;
ELSE
    Running := FALSE;
    ET := T#0s;
    Q := FALSE;
END_IF;

END_FUNCTION_BLOCK
";

const TOF: &str = "FUNCTION_BLOCK TOF
VAR_INPUT
    IN : BOOL;      (* Q follows IN; stays TRUE for PT after IN falls *)
    PT : TIME;      (* off-delay time *)
END_VAR
VAR_OUTPUT
    Q  : BOOL;
    ET : TIME;      (* time since IN fell, 0 .. PT *)
END_VAR
VAR
    PreviousIn : BOOL;
    Timing     : BOOL;      (* IN fell and the delay is running or over *)
    FellAt     : TIME;
END_VAR

IF IN THEN
    Timing := FALSE;
    Q := TRUE;
    ET := T#0s;
ELSE
    IF PreviousIn THEN
        Timing := TRUE;
        FellAt := SCAN_START;
    END_IF;
    (* Before IN was ever TRUE nothing is timing: Q stays FALSE. *)
    IF Timing THEN
        ET := MIN(SCAN_END - FellAt, PT);
        Q := ET < PT;
    END_IF;
END_IF;
PreviousIn := IN;

END_FUNCTION_BLOCK
";

const TP: &str = "FUNCTION_BLOCK TP
VAR_INPUT
    IN : BOOL;      (* a rising edge starts a pulse *)
    PT : TIME;      (* pulse length *)
END_VAR
VAR_OUTPUT
    Q  : BOOL;      (* TRUE for PT, whatever IN does meanwhile *)
    ET : TIME;
END_VAR
VAR
    PreviousIn : BOOL;
    Pulsing    : BOOL;      (* a pulse started and hasn't been reset *)
    StartedAt  : TIME;
END_VAR

IF IN AND NOT PreviousIn AND NOT Pulsing THEN
    Pulsing := TRUE;
    StartedAt := SCAN_START;
END_IF;
PreviousIn := IN;

IF NOT Pulsing THEN
    Q := FALSE;
    ET := T#0s;
ELSE
    ET := MIN(SCAN_END - StartedAt, PT);
    Q := ET < PT;
    (* After the pulse, ET holds at PT while IN stays TRUE. *)
    IF NOT Q AND NOT IN THEN
        Pulsing := FALSE;
        ET := T#0s;
    END_IF;
END_IF;

END_FUNCTION_BLOCK
";

const CTU: &str = "FUNCTION_BLOCK CTU
VAR_INPUT
    CU : BOOL;      (* count up on each rising edge *)
    R  : BOOL;      (* reset CV to 0 *)
    PV : INT;       (* preset value *)
END_VAR
VAR_OUTPUT
    Q  : BOOL;      (* CV >= PV *)
    CV : INT;       (* current value *)
END_VAR
VAR
    PreviousCU : BOOL;
END_VAR

IF R THEN
    CV := 0;
ELSIF CU AND NOT PreviousCU AND CV < 32767 THEN
    CV := CV + 1;
END_IF;
PreviousCU := CU;
Q := CV >= PV;

END_FUNCTION_BLOCK
";

const CTD: &str = "FUNCTION_BLOCK CTD
VAR_INPUT
    CD : BOOL;      (* count down on each rising edge *)
    LD : BOOL;      (* load CV with PV *)
    PV : INT;       (* preset value *)
END_VAR
VAR_OUTPUT
    Q  : BOOL;      (* CV <= 0 *)
    CV : INT;       (* current value *)
END_VAR
VAR
    PreviousCD : BOOL;
END_VAR

IF LD THEN
    CV := PV;
ELSIF CD AND NOT PreviousCD AND CV > -32768 THEN
    CV := CV - 1;
END_IF;
PreviousCD := CD;
Q := CV <= 0;

END_FUNCTION_BLOCK
";

const R_TRIG: &str = "FUNCTION_BLOCK R_TRIG
VAR_INPUT
    CLK : BOOL;
END_VAR
VAR_OUTPUT
    Q : BOOL;       (* TRUE for one call after CLK goes FALSE -> TRUE *)
END_VAR
VAR
    PreviousCLK : BOOL;
END_VAR

Q := CLK AND NOT PreviousCLK;
PreviousCLK := CLK;

END_FUNCTION_BLOCK
";

const F_TRIG: &str = "FUNCTION_BLOCK F_TRIG
VAR_INPUT
    CLK : BOOL;
END_VAR
VAR_OUTPUT
    Q : BOOL;       (* TRUE for one call after CLK goes TRUE -> FALSE *)
END_VAR
VAR
    PreviousCLK : BOOL;     (* starts FALSE: a CLK that starts FALSE doesn't pulse *)
END_VAR

Q := NOT CLK AND PreviousCLK;
PreviousCLK := CLK;

END_FUNCTION_BLOCK
";

/// The ST definition of a standard block, with a header saying it is a reference.
pub fn reference_source(kind: FbKind) -> String {
    let (clock, body) = match kind {
        FbKind::Ton => (TIMER_CLOCK, TON),
        FbKind::Tof => (TIMER_CLOCK, TOF),
        FbKind::Tp => (TIMER_CLOCK, TP),
        FbKind::Ctu => ("", CTU),
        FbKind::Ctd => ("", CTD),
        FbKind::RTrig => ("", R_TRIG),
        FbKind::FTrig => ("", F_TRIG),
    };
    format!("{HEADER}{clock}\n{body}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::st::ast::{DataType, Program, VarType};
    use crate::st::function_blocks::ScanClock;
    use crate::st::parser::{parse, parse_file};
    use crate::st::runtime::Runtime;
    use crate::st::value::Value;

    /// The reference CTU/CTD/R_TRIG/F_TRIG, compiled as user blocks (renamed, since the
    /// standard names are reserved), give the same outputs as the Rust blocks for the same
    /// input sequence.
    #[test]
    fn plain_st_references_match_the_builtin_blocks() {
        for (kind, args, outputs) in [
            (FbKind::Ctu, "CU := a, R := b, PV := 3", ["Q", "CV"].as_slice()),
            (FbKind::Ctd, "CD := a, LD := b, PV := 3", ["Q", "CV"].as_slice()),
            (FbKind::RTrig, "CLK := a", ["Q"].as_slice()),
            (FbKind::FTrig, "CLK := a", ["Q"].as_slice()),
        ] {
            let name = kind.name();
            let mine = format!("My_{name}");
            let source = reference_source(kind).replace(&format!("FUNCTION_BLOCK {name}\n"), &format!("FUNCTION_BLOCK {mine}\n"));
            let fb = parse_file(&source).unwrap().function_blocks.remove(0);
            let program: Program = parse(&format!(
                "PROGRAM p VAR a : BOOL; b : BOOL; Std : {name}; Ref : {mine}; END_VAR Std({args}); Ref({args}); END_PROGRAM"
            ))
            .unwrap();
            let mut rt = Runtime::build(vec![("p".into(), program)], vec![fb], &[]).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            // a: edges; b: reset/load pulses.
            let a = [true, false, true, true, false, true, false, true, false, true, false, true];
            let b = [false, false, false, false, false, true, false, false, true, false, false, false];
            for (step, (a, b)) in a.iter().zip(b).enumerate() {
                rt.set("a", Value::Bool(*a)).unwrap();
                rt.set("b", Value::Bool(b)).unwrap();
                rt.scan(ScanClock { now_ms: 0, cycle_ms: 0 }).unwrap();
                for output in outputs {
                    let builtin = rt.member("Std", output);
                    let reference = rt.function_blocks().into_iter().find(|f| f.name == "Ref").unwrap();
                    let reference = reference.members.into_iter().find(|m| m.name == *output).map(|m| m.value);
                    assert_eq!(reference, builtin, "{name}.{output} at step {step}");
                }
            }
        }
    }

    #[test]
    fn every_reference_declares_the_real_interface() {
        for kind in FbKind::ALL {
            let source = reference_source(kind);
            assert!(source.starts_with("(* Standard function block - read-only reference."));
            let name = kind.name();
            let renamed = source.replace(&format!("FUNCTION_BLOCK {name}\n"), "FUNCTION_BLOCK X\n");
            // Timers use the SCAN_* pseudo-variables, so only their declarations are parsed.
            let fb = parse_file(&renamed).unwrap().function_blocks.remove(0);
            let decls = |vars: &[crate::st::ast::VarDecl]| vars.iter().map(|v| (v.name.clone(), v.var_type)).collect::<Vec<_>>();
            let expect = |members: &[(&str, DataType)]| members.iter().map(|(n, t)| (n.to_string(), VarType::Elementary(*t))).collect::<Vec<_>>();
            assert_eq!(decls(&fb.inputs), expect(kind.inputs()), "{name} inputs");
            assert_eq!(decls(&fb.outputs), expect(kind.outputs()), "{name} outputs");
        }
    }
}
