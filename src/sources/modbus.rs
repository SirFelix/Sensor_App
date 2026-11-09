use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use super::{Link, Sample, SourceEvent};
use crate::config::{ModbusSettings, RegisterType, SensorConfig};

pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc ^= b as u16;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xA001 } else { crc >> 1 };
        }
    }
    crc
}

fn pdu(m: &ModbusSettings) -> [u8; 5] {
    let [rh, rl] = m.register.to_be_bytes();
    let [ch, cl] = m.data_type.register_count().to_be_bytes();
    [m.function.code(), rh, rl, ch, cl]
}

pub fn rtu_request(m: &ModbusSettings) -> Vec<u8> {
    let mut f = vec![m.slave_id];
    f.extend_from_slice(&pdu(m));
    let crc = crc16(&f);
    f.extend_from_slice(&crc.to_le_bytes());
    f
}

pub fn rtu_response_len(m: &ModbusSettings) -> usize {
    5 + 2 * m.data_type.register_count() as usize
}

/// Validates an RTU reply and returns its register payload bytes.
pub fn parse_rtu_response<'a>(m: &ModbusSettings, frame: &'a [u8]) -> Result<&'a [u8], String> {
    if frame.len() >= 5 && frame[1] & 0x80 != 0 {
        return Err(format!("Modbus exception {} from slave {}", frame[2], frame[0]));
    }
    let need = rtu_response_len(m);
    if frame.len() < need {
        return Err(format!("Short reply ({} of {need} bytes)", frame.len()));
    }
    let frame = &frame[..need];
    let crc = u16::from_le_bytes([frame[need - 2], frame[need - 1]]);
    if crc != crc16(&frame[..need - 2]) {
        return Err("CRC mismatch".into());
    }
    if frame[0] != m.slave_id || frame[1] != m.function.code() {
        return Err(format!("Unexpected reply from slave {} fc {}", frame[0], frame[1]));
    }
    Ok(&frame[3..need - 2])
}

pub fn decode(m: &ModbusSettings, data: &[u8]) -> Result<f64, String> {
    let words: Vec<u16> = data.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
    let pair = || -> Result<u32, String> {
        let (hi, lo) = match words.as_slice() {
            [a, b, ..] if m.word_swap => (*b, *a),
            [a, b, ..] => (*a, *b),
            _ => return Err("Reply has too few registers".into()),
        };
        Ok(((hi as u32) << 16) | lo as u32)
    };
    let first = *words.first().ok_or("Empty reply")?;
    Ok(match m.data_type {
        RegisterType::U16 => first as f64,
        RegisterType::I16 => first as i16 as f64,
        RegisterType::U32 => pair()? as f64,
        RegisterType::I32 => pair()? as i32 as f64,
        RegisterType::F32 => f32::from_bits(pair()?) as f64,
    })
}

pub fn run_tcp(cfg: &SensorConfig, link: &Link) {
    link.retry_loop(|link| poll_tcp(cfg, link));
}

fn poll_tcp(cfg: &SensorConfig, link: &Link) -> Result<(), String> {
    let m = &cfg.modbus;
    let addr = (m.host.as_str(), m.tcp_port)
        .to_socket_addrs()
        .map_err(|e| format!("{}:{}: {e}", m.host, m.tcp_port))?
        .next()
        .ok_or("Host did not resolve")?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).map_err(|e| format!("{addr}: {e}"))?;
    let timeout = Some(Duration::from_millis(cfg.serial.timeout_ms.max(100) * 5));
    stream.set_read_timeout(timeout).ok();
    stream.set_write_timeout(timeout).ok();
    stream.set_nodelay(true).ok();
    link.send(SourceEvent::Connected(format!("Modbus TCP {addr}")));

    let interval = Duration::from_millis(m.poll_interval_ms.max(1));
    let mut txn: u16 = 0;
    let mut next = Instant::now();
    while !link.stopped() {
        txn = txn.wrapping_add(1);
        let body = pdu(m);
        let mut req = Vec::with_capacity(12);
        req.extend_from_slice(&txn.to_be_bytes());
        req.extend_from_slice(&[0, 0]);
        req.extend_from_slice(&((body.len() + 1) as u16).to_be_bytes());
        req.push(m.slave_id);
        req.extend_from_slice(&body);
        stream.write_all(&req).map_err(|e| format!("Write failed: {e}"))?;

        let mut head = [0u8; 7];
        stream.read_exact(&mut head).map_err(|e| format!("No reply: {e}"))?;
        let len = u16::from_be_bytes([head[4], head[5]]) as usize;
        if !(2..=260).contains(&len) {
            return Err(format!("Bad MBAP length {len}"));
        }
        let mut rest = vec![0u8; len - 1];
        stream.read_exact(&mut rest).map_err(|e| format!("Short reply: {e}"))?;
        let t = link.clock.now();
        if u16::from_be_bytes([head[0], head[1]]) != txn {
            continue;
        }
        if rest[0] & 0x80 != 0 {
            link.send(SourceEvent::Warning(format!("Modbus exception {}", rest.get(1).copied().unwrap_or(0))));
        } else if rest.len() >= 2 {
            match decode(m, &rest[2..]) {
                Ok(raw) => link.send(SourceEvent::Samples(vec![Sample { t, raw }])),
                Err(e) => link.send(SourceEvent::Warning(e)),
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
    use crate::config::ModbusFunction;

    fn keller() -> ModbusSettings {
        ModbusSettings {
            slave_id: 1,
            function: ModbusFunction::Holding,
            register: 2,
            data_type: RegisterType::F32,
            ..Default::default()
        }
    }

    #[test]
    fn rtu_request_matches_known_frame() {
        // 01 03 00 02 00 02 -> CRC 65 CB
        assert_eq!(rtu_request(&keller()), vec![0x01, 0x03, 0x00, 0x02, 0x00, 0x02, 0x65, 0xCB]);
    }

    #[test]
    fn decodes_float_reply() {
        let m = keller();
        let mut frame = vec![0x01, 0x03, 0x04];
        frame.extend_from_slice(&1.5f32.to_be_bytes());
        let crc = crc16(&frame);
        frame.extend_from_slice(&crc.to_le_bytes());
        let data = parse_rtu_response(&m, &frame).unwrap();
        assert_eq!(decode(&m, data).unwrap(), 1.5);
    }

    #[test]
    fn word_swap_and_signed_types() {
        let mut m = keller();
        m.word_swap = true;
        let b = 2.25f32.to_be_bytes();
        assert_eq!(decode(&m, &[b[2], b[3], b[0], b[1]]).unwrap(), 2.25);
        m.data_type = RegisterType::I16;
        assert_eq!(decode(&m, &[0xFF, 0xFE]).unwrap(), -2.0);
    }

    #[test]
    fn tcp_source_polls_a_live_server() {
        use crate::config::SourceKind;
        use crate::sources::{spawn, Clock};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut req = [0u8; 12];
            while s.read_exact(&mut req).is_ok() {
                assert_eq!(&req[6..12], &[1, 3, 0, 2, 0, 2]);
                let mut reply = vec![req[0], req[1], 0, 0, 0, 7, 1, 3, 4];
                reply.extend_from_slice(&2.5f32.to_be_bytes());
                if s.write_all(&reply).is_err() {
                    break;
                }
            }
        });

        let cfg = SensorConfig {
            kind: SourceKind::ModbusTcp,
            modbus: ModbusSettings { host: "127.0.0.1".into(), tcp_port: port, ..keller() },
            ..Default::default()
        };
        let handle = spawn(&cfg, &Clock::new());
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut got = None;
        while got.is_none() && Instant::now() < deadline {
            if let Ok(SourceEvent::Samples(v)) = handle.rx.recv_timeout(Duration::from_millis(100)) {
                got = v.first().map(|s| s.raw);
            }
        }
        assert_eq!(got, Some(2.5));
    }

    #[test]
    fn rejects_bad_crc_and_exceptions() {
        let m = keller();
        assert!(parse_rtu_response(&m, &[0x01, 0x83, 0x02, 0xC0, 0xF1]).unwrap_err().contains("exception"));
        assert!(parse_rtu_response(&m, &[1, 3, 4, 0, 0, 0, 0, 0, 0]).unwrap_err().contains("CRC"));
    }
}
