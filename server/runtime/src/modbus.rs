//! Live tag values read straight from the Modbus TCP devices, set up entirely in the SCADA's
//! own tables (nothing is hardcoded):
//! - `modbus_tcp_devices`: ip_address, port, unit_id, scan_rate, timeout_constant, offset, is_active
//! - `modbus_tags`: register_type (control_table group 2: 7 Holding, 8 Input, 9 Coil,
//!   10 Discrete input), data_format (group 5: 54 Binary … 68 Double), start_address,
//!   multiplication_factor, offset, inversion
//!
//! Only tags programs use (`st_tags`) are read. On with `MYPLC_MODBUS=1` (and
//! `MYPLC_SCADA_URL` empty). The register sent is `start_address - device offset`.

use std::collections::HashMap;
use std::env;
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::host::Host;

/// How often the list of tags to read is re-read (a program may start using a new tag).
const TAG_REFRESH: Duration = Duration::from_secs(10);

pub fn enabled() -> bool {
    env::var("MYPLC_MODBUS").is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("on") || v.eq_ignore_ascii_case("true"))
}

#[derive(Clone, PartialEq, Debug)]
struct Device {
    addr: String,
    unit: u8,
    scan: Duration,
    timeout: Duration,
}

#[derive(Debug)]
struct Point {
    name: String,
    fc: u8,
    reg: u16,
    format: i32,
    factor: f64,
    offset: f64,
    invert: bool,
}

/// register_type → Modbus function code.
fn function(register_type: i32) -> Option<u8> {
    match register_type {
        7 => Some(3),  // HoldingRegister
        8 => Some(4),  // InputRegister
        9 => Some(1),  // Coil
        10 => Some(2), // DiscreteInput
        _ => None,
    }
}

/// Registers a data_format takes.
fn width(format: i32) -> u16 {
    match format {
        57..=64 => 2,
        65..=68 => 4,
        _ => 1,
    }
}

/// The raw value of `words` in a data_format: 54 Binary, 55 Signed, 56 Unsigned,
/// 57-60 Float, 61-64 Long, 65-68 Double, each in the order ABCD, CDAB, BADC, DCBA.
fn decode(format: i32, words: &[u16]) -> Option<f64> {
    let first = *words.first()?;
    let order = match format {
        54 => return Some(f64::from(u8::from(first != 0))),
        55 => return Some(f64::from(first as i16)),
        56 => return Some(f64::from(first)),
        57..=64 => (format - 57) % 4,
        65..=68 => format - 65,
        _ => return None,
    };
    let mut w = words.get(..usize::from(width(format)))?.to_vec();
    if order & 1 == 1 {
        w.reverse(); // CDAB / GHEF CDAB: words swapped
    }
    if order & 2 == 2 {
        w.iter_mut().for_each(|x| *x = x.swap_bytes()); // BADC / FEHG: bytes swapped
    }
    let b: Vec<u8> = w.iter().flat_map(|x| x.to_be_bytes()).collect();
    Some(match format {
        57..=60 => f64::from(f32::from_be_bytes(b.try_into().ok()?)),
        61..=64 => f64::from(i32::from_be_bytes(b.try_into().ok()?)),
        _ => f64::from_be_bytes(b.try_into().ok()?),
    })
}

/// One Modbus TCP read. Outer error: the connection failed. Inner error: the device answered
/// with an exception code (bad address etc.), the connection is still fine.
fn read(s: &mut TcpStream, unit: u8, tid: u16, fc: u8, reg: u16, count: u16) -> io::Result<Result<Vec<u16>, u8>> {
    let mut req = Vec::with_capacity(12);
    req.extend(tid.to_be_bytes());
    req.extend([0, 0, 0, 6, unit, fc]);
    req.extend(reg.to_be_bytes());
    req.extend(count.to_be_bytes());
    s.write_all(&req)?;
    // tid(2) protocol(2) length(2) unit(1) fc(1) then byte count, or the exception code.
    let mut head = [0u8; 9];
    s.read_exact(&mut head)?;
    if head[7] & 0x80 != 0 {
        return Ok(Err(head[8]));
    }
    let mut data = vec![0; usize::from(head[8])];
    s.read_exact(&mut data)?;
    if head[0..2] != tid.to_be_bytes() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "reply to a different request"));
    }
    if fc <= 2 {
        return Ok(Ok(vec![u16::from(data.first().copied().unwrap_or(0) & 1)]));
    }
    let words: Vec<u16> = data.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
    if words.len() < usize::from(count) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "short reply"));
    }
    Ok(Ok(words))
}

/// The used tags, grouped by their (active) device.
fn load() -> Result<Vec<(Device, Vec<Point>)>, String> {
    let rows = crate::tags::connect()?
        .query(
            r#"SELECT d.id, coalesce(d.ip_address, ''), coalesce(d.port, 502), coalesce(d.unit_id, 1),
                      coalesce(d.scan_rate, 1000), coalesce(d.timeout_constant, 3000), coalesce(d."offset", 0),
                      m.tag_name, coalesce(m.register_type, 0), coalesce(m.data_format, 56), coalesce(m.start_address, 0),
                      coalesce(m.multiplication_factor, 1)::float8, coalesce(m."offset", 0)::float8, coalesce(m.inversion, false)
               FROM modbus_tags m JOIN modbus_tcp_devices d ON d.id = m.device_id
               WHERE d.is_active IS NOT FALSE
                 AND m.tag_name IN (SELECT tag_name FROM st_tags WHERE usage_count > 0)
               ORDER BY d.id, m.start_address"#,
            &[],
        )
        .map_err(|e| format!("Can't read the Modbus setup: {}", crate::tags::why(&e)))?;
    let mut devices: Vec<(i32, Device, Vec<Point>)> = Vec::new();
    for row in &rows {
        let id: i32 = row.get(0);
        let name: String = row.get(7);
        let (register_type, format, start, base): (i32, i32, i32, i32) = (row.get(8), row.get(9), row.get(10), row.get(6));
        let (Some(fc), Ok(reg)) = (function(register_type), u16::try_from(start - base)) else {
            eprintln!("[modbus] {name}: register type {register_type} / address {start} (device offset {base}) can't be read; skipped");
            continue;
        };
        let point = Point { name, fc, reg, format, factor: row.get(11), offset: row.get(12), invert: row.get(13) };
        match devices.last_mut() {
            Some((last, _, points)) if *last == id => points.push(point),
            _ => {
                let ms = |i: usize| Duration::from_millis(u64::try_from(row.get::<_, i32>(i)).unwrap_or(1000).max(100));
                let device = Device {
                    addr: format!("{}:{}", row.get::<_, String>(1).trim(), row.get::<_, i32>(2)),
                    unit: u8::try_from(row.get::<_, i32>(3)).unwrap_or(1),
                    scan: ms(4),
                    timeout: ms(5),
                };
                devices.push((id, device, vec![point]));
            }
        }
    }
    Ok(devices.into_iter().map(|(_, d, p)| (d, p)).collect())
}

/// Forever: read the used tags from their devices and give the values to `host`. A device
/// that goes away is retried every scan; its tags keep their last values.
// ponytail: devices are read one after another, so a dead one slows the rest by its timeout;
// one thread per device if several devices are used at once.
pub fn poll(host: Arc<Host>) {
    let mut devices = Vec::new();
    let mut loaded: Option<Instant> = None;
    let mut links: HashMap<String, TcpStream> = HashMap::new();
    let mut said: HashMap<String, String> = HashMap::new(); // last message per device/tag, said once
    let mut say = |key: &str, msg: String| {
        if said.get(key) != Some(&msg) {
            eprintln!("[modbus] {msg}");
            said.insert(key.to_string(), msg);
        }
    };
    let mut tid = 0u16;
    loop {
        if loaded.is_none_or(|t| t.elapsed() >= TAG_REFRESH) {
            match load() {
                Ok(fresh) => {
                    let list: Vec<String> = fresh.iter().map(|(d, p)| format!("{} unit {} ({} tags)", d.addr, d.unit, p.len())).collect();
                    say("list", format!("reading {}", if list.is_empty() { "nothing yet: no used tag is in modbus_tags".into() } else { list.join(", ") }));
                    devices = fresh;
                }
                Err(e) => say("db", e),
            }
            loaded = Some(Instant::now());
        }
        let mut values = Vec::new();
        for (device, points) in &devices {
            if !links.contains_key(&device.addr) {
                let target = device.addr.to_socket_addrs().ok().and_then(|mut a| a.next());
                match target.map(|t| TcpStream::connect_timeout(&t, device.timeout)) {
                    Some(Ok(s)) => {
                        s.set_read_timeout(Some(device.timeout)).ok();
                        s.set_nodelay(true).ok();
                        say(&device.addr, format!("connected to {}", device.addr));
                        links.insert(device.addr.clone(), s);
                    }
                    Some(Err(e)) => {
                        say(&device.addr, format!("can't connect to {}: {e} (tags keep their last values; retrying)", device.addr));
                        continue;
                    }
                    None => {
                        say(&device.addr, format!("bad device address {}", device.addr));
                        continue;
                    }
                }
            }
            let s = links.get_mut(&device.addr).expect("connected above");
            for p in points {
                tid = tid.wrapping_add(1);
                let count = if p.fc <= 2 { 1 } else { width(p.format) };
                match read(s, device.unit, tid, p.fc, p.reg, count) {
                    Ok(Ok(words)) => {
                        let Some(raw) = decode(if p.fc <= 2 { 54 } else { p.format }, &words) else { continue };
                        let value = if p.fc <= 2 || p.format == 54 {
                            if p.invert { 1.0 - raw } else { raw }
                        } else {
                            raw * p.factor + p.offset
                        };
                        if value.is_finite() {
                            values.push((p.name.clone(), value));
                        }
                    }
                    Ok(Err(code)) => say(&p.name, format!("{} refused {} (register {}, exception {code})", device.addr, p.name, p.reg)),
                    Err(e) => {
                        say(&device.addr, format!("connection to {} lost: {e} (retrying)", device.addr));
                        links.remove(&device.addr);
                        break;
                    }
                }
            }
        }
        if !values.is_empty() {
            host.set_tag_values(values);
        }
        thread::sleep(devices.iter().map(|(d, _)| d.scan).min().unwrap_or(Duration::from_secs(1)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn formats() {
        // 50.0f32 = 0x4248_0000
        assert_eq!(decode(57, &[0x4248, 0x0000]), Some(50.0)); // Float ABCD
        assert_eq!(decode(58, &[0x0000, 0x4248]), Some(50.0)); // Float CDAB
        assert_eq!(decode(59, &[0x4842, 0x0000]), Some(50.0)); // Float BADC
        assert_eq!(decode(60, &[0x0000, 0x4842]), Some(50.0)); // Float DCBA
        assert_eq!(decode(55, &[0xFFFF]), Some(-1.0)); // Signed
        assert_eq!(decode(56, &[0xFFFF]), Some(65535.0)); // Unsigned
        assert_eq!(decode(54, &[5]), Some(1.0)); // Binary
        assert_eq!(decode(61, &[0xFFFF, 0xFFFE]), Some(-2.0)); // Long ABCD
        assert_eq!(decode(62, &[0xFFFE, 0xFFFF]), Some(-2.0)); // Long CDAB
        assert_eq!(decode(65, &[0x4049, 0, 0, 0]), Some(50.0)); // Double ABCD EFGH
        assert_eq!(decode(66, &[0, 0, 0, 0x4049]), Some(50.0)); // Double GHEF CDAB
        assert_eq!(decode(57, &[0x4248]), None); // too few registers
        assert_eq!((function(7), function(9), function(1)), (Some(3), Some(1), None));
    }

    #[test]
    fn reads_over_tcp() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut c, _) = listener.accept().unwrap();
            let mut req = [0u8; 12];
            c.read_exact(&mut req).unwrap();
            assert_eq!(req[6..], [1, 3, 0, 6, 0, 2]); // unit 1, FC3, register 6, 2 registers
            c.write_all(&[req[0], req[1], 0, 0, 0, 7, 1, 3, 4, 0x42, 0x48, 0, 0]).unwrap();
            c.read_exact(&mut req).unwrap();
            assert_eq!(req[7], 1); // FC1 coil
            c.write_all(&[req[0], req[1], 0, 0, 0, 4, 1, 1, 1, 1]).unwrap();
            c.read_exact(&mut req).unwrap();
            c.write_all(&[req[0], req[1], 0, 0, 0, 3, 1, 0x83, 2]).unwrap(); // exception 2
        });
        let mut s = TcpStream::connect(addr).unwrap();
        let words = read(&mut s, 1, 7, 3, 6, 2).unwrap().unwrap();
        assert_eq!(decode(57, &words), Some(50.0));
        assert_eq!(read(&mut s, 1, 8, 1, 0, 1).unwrap(), Ok(vec![1]));
        assert_eq!(read(&mut s, 1, 9, 3, 6, 2).unwrap(), Err(2));
    }
}
