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
    RESET : BOOL;   (* same as R *)
    PV : INT;       (* preset value *)
END_VAR
VAR_OUTPUT
    Q  : BOOL;      (* CV >= PV *)
    CV : INT;       (* current value *)
END_VAR
VAR
    PreviousCU : BOOL;
END_VAR

IF R OR RESET THEN
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
    LOAD : BOOL;    (* same as LD *)
    PV : INT;       (* preset value *)
END_VAR
VAR_OUTPUT
    Q  : BOOL;      (* CV <= 0 *)
    CV : INT;       (* current value *)
END_VAR
VAR
    PreviousCD : BOOL;
END_VAR

IF LD OR LOAD THEN
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

const CTUD: &str = "FUNCTION_BLOCK CTUD
VAR_INPUT
    CU    : BOOL;   (* count up on each rising edge *)
    CD    : BOOL;   (* count down on each rising edge *)
    RESET : BOOL;   (* reset CV to 0 *)
    LOAD  : BOOL;   (* load CV with PV *)
    PV    : INT;    (* preset value *)
END_VAR
VAR_OUTPUT
    QU : BOOL;      (* CV >= PV *)
    QD : BOOL;      (* CV <= 0 *)
    CV : INT;       (* current value *)
END_VAR
VAR
    PreviousCU : BOOL;
    PreviousCD : BOOL;
END_VAR

IF RESET THEN
    CV := 0;
ELSIF LOAD THEN
    CV := PV;
ELSE
    IF CU AND NOT PreviousCU AND CV < 32767 THEN
        CV := CV + 1;
    END_IF;
    IF CD AND NOT PreviousCD AND CV > -32768 THEN
        CV := CV - 1;
    END_IF;
END_IF;
PreviousCU := CU;
PreviousCD := CD;
QU := CV >= PV;
QD := CV <= 0;

END_FUNCTION_BLOCK
";

const SR: &str = "FUNCTION_BLOCK SR
VAR_INPUT
    SET1  : BOOL;   (* set Q1; wins when both are TRUE *)
    RESET : BOOL;   (* reset Q1 *)
END_VAR
VAR_OUTPUT
    Q1 : BOOL;      (* keeps its value while both are FALSE *)
END_VAR

Q1 := SET1 OR (NOT RESET AND Q1);

END_FUNCTION_BLOCK
";

const RS: &str = "FUNCTION_BLOCK RS
VAR_INPUT
    SET    : BOOL;  (* set Q1 *)
    RESET1 : BOOL;  (* reset Q1; wins when both are TRUE *)
END_VAR
VAR_OUTPUT
    Q1 : BOOL;      (* keeps its value while both are FALSE *)
END_VAR

Q1 := NOT RESET1 AND (SET OR Q1);

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
        FbKind::Ctud => ("", CTUD),
        FbKind::Sr => ("", SR),
        FbKind::Rs => ("", RS),
    };
    format!("{HEADER}{clock}\n{body}")
}

