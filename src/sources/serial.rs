use std::io::{ErrorKind, Read, Write};
use std::time::{Duration, Instant};

use serialport::{DataBits, SerialPort, StopBits};

use super::{modbus, unescape, Link, Sample, SourceEvent};
use crate::config::{Parity, SensorConfig, SerialSettings};

pub fn list_ports() -> Vec<String> {
    let mut ports: Vec<String> = serialport::available_ports()
        .unwrap_or_default()
        .into_iter()
        .map(|p| p.port_name)
        .collect();
    ports.sort_by_key(|p| (p.len(), p.clone()));
    ports
}

fn open(s: &SerialSettings) -> Result<Box<dyn SerialPort>, String> {
    if s.port.trim().is_empty() {
        return Err("No COM port selected".into());
    }
    serialport::new(s.port.trim(), s.baud)
        .data_bits(if s.data_bits == 7 { DataBits::Seven } else { DataBits::Eight })
        .parity(match s.parity {
            Parity::None => serialport::Parity::None,
            Parity::Even => serialport::Parity::Even,
            Parity::Odd => serialport::Parity::Odd,
        })
        .stop_bits(if s.stop_bits == 2 { StopBits::Two } else { StopBits::One })
        .timeout(Duration::from_millis(20))
        .open()
        .map_err(|e| format!("{}: {e}", s.port))
}

pub fn run_ascii(cfg: &SensorConfig, link: &Link) {
    link.retry_loop(|link| read_ascii(cfg, link));
}

fn read_ascii(cfg: &SensorConfig, link: &Link) -> Result<(), String> {
    let mut port = open(&cfg.serial)?;
    let a = &cfg.ascii;
    let poll = unescape(&a.poll_command);
    let interval = Duration::from_millis(a.poll_interval_ms.max(1));
    let silence_limit = Duration::from_millis(cfg.serial.timeout_ms.max(100) * 10).max(Duration::from_secs(3));
    link.send(SourceEvent::Connected(format!("{} @ {}", cfg.serial.port, cfg.serial.baud)));

    let mut next_poll = Instant::now();
    let mut last_rx = Instant::now();
    let mut line = Vec::<u8>::new();
    let mut chunk = [0u8; 512];
    let mut warned = false;
    while !link.stopped() {
        if !poll.is_empty() && Instant::now() >= next_poll {
            port.write_all(&poll).map_err(|e| format!("Write failed: {e}"))?;
            next_poll += interval;
            if next_poll < Instant::now() {
                next_poll = Instant::now() + interval;
            }
        }
        match port.read(&mut chunk) {
            Ok(0) => {}
            Ok(n) => {
                last_rx = Instant::now();
                warned = false;
                let t = link.clock.now();
                let mut batch = Vec::new();
                for &b in &chunk[..n] {
                    if b == b'\n' || b == b'\r' {
                        if let Some(raw) = parse_line(&String::from_utf8_lossy(&line), &a.delimiter, a.field_index) {
                            batch.push(Sample { t, raw });
                        }
                        line.clear();
                    } else if line.len() < 4096 {
                        line.push(b);
                    }
                }
                if !batch.is_empty() {
                    link.send(SourceEvent::Samples(batch));
                }
            }
            Err(e) if e.kind() == ErrorKind::TimedOut => {}
            Err(e) => return Err(format!("Read failed: {e}")),
        }
        if !warned && last_rx.elapsed() > silence_limit {
            warned = true;
            link.send(SourceEvent::Warning(format!(
                "No data for {:.0} s — check wiring, baud, or poll command",
                silence_limit.as_secs_f64()
            )));
        }
    }
    Ok(())
}

/// Picks one field from a line and pulls the first number out of it, so
/// replies like `P=+0123.45 psi` or `12.3,45.6,OK` both work.
pub fn parse_line(line: &str, delimiter: &str, field: usize) -> Option<f64> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let part = if delimiter.is_empty() {
        line.split_whitespace().nth(field)?
    } else {
        line.split(delimiter).nth(field)?
    };
    extract_number(part)
}

pub fn extract_number(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let starts = b[i].is_ascii_digit()
            || ((b[i] == b'-' || b[i] == b'+' || b[i] == b'.')
                && b.get(i + 1).is_some_and(|c| c.is_ascii_digit() || *c == b'.'));
        if starts {
            let mut end = i + 1;
            let mut seen_e = false;
            while end < b.len() {
                let c = b[end];
                let ok = c.is_ascii_digit()
                    || c == b'.'
                    || (!seen_e && (c == b'e' || c == b'E') && b.get(end + 1).is_some_and(|n| n.is_ascii_digit() || *n == b'-' || *n == b'+'))
                    || ((c == b'-' || c == b'+') && matches!(b[end - 1], b'e' | b'E'));
                if !ok {
                    break;
                }
                seen_e |= c == b'e' || c == b'E';
                end += 1;
            }
            if let Ok(v) = s[i..end].parse::<f64>() {
                return Some(v);
            }
        }
        i += 1;
    }
    None
}

pub fn run_modbus_rtu(cfg: &SensorConfig, link: &Link) {
    link.retry_loop(|link| poll_rtu(cfg, link));
}

fn poll_rtu(cfg: &SensorConfig, link: &Link) -> Result<(), String> {
    let mut port = open(&cfg.serial)?;
    let m = &cfg.modbus;
    let req = modbus::rtu_request(m);
    let want = modbus::rtu_response_len(m);
    let timeout = Duration::from_millis(cfg.serial.timeout_ms.max(20));
    let interval = Duration::from_millis(m.poll_interval_ms.max(1));
    link.send(SourceEvent::Connected(format!(
        "{} @ {} · slave {}",
        cfg.serial.port, cfg.serial.baud, m.slave_id
    )));

    let mut next = Instant::now();
    let mut failures = 0u32;
    let mut buf = vec![0u8; 256];
    while !link.stopped() {
        let _ = port.clear(serialport::ClearBuffer::Input);
        port.write_all(&req).map_err(|e| format!("Write failed: {e}"))?;
        let deadline = Instant::now() + timeout;
        let mut got = 0;
        while got < want && Instant::now() < deadline {
            match port.read(&mut buf[got..]) {
                Ok(n) => {
                    got += n;
                    // An exception reply is only 5 bytes.
                    if got >= 5 && buf[1] & 0x80 != 0 {
                        break;
                    }
                }
                Err(e) if e.kind() == ErrorKind::TimedOut => {}
                Err(e) => return Err(format!("Read failed: {e}")),
            }
        }
        let t = link.clock.now();
        match modbus::parse_rtu_response(m, &buf[..got]).and_then(|d| modbus::decode(m, d)) {
            Ok(raw) => {
                failures = 0;
                link.send(SourceEvent::Samples(vec![Sample { t, raw }]));
            }
            Err(e) => {
                failures += 1;
                if failures == 1 || failures.is_multiple_of(20) {
                    let e = if got == 0 { "No reply (timeout)".to_string() } else { e };
                    link.send(SourceEvent::Warning(format!("{e} ({failures} in a row)")));
                }
            }
        }
        next += interval;
        let now = Instant::now();
        if next > now {
            link.sleep(next - now);
        } else {
            next = now;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fields_and_embedded_numbers() {
        assert_eq!(parse_line("12.5,33.1,OK", ",", 1), Some(33.1));
        assert_eq!(parse_line("P=+0123.45 psi", "", 0), Some(123.45));
        assert_eq!(parse_line("  -4.2e-3  V ", "", 0), Some(-0.0042));
        assert_eq!(parse_line("T 21.0 P 1500", "", 3), Some(1500.0));
        assert_eq!(parse_line("ERR", ",", 0), None);
        assert_eq!(parse_line("1,2", ",", 5), None);
        assert_eq!(extract_number("value: .5"), Some(0.5));
        assert_eq!(extract_number("no digits"), None);
    }
}
