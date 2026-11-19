//! Live tag values read straight from the Modbus TCP devices, set up entirely in the SCADA's
//! own tables (nothing is hardcoded):
//! - `modbus_tcp_devices`: ip_address, port, unit_id, scan_rate, timeout_constant, offset, is_active
//! - `modbus_tags`: register_type (control_table group 2: 7 Holding, 8 Input, 9 Coil,
//!   10 Discrete input), data_format (group 5: 54 Binary … 68 Double), start_address,
//!   multiplication_factor, offset, inversion
//!
//! Only tags programs use (`st_tags`) are read. On with `MYPLC_MODBUS=1` (and
//! `MYPLC_SCADA_URL` empty). The register sent is `start_address - device offset`.
//!
//! Built for many tags: neighbouring registers are read together (up to 125 registers or
//! 2000 coils per request), every device has its own thread on its own fixed schedule
//! (`scan_rate`), so a slow or dead device only delays its own tags, and only values that
//! changed are passed on.

use std::collections::{HashMap, HashSet};
use std::env;
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::host::Host;

/// How often the list of tags to read is re-read (a program may start using a new tag).
const TAG_REFRESH: Duration = Duration::from_secs(10);
/// The most one request may read (the Modbus limits).
const MAX_REGISTERS: u32 = 125;
const MAX_BITS: u32 = 2000;
/// Unused registers a block may span to join two tags: reading a few extra is cheaper than
/// another request. A device that refuses a block (a hole in its map) gets those tags one by one.
const MAX_GAP: u32 = 8;

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

#[derive(Clone, PartialEq, Debug)]
struct Point {
    name: String,
    fc: u8,
    reg: u16,
    format: i32,
    factor: f64,
    offset: f64,
    invert: bool,
}

impl Point {
    fn bits(&self) -> bool {
        self.fc <= 2
    }

    /// Registers (or coils) it takes.
    fn width(&self) -> u16 {
        if self.bits() { 1 } else { width(self.format) }
    }

    /// Its engineering value from its own registers (or bit).
    fn value(&self, words: &[u16]) -> Option<f64> {
        let raw = decode(if self.bits() { 54 } else { self.format }, words)?;
        let value = if self.bits() || self.format == 54 {
            if self.invert { 1.0 - raw } else { raw }
        } else {
            raw * self.factor + self.offset
        };
        value.is_finite().then_some(value)
    }
}

/// One request: `count` registers (or coils) from `start`, holding these tags.
#[derive(Debug)]
struct Block {
    fc: u8,
    start: u16,
    count: u16,
    points: Vec<Point>,
}

impl Block {
    fn single(point: Point) -> Self {
        Self { fc: point.fc, start: point.reg, count: point.width(), points: vec![point] }
    }
}

/// The fewest requests that read all `points`: same function, sorted by address, joined
/// while the gap is small and the request stays within the Modbus limit.
fn blocks(mut points: Vec<Point>) -> Vec<Block> {
    points.sort_by_key(|p| (p.fc, p.reg));
    let mut out: Vec<Block> = Vec::new();
    for p in points {
        let end = u32::from(p.reg) + u32::from(p.width());
        if let Some(b) = out.last_mut() {
            let (start, b_end) = (u32::from(b.start), u32::from(b.start) + u32::from(b.count));
            let limit = if p.bits() { MAX_BITS } else { MAX_REGISTERS };
            if b.fc == p.fc && u32::from(p.reg) <= b_end + MAX_GAP && end.max(b_end) - start <= limit {
                b.count = (end.max(b_end) - start) as u16;
                b.points.push(p);
                continue;
            }
        }
        out.push(Block::single(p));
    }
    out
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

/// One Modbus TCP read of `count` registers (or coils, one 0/1 per coil). Outer error: the
/// connection failed. Inner error: the device answered with an exception code (bad address
/// etc.), the connection is still fine.
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
    let short = || io::Error::new(io::ErrorKind::InvalidData, "short reply");
    if head[0..2] != tid.to_be_bytes() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "reply to a different request"));
    }
    if fc <= 2 {
        if data.len() < usize::from(count).div_ceil(8) {
            return Err(short());
        }
        return Ok(Ok((0..usize::from(count)).map(|i| u16::from((data[i / 8] >> (i % 8)) & 1)).collect()));
    }
    let words: Vec<u16> = data.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
    if words.len() < usize::from(count) {
        return Err(short());
    }
    Ok(Ok(words))
}

type Config = (Device, Vec<Point>);

/// The used tags, grouped by their (active) device.
fn load() -> Result<Vec<Config>, String> {
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
                // scan_rate may be below 1 s for fast tags; 1 ms is the floor.
                let ms = |i: usize, floor: u64| Duration::from_millis(u64::try_from(row.get::<_, i32>(i)).unwrap_or(1000).max(floor));
                let device = Device {
                    addr: format!("{}:{}", row.get::<_, String>(1).trim(), row.get::<_, i32>(2)),
                    unit: u8::try_from(row.get::<_, i32>(3)).unwrap_or(1),
                    scan: ms(4, 1),
                    timeout: ms(5, 100),
                };
                devices.push((id, device, vec![point]));
            }
        }
    }
    Ok(devices.into_iter().map(|(_, d, p)| (d, p)).collect())
}

/// Forever: keep one reader thread per device that has used tags, each given its latest
/// setup. Re-reads the setup every `TAG_REFRESH`.
pub fn poll(host: Arc<Host>) {
    let mut workers: HashMap<String, (Sender<Config>, Config)> = HashMap::new();
    let mut last_error = String::new();
    let mut last_list = String::new();
    loop {
        match load() {
            Ok(configs) => {
                last_error.clear();
                let list: Vec<String> = configs.iter().map(|(d, p)| format!("{} unit {} ({} tags)", d.addr, d.unit, p.len())).collect();
                let list = if list.is_empty() { "nothing yet: no used tag is in modbus_tags".into() } else { list.join(", ") };
                if list != last_list {
                    eprintln!("[modbus] reading {list}");
                    last_list = list;
                }
                let mut seen = HashSet::new();
                for config in configs {
                    let key = format!("{} unit {}", config.0.addr, config.0.unit);
                    seen.insert(key.clone());
                    let alive = match workers.get(&key) {
                        Some((_, current)) if *current == config => continue,
                        Some((tx, _)) => tx.send(config.clone()).is_ok(),
                        None => false,
                    };
                    if alive {
                        workers.get_mut(&key).expect("present").1 = config;
                    } else {
                        let (tx, rx) = mpsc::channel();
                        let _ = tx.send(config.clone());
                        let host = Arc::clone(&host);
                        thread::Builder::new()
                            .name(format!("modbus {key}"))
                            .spawn(move || read_device(&rx, &host))
                            .expect("can't start a Modbus reader thread");
                        workers.insert(key, (tx, config));
                    }
                }
                workers.retain(|key, _| seen.contains(key)); // dropping the sender stops that thread
            }
            Err(e) => {
                if e != last_error {
                    eprintln!("[modbus] {e} (retrying)");
                    last_error = e;
                }
            }
        }
        thread::sleep(TAG_REFRESH);
    }
}

/// One device, forever (until its setup is withdrawn): read all its blocks every `scan_rate`
/// on a fixed schedule, and pass on the values that changed.
fn read_device(rx: &Receiver<Config>, host: &Host) {
    let Ok((mut device, points)) = rx.recv() else { return };
    let mut blocks = blocks(points);
    let mut link: Option<TcpStream> = None;
    let mut last: HashMap<String, f64> = HashMap::new();
    let mut said = String::new();
    let mut refused = HashSet::new();
    let mut tid = 0u16;
    let mut next = Instant::now();
    let mut say = |msg: String| {
        if msg != said {
            eprintln!("[modbus] {msg}");
            said = msg;
        }
    };
    loop {
        loop {
            match rx.try_recv() {
                Ok((d, p)) => {
                    if d != device {
                        link = None; // new address / unit / timeout: reconnect
                    }
                    device = d;
                    blocks = self::blocks(p);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        if link.is_none() {
            let target = device.addr.to_socket_addrs().ok().and_then(|mut a| a.next());
            match target.map(|t| TcpStream::connect_timeout(&t, device.timeout)) {
                Some(Ok(s)) => {
                    s.set_read_timeout(Some(device.timeout)).ok();
                    s.set_nodelay(true).ok();
                    say(format!("connected to {} ({} tags in {} requests)", device.addr, blocks.iter().map(|b| b.points.len()).sum::<usize>(), blocks.len()));
                    link = Some(s);
                }
                Some(Err(e)) => say(format!("can't connect to {}: {e} (tags keep their last values; retrying)", device.addr)),
                None => say(format!("bad device address {}", device.addr)),
            }
        }
        if let Some(s) = link.as_mut() {
            let mut changed = Vec::new();
            let mut i = 0;
            while i < blocks.len() {
                tid = tid.wrapping_add(1);
                let (fc, start, count) = (blocks[i].fc, blocks[i].start, blocks[i].count);
                match read(s, device.unit, tid, fc, start, count) {
                    Ok(Ok(words)) => {
                        for p in &blocks[i].points {
                            let value = words.get(usize::from(p.reg - start)..).and_then(|w| p.value(w));
                            if let Some(value) = value.filter(|v| last.get(&p.name) != Some(v)) {
                                last.insert(p.name.clone(), value);
                                changed.push((p.name.clone(), value));
                            }
                        }
                        i += 1;
                    }
                    Ok(Err(_)) if blocks[i].points.len() > 1 => {
                        // A hole in the device's map: read this block's tags one by one from now on.
                        let block = blocks.remove(i);
                        blocks.splice(i..i, block.points.into_iter().map(Block::single));
                    }
                    Ok(Err(code)) => {
                        let name = &blocks[i].points[0].name;
                        if refused.insert(name.clone()) {
                            eprintln!("[modbus] {} refused {name} (register {start}, exception {code})", device.addr);
                        }
                        i += 1;
                    }
                    Err(e) => {
                        say(format!("connection to {} lost: {e} (retrying)", device.addr));
                        link = None;
                        break;
                    }
                }
            }
            if !changed.is_empty() {
                host.set_tag_values(changed);
            }
        }
        // Fixed schedule: the time the reads took doesn't add up. Behind: start right away.
        next += device.scan;
        let now = Instant::now();
        if next > now {
            thread::sleep(next - now);
        } else {
            next = now;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn point(name: &str, fc: u8, reg: u16, format: i32) -> Point {
        Point { name: name.into(), fc, reg, format, factor: 1.0, offset: 0.0, invert: false }
    }

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
    fn groups_neighbouring_registers() {
        let b = blocks(vec![
            point("f2", 3, 2, 57),   // registers 2-3
            point("u0", 3, 0, 56),   // register 0
            point("u10", 3, 10, 56), // gap of 6: joins
            point("u40", 3, 40, 56), // gap too big: new block
            point("c0", 1, 0, 54),   // coils: their own block
            point("c1", 1, 1, 54),
        ]);
        let shape: Vec<(u8, u16, u16, usize)> = b.iter().map(|b| (b.fc, b.start, b.count, b.points.len())).collect();
        assert_eq!(shape, vec![(1, 0, 2, 2), (3, 0, 11, 3), (3, 40, 1, 1)]);
        // 1000 neighbouring registers: 8 requests of up to 125, not 1000.
        let many = blocks((0..1000).map(|r| point("x", 3, r, 56)).collect());
        assert_eq!(many.len(), 8);
        assert!(many.iter().all(|b| b.count <= 125));
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
            assert_eq!(req[6..], [1, 1, 0, 0, 0, 10]); // FC1, 10 coils
            c.write_all(&[req[0], req[1], 0, 0, 0, 5, 1, 1, 2, 0b0000_0101, 0b10]).unwrap();
            c.read_exact(&mut req).unwrap();
            c.write_all(&[req[0], req[1], 0, 0, 0, 3, 1, 0x83, 2]).unwrap(); // exception 2
        });
        let mut s = TcpStream::connect(addr).unwrap();
        let words = read(&mut s, 1, 7, 3, 6, 2).unwrap().unwrap();
        assert_eq!(decode(57, &words), Some(50.0));
        assert_eq!(read(&mut s, 1, 8, 1, 0, 10).unwrap(), Ok(vec![1, 0, 1, 0, 0, 0, 0, 0, 0, 1]));
        assert_eq!(read(&mut s, 1, 9, 3, 6, 2).unwrap(), Err(2));
    }
}
