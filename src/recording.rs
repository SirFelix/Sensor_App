//! A recording is a folder holding `session.json` plus one CSV per sensor:
//! `time_s,timestamp,raw,value`. Plain CSV keeps the data readable in Excel.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration as ChronoDuration, Local};
use serde::{Deserialize, Serialize};

use crate::config::SensorConfig;
use crate::sources::Sample;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionMeta {
    pub name: String,
    pub started: DateTime<Local>,
    pub ended: Option<DateTime<Local>>,
    pub duration_s: f64,
    pub sensors: Vec<SessionSensor>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionSensor {
    pub file: String,
    pub config: SensorConfig,
    pub samples: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecState {
    Recording,
    Paused,
}

pub struct Recorder {
    pub dir: PathBuf,
    pub state: RecState,
    meta: SessionMeta,
    /// App-clock time the recording started; CSV time_s is relative to it.
    t0: f64,
    started_wall: DateTime<Local>,
    active_since: Option<f64>,
    active_total: f64,
    writers: HashMap<u64, (usize, BufWriter<File>)>,
}

impl Recorder {
    pub fn start(root: &Path, name: &str, now: f64) -> Result<Self, String> {
        let started = Local::now();
        let clean = sanitize(name);
        let folder = if clean.is_empty() {
            started.format("%Y-%m-%d_%H-%M-%S").to_string()
        } else {
            format!("{}_{clean}", started.format("%Y-%m-%d_%H-%M-%S"))
        };
        let dir = root.join(folder);
        fs::create_dir_all(&dir).map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
        let rec = Self {
            dir,
            state: RecState::Recording,
            meta: SessionMeta {
                name: if name.trim().is_empty() { "Recording".into() } else { name.trim().into() },
                started,
                ended: None,
                duration_s: 0.0,
                sensors: Vec::new(),
            },
            t0: now,
            started_wall: started,
            active_since: Some(now),
            active_total: 0.0,
            writers: HashMap::new(),
        };
        rec.save_meta()?;
        Ok(rec)
    }

    /// Time actually spent recording, excluding pauses.
    pub fn recorded_secs(&self, now: f64) -> f64 {
        self.active_total + self.active_since.map_or(0.0, |s| now - s)
    }

    pub fn pause(&mut self, now: f64) {
        if let Some(s) = self.active_since.take() {
            self.active_total += now - s;
        }
        self.state = RecState::Paused;
        self.flush();
    }

    pub fn resume(&mut self, now: f64) {
        self.active_since.get_or_insert(now);
        self.state = RecState::Recording;
    }

    pub fn write(&mut self, cfg: &SensorConfig, samples: &[Sample]) -> Result<(), String> {
        if self.state != RecState::Recording || samples.is_empty() {
            return Ok(());
        }
        if !self.writers.contains_key(&cfg.id) {
            self.add_sensor(cfg)?;
        }
        let (idx, w) = self.writers.get_mut(&cfg.id).expect("writer just added");
        let samples: Vec<&Sample> = samples.iter().filter(|s| s.t >= self.t0).collect();
        for s in &samples {
            let rel = s.t - self.t0;
            let stamp = self.started_wall + ChronoDuration::microseconds((rel * 1e6) as i64);
            writeln!(
                w,
                "{rel:.6},{},{},{}",
                stamp.format("%Y-%m-%d %H:%M:%S%.3f"),
                s.raw,
                cfg.value_of(s.raw)
            )
            .map_err(|e| format!("Write failed: {e}"))?;
        }
        self.meta.sensors[*idx].samples += samples.len() as u64;
        Ok(())
    }

    fn add_sensor(&mut self, cfg: &SensorConfig) -> Result<(), String> {
        let idx = self.meta.sensors.len();
        let mut file = format!("{:02}_{}.csv", idx + 1, sanitize(&cfg.name));
        if file.ends_with("_.csv") {
            file = format!("{:02}_sensor.csv", idx + 1);
        }
        let f = File::create(self.dir.join(&file)).map_err(|e| format!("Cannot create {file}: {e}"))?;
        let mut w = BufWriter::new(f);
        let raw = match cfg.kind.raw_unit() {
            "raw" => "raw".to_string(),
            u => format!("raw ({u})"),
        };
        writeln!(w, "time_s,timestamp,{raw},value ({})", cfg.unit.replace(',', " ")).map_err(|e| e.to_string())?;
        self.meta.sensors.push(SessionSensor { file, config: cfg.clone(), samples: 0 });
        self.writers.insert(cfg.id, (idx, w));
        self.save_meta()
    }

    pub fn flush(&mut self) {
        for (_, w) in self.writers.values_mut() {
            let _ = w.flush();
        }
    }

    pub fn stop(mut self, now: f64) -> Result<PathBuf, String> {
        self.pause(now);
        self.meta.ended = Some(Local::now());
        self.meta.duration_s = now - self.t0;
        self.save_meta()?;
        Ok(self.dir)
    }

    fn save_meta(&self) -> Result<(), String> {
        let json = serde_json::to_string_pretty(&self.meta).map_err(|e| e.to_string())?;
        fs::write(self.dir.join("session.json"), json).map_err(|e| format!("Cannot write session.json: {e}"))
    }
}

pub fn sanitize(name: &str) -> String {
    name.trim()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

pub struct RecordingEntry {
    pub dir: PathBuf,
    pub meta: SessionMeta,
}

/// Newest first.
pub fn list(root: &Path) -> Vec<RecordingEntry> {
    let mut out: Vec<RecordingEntry> = fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let dir = e.path();
            let text = fs::read_to_string(dir.join("session.json")).ok()?;
            let meta = serde_json::from_str(&text).ok()?;
            Some(RecordingEntry { dir, meta })
        })
        .collect();
    out.sort_by_key(|e| std::cmp::Reverse(e.meta.started));
    out
}

pub struct ReviewChannel {
    pub name: String,
    pub unit: String,
    pub config: Option<SensorConfig>,
    pub t: Vec<f64>,
    pub v: Vec<f64>,
}

pub struct ReviewSession {
    pub title: String,
    pub path: PathBuf,
    pub channels: Vec<ReviewChannel>,
}

pub fn load_session(dir: &Path) -> Result<ReviewSession, String> {
    let text = fs::read_to_string(dir.join("session.json")).map_err(|e| format!("No session.json in {}: {e}", dir.display()))?;
    let meta: SessionMeta = serde_json::from_str(&text).map_err(|e| format!("Bad session.json: {e}"))?;
    let mut channels = Vec::new();
    for s in &meta.sensors {
        let (t, cols) = read_csv(&dir.join(&s.file), &[0, 3])?;
        channels.push(ReviewChannel {
            name: s.config.name.clone(),
            unit: s.config.unit.clone(),
            config: Some(s.config.clone()),
            t,
            v: cols.into_iter().next().unwrap_or_default(),
        });
    }
    Ok(ReviewSession {
        title: format!("{} — {}", meta.name, meta.started.format("%Y-%m-%d %H:%M:%S")),
        path: dir.to_path_buf(),
        channels,
    })
}

/// Any CSV whose first column is time: every other numeric column becomes a tile.
pub fn load_generic_csv(path: &Path) -> Result<ReviewSession, String> {
    let f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut lines = BufReader::new(f).lines();
    let header = lines.next().ok_or("Empty file")?.map_err(|e| e.to_string())?;
    let names: Vec<String> = header.split(',').map(|s| s.trim().trim_matches('"').to_string()).collect();
    let first_row = lines.next().ok_or("No data rows")?.map_err(|e| e.to_string())?;
    let numeric: Vec<usize> = first_row
        .split(',')
        .enumerate()
        .skip(1)
        .filter(|(_, v)| v.trim().parse::<f64>().is_ok())
        .map(|(i, _)| i)
        .collect();
    if numeric.is_empty() {
        return Err("No numeric columns after the time column".into());
    }
    let mut cols = vec![0usize];
    cols.extend(&numeric);
    let (t, data) = read_csv(path, &cols)?;
    let channels = numeric
        .iter()
        .zip(data)
        .map(|(&i, v)| {
            let full = names.get(i).cloned().unwrap_or_else(|| format!("column {i}"));
            let (name, unit) = split_unit(&full);
            ReviewChannel { name, unit, config: None, t: t.clone(), v }
        })
        .collect();
    Ok(ReviewSession {
        title: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        path: path.to_path_buf(),
        channels,
    })
}

fn split_unit(header: &str) -> (String, String) {
    match (header.rfind('('), header.ends_with(')')) {
        (Some(i), true) => (header[..i].trim().to_string(), header[i + 1..header.len() - 1].to_string()),
        _ => (header.to_string(), String::new()),
    }
}

/// Reads column `cols[0]` as time (seconds, or a timestamp) and the rest as values.
fn read_csv(path: &Path, cols: &[usize]) -> Result<(Vec<f64>, Vec<Vec<f64>>), String> {
    let f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut t = Vec::new();
    let mut vals = vec![Vec::new(); cols.len() - 1];
    let mut t_origin: Option<f64> = None;
    for line in BufReader::new(f).lines().skip(1) {
        let line = line.map_err(|e| e.to_string())?;
        let fields: Vec<&str> = line.split(',').collect();
        let Some(ts) = fields.get(cols[0]).and_then(|s| parse_time(s.trim())) else { continue };
        let row: Option<Vec<f64>> = cols[1..]
            .iter()
            .map(|&c| fields.get(c).and_then(|s| s.trim().parse::<f64>().ok()))
            .collect();
        let Some(row) = row else { continue };
        let origin = *t_origin.get_or_insert(if ts > 1e8 { ts } else { 0.0 });
        t.push(ts - origin);
        for (dst, v) in vals.iter_mut().zip(row) {
            dst.push(v);
        }
    }
    Ok((t, vals))
}

fn parse_time(s: &str) -> Option<f64> {
    if let Ok(v) = s.parse::<f64>() {
        return Some(v);
    }
    for fmt in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f", "%m/%d/%Y %H:%M:%S%.f"] {
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Some(dt.and_utc().timestamp_micros() as f64 / 1e6);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SourceKind;

    #[test]
    fn record_pause_resume_stop_and_reload() {
        let root = std::env::temp_dir().join(format!("sensor_plotter_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mut cfg = SensorConfig { id: 7, name: "Line pressure".into(), kind: SourceKind::Simulated, ..Default::default() };
        cfg.scaling.gain = 2.0;

        let mut rec = Recorder::start(&root, "bench test", 10.0).unwrap();
        rec.write(&cfg, &[Sample { t: 10.0, raw: 1.0 }, Sample { t: 10.5, raw: 2.0 }]).unwrap();
        rec.pause(11.0);
        rec.write(&cfg, &[Sample { t: 11.5, raw: 99.0 }]).unwrap();
        rec.resume(12.0);
        rec.write(&cfg, &[Sample { t: 12.0, raw: 3.0 }]).unwrap();
        assert!((rec.recorded_secs(13.0) - 2.0).abs() < 1e-9);
        let dir = rec.stop(13.0).unwrap();

        let entries = list(&root);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].meta.sensors[0].samples, 3);

        let session = load_session(&dir).unwrap();
        let ch = &session.channels[0];
        assert_eq!(ch.name, "Line pressure");
        assert_eq!(ch.t, vec![0.0, 0.5, 2.0]);
        assert_eq!(ch.v, vec![2.0, 4.0, 6.0]);

        let generic = load_generic_csv(&dir.join(&entries[0].meta.sensors[0].file)).unwrap();
        assert_eq!(generic.channels.len(), 2);
        assert_eq!(generic.channels[1].unit, "psi");
        let _ = fs::remove_dir_all(&root);
    }
}
