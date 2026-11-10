use serde::{Deserialize, Serialize};

use crate::scaling::{Scaling, ScalingMode};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceKind {
    NiVoltage,
    NiCurrent,
    SerialAscii,
    ModbusRtu,
    ModbusTcp,
    Simulated,
}

impl SourceKind {
    pub const ALL: [SourceKind; 6] = [
        SourceKind::NiVoltage,
        SourceKind::NiCurrent,
        SourceKind::SerialAscii,
        SourceKind::ModbusRtu,
        SourceKind::ModbusTcp,
        SourceKind::Simulated,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SourceKind::NiVoltage => "NI-DAQmx analog voltage (0-10 V, ±10 V…)",
            SourceKind::NiCurrent => "NI-DAQmx analog current (4-20 mA)",
            SourceKind::SerialAscii => "Serial / RS232 / RS485 ASCII",
            SourceKind::ModbusRtu => "Modbus RTU (RS485 / serial)",
            SourceKind::ModbusTcp => "Modbus TCP (Ethernet)",
            SourceKind::Simulated => "Simulated signal",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            SourceKind::NiVoltage => "NI voltage",
            SourceKind::NiCurrent => "NI current",
            SourceKind::SerialAscii => "Serial ASCII",
            SourceKind::ModbusRtu => "Modbus RTU",
            SourceKind::ModbusTcp => "Modbus TCP",
            SourceKind::Simulated => "Simulated",
        }
    }

    pub fn raw_unit(self) -> &'static str {
        match self {
            SourceKind::NiVoltage => "V",
            SourceKind::NiCurrent => "mA",
            _ => "raw",
        }
    }

    pub fn is_ni(self) -> bool {
        matches!(self, SourceKind::NiVoltage | SourceKind::NiCurrent)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalConfig {
    Default,
    Rse,
    Nrse,
    Differential,
    PseudoDifferential,
}

impl TerminalConfig {
    pub const ALL: [TerminalConfig; 5] = [
        TerminalConfig::Default,
        TerminalConfig::Rse,
        TerminalConfig::Nrse,
        TerminalConfig::Differential,
        TerminalConfig::PseudoDifferential,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TerminalConfig::Default => "Device default",
            TerminalConfig::Rse => "RSE (referenced single-ended)",
            TerminalConfig::Nrse => "NRSE",
            TerminalConfig::Differential => "Differential",
            TerminalConfig::PseudoDifferential => "Pseudo-differential",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShuntLocation {
    Default,
    Internal,
    External,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NiSettings {
    pub channel: String,
    pub terminal: TerminalConfig,
    /// Input range in the raw unit (V for voltage, mA for current).
    pub min: f64,
    pub max: f64,
    pub sample_rate_hz: f64,
    pub shunt: ShuntLocation,
    pub ext_shunt_ohms: f64,
}

impl Default for NiSettings {
    fn default() -> Self {
        Self {
            channel: String::new(),
            terminal: TerminalConfig::Default,
            min: 0.0,
            max: 10.0,
            sample_rate_hz: 100.0,
            shunt: ShuntLocation::Default,
            ext_shunt_ohms: 249.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Parity {
    None,
    Even,
    Odd,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SerialSettings {
    pub port: String,
    pub baud: u32,
    pub data_bits: u8,
    pub parity: Parity,
    pub stop_bits: u8,
    pub timeout_ms: u64,
}

impl Default for SerialSettings {
    fn default() -> Self {
        Self {
            port: String::new(),
            baud: 9600,
            data_bits: 8,
            parity: Parity::None,
            stop_bits: 1,
            timeout_ms: 200,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AsciiSettings {
    /// Sent every poll interval; empty means the device streams on its own.
    /// Supports \r, \n, \t and \xNN escapes.
    pub poll_command: String,
    pub poll_interval_ms: u64,
    /// Field separator; empty splits on whitespace.
    pub delimiter: String,
    /// Zero-based field holding the reading.
    pub field_index: usize,
}

impl Default for AsciiSettings {
    fn default() -> Self {
        Self {
            poll_command: String::new(),
            poll_interval_ms: 100,
            delimiter: ",".into(),
            field_index: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModbusFunction {
    Holding,
    Input,
}

impl ModbusFunction {
    pub fn code(self) -> u8 {
        match self {
            ModbusFunction::Holding => 0x03,
            ModbusFunction::Input => 0x04,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RegisterType {
    U16,
    I16,
    U32,
    I32,
    F32,
}

impl RegisterType {
    pub const ALL: [RegisterType; 5] =
        [RegisterType::U16, RegisterType::I16, RegisterType::U32, RegisterType::I32, RegisterType::F32];

    pub fn label(self) -> &'static str {
        match self {
            RegisterType::U16 => "uint16",
            RegisterType::I16 => "int16",
            RegisterType::U32 => "uint32 (2 regs)",
            RegisterType::I32 => "int32 (2 regs)",
            RegisterType::F32 => "float32 (2 regs)",
        }
    }

    pub fn register_count(self) -> u16 {
        match self {
            RegisterType::U16 | RegisterType::I16 => 1,
            _ => 2,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModbusSettings {
    pub slave_id: u8,
    pub function: ModbusFunction,
    pub register: u16,
    pub data_type: RegisterType,
    /// Low word first for 32-bit values.
    pub word_swap: bool,
    pub poll_interval_ms: u64,
    pub host: String,
    pub tcp_port: u16,
}

impl Default for ModbusSettings {
    fn default() -> Self {
        Self {
            slave_id: 1,
            function: ModbusFunction::Holding,
            register: 0,
            data_type: RegisterType::F32,
            word_swap: false,
            poll_interval_ms: 50,
            host: "127.0.0.1".into(),
            tcp_port: 502,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Waveform {
    Sine,
    Square,
    Triangle,
    Ramp,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SimSettings {
    pub waveform: Waveform,
    pub amplitude: f64,
    pub offset: f64,
    pub frequency_hz: f64,
    pub noise: f64,
    pub sample_rate_hz: f64,
}

impl Default for SimSettings {
    fn default() -> Self {
        Self {
            waveform: Waveform::Sine,
            amplitude: 4.0,
            offset: 5.0,
            frequency_hz: 0.2,
            noise: 0.05,
            sample_rate_hz: 50.0,
        }
    }
}

/// How the live graph moves once it is full.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScrollMode {
    /// Slides continuously with the newest reading at the right edge.
    #[default]
    Continuous,
    /// Holds still while new readings fill the right side, then jumps left by
    /// `step_points` to open a fresh gap.
    Step,
}

impl ScrollMode {
    pub fn label(self) -> &'static str {
        match self {
            ScrollMode::Continuous => "Slide",
            ScrollMode::Step => "Step",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SensorConfig {
    pub id: u64,
    pub name: String,
    pub kind: SourceKind,
    pub ni: NiSettings,
    pub serial: SerialSettings,
    pub ascii: AsciiSettings,
    pub modbus: ModbusSettings,
    pub sim: SimSettings,

    pub unit: String,
    pub scaling: Scaling,
    /// Added after scaling; the tile's Tare button adjusts this.
    pub offset: f64,
    pub range_min: f64,
    pub range_max: f64,
    pub clamp_to_range: bool,
    pub range_as_axis: bool,

    /// Points shown before the graph starts scrolling.
    pub visible_points: usize,
    pub scroll_mode: ScrollMode,
    /// Points the graph jumps by in `ScrollMode::Step`.
    pub step_points: usize,
    pub decimals: usize,

    pub show_raw: bool,
    /// Moving-average trace and readout over the last `avg_points` samples.
    pub show_avg: bool,
    pub avg_points: usize,
}

impl Default for SensorConfig {
    fn default() -> Self {
        Self {
            id: 0,
            name: "New sensor".into(),
            kind: SourceKind::Simulated,
            ni: NiSettings::default(),
            serial: SerialSettings::default(),
            ascii: AsciiSettings::default(),
            modbus: ModbusSettings::default(),
            sim: SimSettings::default(),
            unit: "psi".into(),
            scaling: Scaling::default(),
            offset: 0.0,
            range_min: 0.0,
            range_max: 10.0,
            clamp_to_range: false,
            range_as_axis: false,
            visible_points: 500,
            scroll_mode: ScrollMode::Continuous,
            step_points: 250,
            decimals: 2,
            show_raw: true,
            show_avg: false,
            avg_points: 20,
        }
    }
}

impl SensorConfig {
    pub fn value_of(&self, raw: f64) -> f64 {
        let v = self.scaling.apply(raw) + self.offset;
        if self.clamp_to_range && self.range_min < self.range_max {
            v.clamp(self.range_min, self.range_max)
        } else {
            v
        }
    }

    pub fn in_range(&self, value: f64) -> bool {
        self.range_min >= self.range_max || (value >= self.range_min && value <= self.range_max)
    }

    /// Anything that requires reopening the hardware connection when changed.
    pub fn connection_key(&self) -> String {
        let part = match self.kind {
            SourceKind::NiVoltage | SourceKind::NiCurrent => serde_json::to_string(&self.ni),
            SourceKind::SerialAscii => serde_json::to_string(&(&self.serial, &self.ascii)),
            SourceKind::ModbusRtu => serde_json::to_string(&(&self.serial, &self.modbus)),
            SourceKind::ModbusTcp => serde_json::to_string(&self.modbus),
            SourceKind::Simulated => serde_json::to_string(&self.sim),
        };
        format!("{:?}{}", self.kind, part.unwrap_or_default())
    }

    pub fn connection_label(&self) -> String {
        match self.kind {
            SourceKind::NiVoltage | SourceKind::NiCurrent => format!(
                "{} · {} Hz",
                if self.ni.channel.is_empty() { "(no channel)" } else { &self.ni.channel },
                self.ni.sample_rate_hz
            ),
            SourceKind::SerialAscii => format!(
                "{} @ {}",
                if self.serial.port.is_empty() { "(no port)" } else { &self.serial.port },
                self.serial.baud
            ),
            SourceKind::ModbusRtu => format!(
                "{} @ {} · id {} · reg {}",
                if self.serial.port.is_empty() { "(no port)" } else { &self.serial.port },
                self.serial.baud,
                self.modbus.slave_id,
                self.modbus.register
            ),
            SourceKind::ModbusTcp => format!(
                "{}:{} · id {} · reg {}",
                self.modbus.host, self.modbus.tcp_port, self.modbus.slave_id, self.modbus.register
            ),
            SourceKind::Simulated => format!("{:?} · {} Hz", self.sim.waveform, self.sim.sample_rate_hz),
        }
    }

    /// Sensible defaults when the source type is changed in the editor.
    pub fn apply_kind_defaults(&mut self) {
        match self.kind {
            SourceKind::NiVoltage => {
                self.ni.min = 0.0;
                self.ni.max = 10.0;
                self.scaling.raw_lo = 0.0;
                self.scaling.raw_hi = 10.0;
            }
            SourceKind::NiCurrent => {
                self.ni.min = 4.0;
                self.ni.max = 20.0;
                self.scaling.mode = ScalingMode::Range;
                self.scaling.raw_lo = 4.0;
                self.scaling.raw_hi = 20.0;
            }
            _ => {}
        }
    }

    pub fn keller_preset(&mut self) {
        self.kind = SourceKind::ModbusRtu;
        self.serial.baud = 115200;
        self.serial.data_bits = 8;
        self.serial.parity = Parity::None;
        self.serial.stop_bits = 1;
        self.modbus.slave_id = 1;
        self.modbus.function = ModbusFunction::Holding;
        self.modbus.register = 2;
        self.modbus.data_type = RegisterType::F32;
        self.modbus.word_swap = false;
        self.unit = "psi".into();
        self.scaling.mode = ScalingMode::Linear;
        self.scaling.gain = 14.503_773_8;
        self.scaling.intercept = 0.0;
        self.range_min = 0.0;
        self.range_max = 15000.0;
    }
}

pub const UNIT_GROUPS: &[(&str, &[&str])] = &[
    ("Pressure", &["psi", "bar", "mbar", "kPa", "MPa", "Pa", "inHg", "inH2O", "atm"]),
    ("Force", &["lbf", "ozf", "kip", "N", "kN", "kgf"]),
    ("Weight / mass", &["lb", "oz", "kg", "g", "ton"]),
    ("Torque", &["ft·lbf", "in·lbf", "N·m"]),
    ("Temperature", &["°F", "°C", "K"]),
    ("Electrical", &["V", "mV", "mA", "A", "Ω"]),
    ("Flow", &["gpm", "L/min", "bbl/min", "scfm"]),
    ("Length / position", &["in", "ft", "mm", "m"]),
    ("Other", &["%", "rpm", "Hz", "counts"]),
];
