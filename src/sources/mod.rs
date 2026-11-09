pub mod modbus;
pub mod nidaq;
pub mod serial;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::config::{SensorConfig, SourceKind, Waveform};

#[derive(Clone, Copy, Debug)]
pub struct Sample {
    /// Seconds since the app clock epoch.
    pub t: f64,
    pub raw: f64,
}

pub enum SourceEvent {
    Connected(String),
    Samples(Vec<Sample>),
    /// Recoverable problem; the source keeps trying.
    Warning(String),
}

#[derive(Clone)]
pub struct Clock {
    epoch: Instant,
}

impl Clock {
    pub fn new() -> Self {
        Self { epoch: Instant::now() }
    }

    pub fn now(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }
}

/// Thread context shared by every source implementation.
pub struct Link {
    pub tx: Sender<SourceEvent>,
    pub stop: Arc<AtomicBool>,
    pub clock: Clock,
}

impl Link {
    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    pub fn send(&self, ev: SourceEvent) {
        let _ = self.tx.send(ev);
    }

    /// Sleeps in small slices so Disconnect stays responsive.
    pub fn sleep(&self, d: Duration) {
        let end = Instant::now() + d;
        while !self.stopped() {
            let now = Instant::now();
            if now >= end {
                break;
            }
            thread::sleep((end - now).min(Duration::from_millis(25)));
        }
    }

    /// Runs `attempt` until stopped, waiting between failures so a dropped
    /// cable or chassis reconnects on its own.
    pub fn retry_loop(&self, mut attempt: impl FnMut(&Link) -> Result<(), String>) {
        while !self.stopped() {
            if let Err(e) = attempt(self) {
                if self.stopped() {
                    break;
                }
                self.send(SourceEvent::Warning(format!("{e} — retrying in 2 s")));
                self.sleep(Duration::from_secs(2));
            }
        }
    }
}

pub struct SourceHandle {
    pub rx: Receiver<SourceEvent>,
    stop: Arc<AtomicBool>,
}

impl Drop for SourceHandle {
    fn drop(&mut self) {
        // The thread is detached: a hung driver call must not freeze the UI.
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub fn spawn(cfg: &SensorConfig, clock: &Clock) -> SourceHandle {
    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let link = Link { tx, stop: stop.clone(), clock: clock.clone() };
    let cfg = cfg.clone();
    thread::Builder::new()
        .name(format!("source-{}", cfg.name))
        .spawn(move || match cfg.kind {
            SourceKind::NiVoltage | SourceKind::NiCurrent => nidaq::run(&cfg, &link),
            SourceKind::SerialAscii => serial::run_ascii(&cfg, &link),
            SourceKind::ModbusRtu => serial::run_modbus_rtu(&cfg, &link),
            SourceKind::ModbusTcp => modbus::run_tcp(&cfg, &link),
            SourceKind::Simulated => run_simulated(&cfg, &link),
        })
        .expect("spawn source thread");
    SourceHandle { rx, stop }
}

fn run_simulated(cfg: &SensorConfig, link: &Link) {
    let s = &cfg.sim;
    let rate = s.sample_rate_hz.clamp(0.1, 10_000.0);
    let dt = 1.0 / rate;
    let start = link.clock.now();
    let mut n: u64 = 0;
    let mut rng = 0x2545_F491_4F6C_DD1Du64 ^ cfg.id.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    link.send(SourceEvent::Connected(format!("Simulated {:?}", s.waveform)));
    while !link.stopped() {
        let now = link.clock.now();
        let mut batch = Vec::new();
        while start + n as f64 * dt <= now {
            let t = start + n as f64 * dt;
            let phase = ((t - start) * s.frequency_hz).fract();
            let shape = match s.waveform {
                Waveform::Sine => (phase * std::f64::consts::TAU).sin(),
                Waveform::Square => {
                    if phase < 0.5 {
                        1.0
                    } else {
                        -1.0
                    }
                }
                Waveform::Triangle => 1.0 - 4.0 * (phase - 0.5).abs(),
                Waveform::Ramp => 2.0 * phase - 1.0,
            };
            batch.push(Sample { t, raw: s.offset + s.amplitude * shape + s.noise * gaussian(&mut rng) });
            n += 1;
        }
        if !batch.is_empty() {
            link.send(SourceEvent::Samples(batch));
        }
        link.sleep(Duration::from_millis(20));
    }
}

fn gaussian(state: &mut u64) -> f64 {
    let mut uniform = || {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        ((*state >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    };
    let (u1, u2) = (uniform(), uniform());
    (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
}

/// Expands \r \n \t \\ and \xNN escapes typed into poll command fields.
pub fn unescape(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'r' => out.push(b'\r'),
                b'n' => out.push(b'\n'),
                b't' => out.push(b'\t'),
                b'\\' => out.push(b'\\'),
                b'x' => {
                    let hex = bytes.get(i + 2..i + 4).and_then(|h| std::str::from_utf8(h).ok());
                    match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                        Some(b) => {
                            out.push(b);
                            i += 4;
                            continue;
                        }
                        None => out.extend_from_slice(b"\\x"),
                    }
                }
                other => {
                    out.push(b'\\');
                    out.push(other);
                }
            }
            i += 2;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::unescape;

    #[test]
    fn unescape_handles_common_sequences() {
        assert_eq!(unescape(r"P?\r\n"), b"P?\r\n");
        assert_eq!(unescape(r"\x02R\x03"), [0x02, b'R', 0x03]);
        assert_eq!(unescape(r"a\qb"), b"a\\qb");
        assert_eq!(unescape(r"\x4"), b"\\x4");
    }
}
