//! Program outputs to the SCADA: command tags. A tag is an output when its class
//! (`control_table` group 1) is AnalogCommand or DigitalCommand, in any of the SCADA's tag
//! tables: `modbus_tags`, `iec104_tags`, `user_defined_tags`. Programs may write these (every
//! other tag stays read-only); what they write is sent to the SCADA's ST server, only when it
//! changes, in its own message format:
//!
//! `{"type":"STTagValues","total_data":1,"data":[{"TagID":…,"Register Address":20,"Value":340,"ProtocolType":"Modbus_TCP","TagClass":"AnalogCommand"}]}`
//!
//! Modbus and user-defined tags give their `start_address` as "Register Address", IEC104 tags
//! their `ioa` as "IOA Address".

use std::collections::HashMap;

use serde_json::{json, Map, Value};

/// What the SCADA needs to carry out one command tag.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub tag_id: String,
    pub address_key: &'static str,
    pub address: Option<i64>,
    pub protocol: String,
    pub class: String,
}

/// AnalogCommand / DigitalCommand (also written "Analog Command"), by name.
pub fn is_command(class: &str) -> bool {
    let c: String = class.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_lowercase();
    c == "analogcommand" || c == "digitalcommand"
}

pub fn is_digital(class: &str) -> bool {
    class.chars().filter(|c| !c.is_whitespace()).collect::<String>().eq_ignore_ascii_case("digitalcommand")
}

/// Every command tag, by name (upper case, like ST names). A table this database doesn't
/// have is skipped, so an older database just has fewer (or no) outputs.
pub fn load() -> Result<HashMap<String, Output>, String> {
    let mut client = crate::tags::connect()?;
    // (table, protocol, address key, address column); the class name comes from control_table
    // group 1 when tag_class holds its id, else tag_class is the name itself.
    let sources = [
        ("modbus_tags", "'Modbus_TCP'", "Register Address", "start_address"),
        ("iec104_tags", "'IEC104'", "IOA Address", "ioa"),
        ("user_defined_tags", "coalesce(nullif(u.protocol_binding, ''), 'UserDefined')", "Register Address", "start_address"),
    ];
    let mut outputs = HashMap::new();
    for (table, protocol, address_key, address) in sources {
        let sql = format!(
            "SELECT u.tag_name, u.id::text, {protocol}, u.{address}::int8, coalesce(c.tag, u.tag_class::text, '')
             FROM public.{table} u
             LEFT JOIN public.control_table c ON c.code = 1 AND c.id::text = u.tag_class::text
             WHERE u.tag_name IS NOT NULL"
        );
        let rows = match client.query(&sql, &[]) {
            Ok(rows) => rows,
            Err(e) if e.as_db_error().is_some_and(|d| d.code().code() == "42P01" || d.code().code() == "42703") => continue,
            Err(e) => return Err(format!("Can't read the command tags from {table}: {}", crate::tags::why(&e))),
        };
        for row in rows {
            let class: String = row.get(4);
            if is_command(&class) {
                let name: String = row.get(0);
                let output = Output { tag_id: row.get(1), address_key, address: row.get(3), protocol: row.get(2), class };
                outputs.insert(name.to_uppercase(), output);
            }
        }
    }
    Ok(outputs)
}

/// The message for these changed outputs (name, value), or None if none of them is known.
/// Whole numbers go as JSON integers (340, not 340.0), like the SCADA's own messages.
pub fn message(changes: &[(String, f64)], outputs: &HashMap<String, Output>) -> Option<String> {
    let data: Vec<Value> = changes
        .iter()
        .filter_map(|(name, value)| {
            let o = outputs.get(&name.to_uppercase())?;
            let mut item = Map::new();
            item.insert("TagID".into(), json!(o.tag_id));
            item.insert(o.address_key.into(), json!(o.address));
            item.insert("Value".into(), number(*value));
            item.insert("ProtocolType".into(), json!(o.protocol));
            item.insert("TagClass".into(), json!(o.class));
            Some(Value::Object(item))
        })
        .collect();
    if data.is_empty() {
        return None;
    }
    Some(json!({ "type": "STTagValues", "total_data": data.len(), "data": data }).to_string())
}

fn number(v: f64) -> Value {
    if v.fract() == 0.0 && v.abs() < 9.0e15 { json!(v as i64) } else { json!(v) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_the_scada_message() {
        assert!(is_command("AnalogCommand") && is_command("Analog Command") && is_command("DigitalCommand"));
        assert!(!is_command("Analog") && !is_command("3"));
        let outputs = HashMap::from([
            ("SETPOINT".to_string(), Output { tag_id: "0047f8e7".into(), address_key: "Register Address", address: Some(20), protocol: "Modbus_TCP".into(), class: "AnalogCommand".into() }),
            ("BREAKER".to_string(), Output { tag_id: "fcf4867e".into(), address_key: "IOA Address", address: Some(2100), protocol: "IEC104".into(), class: "DigitalCommand".into() }),
        ]);
        let text = message(&[("Setpoint".into(), 340.0), ("BREAKER".into(), 1.0), ("unknown".into(), 5.0)], &outputs).unwrap();
        let m: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(m["type"], "STTagValues");
        assert_eq!(m["total_data"], 2);
        let modbus = m["data"].as_array().unwrap().iter().find(|d| d["TagID"] == "0047f8e7").unwrap();
        assert_eq!(*modbus, json!({"TagID":"0047f8e7","Register Address":20,"Value":340,"ProtocolType":"Modbus_TCP","TagClass":"AnalogCommand"}));
        let iec = m["data"].as_array().unwrap().iter().find(|d| d["TagID"] == "fcf4867e").unwrap();
        assert_eq!(iec["IOA Address"], 2100);
        assert_eq!(iec["Value"], 1);
        assert_eq!(message(&[("SETPOINT".into(), 45.5)], &outputs).map(|t| t.contains("\"Value\":45.5")), Some(true));
        assert_eq!(message(&[("unknown".into(), 1.0)], &outputs), None);
    }
}
