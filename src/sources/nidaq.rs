//! NI-DAQmx analog input through the C API in nicaiu.dll, loaded at runtime so
//! the app still starts on machines without the NI driver installed.

use std::ffi::{c_char, c_void, CString};
use std::ptr;
use std::sync::OnceLock;
use std::time::Duration;

use libloading::Library;

use super::{Link, Sample, SourceEvent};
use crate::config::{SensorConfig, ShuntLocation, SourceKind, TerminalConfig};

type TaskHandle = *mut c_void;

const VAL_CFG_DEFAULT: i32 = -1;
const VAL_RSE: i32 = 10083;
const VAL_NRSE: i32 = 10078;
const VAL_DIFF: i32 = 10106;
const VAL_PSEUDO_DIFF: i32 = 12529;
const VAL_VOLTS: i32 = 10348;
const VAL_AMPS: i32 = 10342;
const VAL_DEFAULT: i32 = -1;
const VAL_INTERNAL: i32 = 10200;
const VAL_EXTERNAL: i32 = 10167;
const VAL_RISING: i32 = 10280;
const VAL_CONT_SAMPS: i32 = 10123;
const VAL_GROUP_BY_CHANNEL: u32 = 0;

struct Api {
    _lib: Library,
    create_task: unsafe extern "system" fn(*const c_char, *mut TaskHandle) -> i32,
    start_task: unsafe extern "system" fn(TaskHandle) -> i32,
    stop_task: unsafe extern "system" fn(TaskHandle) -> i32,
    clear_task: unsafe extern "system" fn(TaskHandle) -> i32,
    create_ai_voltage: unsafe extern "system" fn(
        TaskHandle,
        *const c_char,
        *const c_char,
        i32,
        f64,
        f64,
        i32,
        *const c_char,
    ) -> i32,
    create_ai_current: unsafe extern "system" fn(
        TaskHandle,
        *const c_char,
        *const c_char,
        i32,
        f64,
        f64,
        i32,
        i32,
        f64,
        *const c_char,
    ) -> i32,
    cfg_samp_clk: unsafe extern "system" fn(TaskHandle, *const c_char, f64, i32, i32, u64) -> i32,
    read_f64: unsafe extern "system" fn(TaskHandle, i32, f64, u32, *mut f64, u32, *mut i32, *mut u32) -> i32,
    read_avail: unsafe extern "system" fn(TaskHandle, *mut u32) -> i32,
    extended_error: unsafe extern "system" fn(*mut c_char, u32) -> i32,
    sys_dev_names: unsafe extern "system" fn(*mut c_char, u32) -> i32,
    dev_ai_chans: unsafe extern "system" fn(*const c_char, *mut c_char, u32) -> i32,
    chassis_modules: unsafe extern "system" fn(*const c_char, *mut c_char, u32) -> i32,
}

// The function pointers are plain C entry points; NI-DAQmx is thread-safe.
unsafe impl Send for Api {}
unsafe impl Sync for Api {}

static API: OnceLock<Result<Api, String>> = OnceLock::new();

fn api() -> Result<&'static Api, String> {
    API.get_or_init(load).as_ref().map_err(Clone::clone)
}

fn load() -> Result<Api, String> {
    unsafe {
        let lib = Library::new("nicaiu.dll")
            .map_err(|e| format!("NI-DAQmx driver not found (nicaiu.dll): {e}"))?;
        macro_rules! sym {
            ($name:literal) => {
                *lib.get(concat!($name, "\0").as_bytes())
                    .map_err(|e| format!("nicaiu.dll is missing {}: {e}", $name))?
            };
        }
        Ok(Api {
            create_task: sym!("DAQmxCreateTask"),
            start_task: sym!("DAQmxStartTask"),
            stop_task: sym!("DAQmxStopTask"),
            clear_task: sym!("DAQmxClearTask"),
            create_ai_voltage: sym!("DAQmxCreateAIVoltageChan"),
            create_ai_current: sym!("DAQmxCreateAICurrentChan"),
            cfg_samp_clk: sym!("DAQmxCfgSampClkTiming"),
            read_f64: sym!("DAQmxReadAnalogF64"),
            read_avail: sym!("DAQmxGetReadAvailSampPerChan"),
            extended_error: sym!("DAQmxGetExtendedErrorInfo"),
            sys_dev_names: sym!("DAQmxGetSysDevNames"),
            dev_ai_chans: sym!("DAQmxGetDevAIPhysicalChans"),
            chassis_modules: sym!("DAQmxGetDevChassisModuleDevNames"),
            _lib: lib,
        })
    }
}

impl Api {
    fn check(&self, code: i32) -> Result<(), String> {
        if code >= 0 {
            return Ok(());
        }
        let mut buf = vec![0 as c_char; 4096];
        unsafe { (self.extended_error)(buf.as_mut_ptr(), buf.len() as u32) };
        let msg = c_buf_to_string(&buf);
        Err(if msg.is_empty() { format!("NI-DAQmx error {code}") } else { msg })
    }

    fn string_property(&self, f: impl Fn(*mut c_char, u32) -> i32) -> Vec<String> {
        let mut buf = vec![0 as c_char; 16384];
        if f(buf.as_mut_ptr(), buf.len() as u32) < 0 {
            return Vec::new();
        }
        c_buf_to_string(&buf)
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }
}

fn c_buf_to_string(buf: &[c_char]) -> String {
    let bytes: Vec<u8> = buf.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
    String::from_utf8_lossy(&bytes).trim().to_string()
}

/// Every analog input channel on every detected device, including cDAQ modules.
pub fn list_ai_channels() -> Result<Vec<String>, String> {
    let api = api()?;
    let devices = api.string_property(|p, n| unsafe { (api.sys_dev_names)(p, n) });
    let mut chans = Vec::new();
    for dev in &devices {
        let Ok(c) = CString::new(dev.as_str()) else { continue };
        chans.extend(api.string_property(|p, n| unsafe { (api.dev_ai_chans)(c.as_ptr(), p, n) }));
        for module in api.string_property(|p, n| unsafe { (api.chassis_modules)(c.as_ptr(), p, n) }) {
            let Ok(m) = CString::new(module) else { continue };
            chans.extend(api.string_property(|p, n| unsafe { (api.dev_ai_chans)(m.as_ptr(), p, n) }));
        }
    }
    chans.sort();
    chans.dedup();
    match (chans.is_empty(), devices.is_empty()) {
        (false, _) => Ok(chans),
        (true, true) => Err("No NI-DAQmx devices found. Add them in NI MAX, or type the channel name.".into()),
        (true, false) => Err(format!(
            "Found {} but no analog input modules. Check the chassis is powered, on the network, \
             and reserved in NI MAX — or type the channel name.",
            devices.join(", ")
        )),
    }
}

struct Task<'a> {
    api: &'a Api,
    handle: TaskHandle,
}

impl Drop for Task<'_> {
    fn drop(&mut self) {
        unsafe {
            (self.api.stop_task)(self.handle);
            (self.api.clear_task)(self.handle);
        }
    }
}

pub fn run(cfg: &SensorConfig, link: &Link) {
    let api = match api() {
        Ok(api) => api,
        Err(e) => {
            // Driver missing will not fix itself; keep reporting without spinning.
            while !link.stopped() {
                link.send(SourceEvent::Warning(e.clone()));
                link.sleep(Duration::from_secs(5));
            }
            return;
        }
    };
    link.retry_loop(|link| acquire(api, cfg, link));
}

fn acquire(api: &Api, cfg: &SensorConfig, link: &Link) -> Result<(), String> {
    let ni = &cfg.ni;
    if ni.channel.trim().is_empty() {
        return Err("No NI physical channel selected".into());
    }
    let chan = CString::new(ni.channel.trim()).map_err(|e| e.to_string())?;
    let terminal = match ni.terminal {
        TerminalConfig::Default => VAL_CFG_DEFAULT,
        TerminalConfig::Rse => VAL_RSE,
        TerminalConfig::Nrse => VAL_NRSE,
        TerminalConfig::Differential => VAL_DIFF,
        TerminalConfig::PseudoDifferential => VAL_PSEUDO_DIFF,
    };
    let rate = ni.sample_rate_hz.max(0.1);
    let current = cfg.kind == SourceKind::NiCurrent;
    // Current is configured in amps but shown and scaled in mA.
    let raw_factor = if current { 1000.0 } else { 1.0 };

    let mut handle: TaskHandle = ptr::null_mut();
    api.check(unsafe { (api.create_task)(c"".as_ptr(), &mut handle) })?;
    let task = Task { api, handle };

    unsafe {
        if current {
            let shunt = match ni.shunt {
                ShuntLocation::Default => VAL_DEFAULT,
                ShuntLocation::Internal => VAL_INTERNAL,
                ShuntLocation::External => VAL_EXTERNAL,
            };
            api.check((api.create_ai_current)(
                task.handle,
                chan.as_ptr(),
                c"".as_ptr(),
                terminal,
                ni.min / 1000.0,
                ni.max / 1000.0,
                VAL_AMPS,
                shunt,
                ni.ext_shunt_ohms,
                ptr::null(),
            ))?;
        } else {
            api.check((api.create_ai_voltage)(
                task.handle,
                chan.as_ptr(),
                c"".as_ptr(),
                terminal,
                ni.min,
                ni.max,
                VAL_VOLTS,
                ptr::null(),
            ))?;
        }
        let buffer = ((rate * 10.0) as u64).max(1000);
        api.check((api.cfg_samp_clk)(task.handle, ptr::null(), rate, VAL_RISING, VAL_CONT_SAMPS, buffer))?;
        api.check((api.start_task)(task.handle))?;
    }

    let t0 = link.clock.now();
    let mut n: u64 = 0;
    let mut buf = vec![0.0f64; ((rate * 2.0) as usize).max(256)];
    link.send(SourceEvent::Connected(format!("{} @ {rate} Hz", ni.channel)));

    while !link.stopped() {
        let mut avail: u32 = 0;
        api.check(unsafe { (api.read_avail)(task.handle, &mut avail) })?;
        if avail == 0 {
            link.sleep(Duration::from_millis(20));
            continue;
        }
        let want = avail.min(buf.len() as u32);
        let mut read: i32 = 0;
        let code = unsafe {
            (api.read_f64)(
                task.handle,
                want as i32,
                1.0,
                VAL_GROUP_BY_CHANNEL,
                buf.as_mut_ptr(),
                buf.len() as u32,
                &mut read,
                ptr::null_mut(),
            )
        };
        api.check(code)?;
        if read > 0 {
            let batch = buf[..read as usize]
                .iter()
                .map(|&v| {
                    let s = Sample { t: t0 + n as f64 / rate, raw: v * raw_factor };
                    n += 1;
                    s
                })
                .collect();
            link.send(SourceEvent::Samples(batch));
        }
        link.sleep(Duration::from_millis(20));
    }
    Ok(())
}
