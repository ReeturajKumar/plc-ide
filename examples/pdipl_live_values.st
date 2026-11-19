PROGRAM PdiplLive
VAR
    (* Latest live values from PDIPL_DEVICE (192.168.1.150) *)
    Freq      : REAL := 0.0;   (* AUX_HZ *)
    PowerFact : REAL := 0.0;   (* AUX_AVG_PF *)
    PowerB    : REAL := 0.0;   (* AUX_BPH_KW *)
    CurrentB  : REAL := 0.0;   (* AUX_BPH_AMP *)
    VoltBR    : REAL := 0.0;   (* AUX_BR_VOLT *)
    EnergyIn  : REAL := 0.0;   (* AUX_KWHI *)
    AllLive   : BOOL := FALSE; (* TRUE once every tag has a live value *)
END_VAR

(* Each tag X also has X_VALID: TRUE once a live value has arrived from the device. *)
AllLive := AUX_HZ_VALID AND AUX_AVG_PF_VALID AND AUX_BPH_KW_VALID
       AND AUX_BPH_AMP_VALID AND AUX_BR_VOLT_VALID AND AUX_KWHI_VALID;

(* Copy the live values (watch these in the IDE) *)
Freq      := AUX_HZ;
PowerFact := AUX_AVG_PF;
PowerB    := AUX_BPH_KW;
CurrentB  := AUX_BPH_AMP;
VoltBR    := AUX_BR_VOLT;
EnergyIn  := AUX_KWHI;

(* DO0 = all tags live, AO0 = frequency, AO1 = B-phase kW *)
DO0 := AllLive;
AO0 := Freq;
AO1 := PowerB;
END_PROGRAM
