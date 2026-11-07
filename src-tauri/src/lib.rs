use base64::{engine::general_purpose::STANDARD, Engine as _};
use rayon::prelude::*;
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    ffi::c_void,
    mem::size_of,
    os::windows::process::CommandExt,
    process::Command,
    sync::{mpsc, LazyLock, Mutex, OnceLock, RwLock},
    time::{Duration, UNIX_EPOCH},
};
use sysinfo::{Pid, Process, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{TrayIconBuilder, TrayIconEvent},
    Manager, RunEvent, Runtime, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};
use tauri_plugin_store::StoreExt;
use windows::{
    core::{Interface, Result as WinResult, BOOL, BSTR, HRESULT, PCWSTR},
    Win32::{
        Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND, HANDLE, LUID, RPC_E_CHANGED_MODE},
        Graphics::Gdi::{
            CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, BITMAPINFO, BITMAPINFOHEADER,
            DIB_RGB_COLORS,
        },
        Security::{
            AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES,
            SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
        },
        System::{
            Com::{
                CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
                COINIT_APARTMENTTHREADED, COINIT_MULTITHREADED,
            },
            TaskScheduler::{
                IExecAction, ILogonTrigger, IRegisteredTask, ITaskFolder, ITaskService,
                TaskScheduler, TASK_ACTION_EXEC, TASK_CREATE_OR_UPDATE,
                TASK_LOGON_INTERACTIVE_TOKEN, TASK_RUNLEVEL_HIGHEST, TASK_TRIGGER_LOGON,
            },
            Threading::{
                GetCurrentProcess, GetPriorityClass, GetProcessAffinityMask, GetProcessInformation,
                GetProcessPriorityBoost, OpenProcess, OpenProcessToken, ProcessMemoryPriority,
                ProcessPowerThrottling, SetPriorityClass, SetProcessAffinityMask,
                SetProcessInformation, SetProcessPriorityBoost, TerminateProcess, MEMORY_PRIORITY,
                MEMORY_PRIORITY_INFORMATION, PROCESS_ALL_ACCESS, PROCESS_CREATION_FLAGS,
                PROCESS_INFORMATION_CLASS, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
                PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE,
            },
            Variant::VARIANT,
        },
        UI::{
            Shell::ExtractIconExW,
            WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO},
        },
    },
};
use winreg::{enums::*, types::FromRegValue, RegKey, HKEY};

const IO_CLASS: u32 = 33;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const AUTOSTART_TASK_NAME: &str = "WinProcessAutostart";
const EFFICIENCY_FLAG: u32 = PROCESS_POWER_THROTTLING_EXECUTION_SPEED;

#[link(name = "ntdll")]
extern "system" {
    fn NtQueryInformationProcess(
        h: HANDLE,
        class: u32,
        info: *mut c_void,
        len: u32,
        ret: *mut u32,
    ) -> i32;
    fn NtSetInformationProcess(h: HANDLE, class: u32, info: *mut c_void, len: u32) -> i32;
}

#[link(name = "gdi32")]
extern "system" {
    fn D3DKMTGetProcessSchedulingPriorityClass(h: HANDLE, class: *mut i32) -> i32;
    fn D3DKMTSetProcessSchedulingPriorityClass(h: HANDLE, class: i32) -> i32;
}

fn enable_privilege(name: &str) -> WinResult<()> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        )?;
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let mut luid = LUID::default();
        LookupPrivilegeValueW(PCWSTR::null(), PCWSTR(wide.as_ptr()), &mut luid)?;
        let tp = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES {
                Luid: luid,
                Attributes: SE_PRIVILEGE_ENABLED,
            }],
        };
        AdjustTokenPrivileges(token, false, Some(&tp), 0, None, None)?;
        CloseHandle(token)
    }
}

static ICONS: LazyLock<Mutex<HashMap<(String, Option<u64>), Option<String>>>> =
    LazyLock::new(Default::default);

#[tauri::command]
fn get_process_icon(exe_path: String) -> Option<String> {
    let mtime = std::fs::metadata(&exe_path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs());
    let key = (exe_path, mtime);
    if let Some(cached) = ICONS.lock().unwrap().get(&key) {
        return cached.clone();
    }
    let icon = unsafe { extract_icon(&key.0) };
    ICONS.lock().unwrap().insert(key, icon.clone());
    icon
}

unsafe fn extract_icon(path: &str) -> Option<String> {
    let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    let mut icon = HICON::default();
    if ExtractIconExW(PCWSTR(wide.as_ptr()), 0, Some(&mut icon), None, 1) == 0 || icon.is_invalid()
    {
        return None;
    }
    let mut info = ICONINFO::default();
    let mut px = vec![0u8; 32 * 32 * 4];
    let mut lines = 0;
    if GetIconInfo(icon, &mut info).is_ok() {
        let hdc = CreateCompatibleDC(None);
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: 32,
                biHeight: -32,
                biPlanes: 1,
                biBitCount: 32,
                ..Default::default()
            },
            ..Default::default()
        };
        lines = GetDIBits(
            hdc,
            info.hbmColor,
            0,
            32,
            Some(px.as_mut_ptr().cast()),
            &mut bmi,
            DIB_RGB_COLORS,
        );
        let _ = (
            DeleteDC(hdc),
            DeleteObject(info.hbmColor.into()),
            DeleteObject(info.hbmMask.into()),
        );
    }
    DestroyIcon(icon).ok();
    if lines == 0 {
        return None;
    }
    px.chunks_exact_mut(4).for_each(|c| c.swap(0, 2));
    let mut png = Vec::new();
    image::RgbaImage::from_raw(32, 32, px)?
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    Some(format!("data:image/png;base64,{}", STANDARD.encode(png)))
}

fn with_process_handle<T>(pid: u32, f: impl FnOnce(HANDLE) -> T) -> Option<T> {
    let h = unsafe { OpenProcess(PROCESS_ALL_ACCESS, false, pid) }.ok()?;
    let result = f(h);
    let _ = unsafe { CloseHandle(h) };
    Some(result)
}

fn on_proc(pid: u32, f: impl FnOnce(HANDLE) -> bool) -> bool {
    with_process_handle(pid, f).unwrap_or(false)
}

unsafe fn query<T>(h: HANDLE, class: PROCESS_INFORMATION_CLASS, mut v: T) -> Option<T> {
    GetProcessInformation(
        h,
        class,
        &mut v as *mut T as *mut c_void,
        size_of::<T>() as u32,
    )
    .ok()
    .map(|_| v)
}

unsafe fn update<T>(h: HANDLE, class: PROCESS_INFORMATION_CLASS, mut v: T) -> bool {
    SetProcessInformation(
        h,
        class,
        &mut v as *mut T as *mut c_void,
        size_of::<T>() as u32,
    )
    .is_ok()
}

fn throttling(control: u32, state: u32) -> PROCESS_POWER_THROTTLING_STATE {
    PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: control,
        StateMask: state,
    }
}

fn set_power_plan_native(id: &str) -> bool {
    Command::new("powercfg")
        .args(["/setactive", id])
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .is_ok_and(|s| s.success())
}

fn apply_rule_native(pid: u32, rule: &str, value: &str) -> bool {
    let label = |table: &[(&str, u32)]| table.iter().find(|e| e.0 == value).map(|e| e.1);
    let on = value == "true";
    match rule {
        "cpu_priority" => label(&[
            ("realtime", 0x100),
            ("high", 0x80),
            ("abovenormal", 0x8000),
            ("normal", 0x20),
            ("belownormal", 0x4000),
            ("idle", 0x40),
        ])
        .is_some_and(|c| {
            on_proc(pid, |h| unsafe {
                SetPriorityClass(h, PROCESS_CREATION_FLAGS(c)).is_ok()
            })
        }),
        "cpu_affinity" => value.parse::<u64>().is_ok_and(|m| {
            on_proc(pid, |h| unsafe {
                SetProcessAffinityMask(h, m as usize).is_ok()
            })
        }),
        "io_priority" => label(&[("verylow", 0), ("low", 1), ("normal", 2), ("high", 3)])
            .is_some_and(|mut v| {
                on_proc(pid, |h| unsafe {
                    NtSetInformationProcess(h, IO_CLASS, &mut v as *mut u32 as *mut c_void, 4) == 0
                })
            }),
        "gpu_priority" => label(&[
            ("idle", 0),
            ("belownormal", 1),
            ("normal", 2),
            ("abovenormal", 3),
            ("high", 4),
            ("realtime", 5),
        ])
        .is_some_and(|v| {
            on_proc(pid, |h| unsafe {
                D3DKMTSetProcessSchedulingPriorityClass(h, v as i32) == 0
            })
        }),
        "mem_priority" => label(&[
            ("verylow", 1),
            ("low", 2),
            ("medium", 3),
            ("belownormal", 4),
            ("normal", 5),
        ])
        .is_some_and(|v| {
            on_proc(pid, |h| unsafe {
                update(
                    h,
                    ProcessMemoryPriority,
                    MEMORY_PRIORITY_INFORMATION {
                        MemoryPriority: MEMORY_PRIORITY(v),
                    },
                )
            })
        }),
        "boost" => on_proc(pid, |h| unsafe { SetProcessPriorityBoost(h, !on).is_ok() }),
        "efficiency_mode" => on_proc(pid, |h| unsafe {
            let class_ok =
                SetPriorityClass(h, PROCESS_CREATION_FLAGS(if on { 0x40 } else { 0x20 })).is_ok();
            let throttle_ok = update(
                h,
                ProcessPowerThrottling,
                throttling(EFFICIENCY_FLAG, if on { EFFICIENCY_FLAG } else { 0 }),
            );
            class_ok && throttle_ok
        }),
        "powerplan" => set_power_plan_native(value),
        _ => false,
    }
}

#[tauri::command]
fn set_cpu_priority(pid: u32, priority: String) -> bool {
    apply_rule_native(pid, "cpu_priority", &priority)
}
#[tauri::command]
fn set_process_affinity(pid: u32, affinity: u64) -> bool {
    apply_rule_native(pid, "cpu_affinity", &affinity.to_string())
}
#[tauri::command]
fn set_io_priority(pid: u32, priority: String) -> bool {
    apply_rule_native(pid, "io_priority", &priority)
}
#[tauri::command]
fn set_gpu_priority(pid: u32, priority: String) -> bool {
    apply_rule_native(pid, "gpu_priority", &priority)
}
#[tauri::command]
fn set_memory_priority(pid: u32, priority: String) -> bool {
    apply_rule_native(pid, "mem_priority", &priority)
}
#[tauri::command]
fn set_process_boost(pid: u32, enable: bool) -> bool {
    apply_rule_native(pid, "boost", &enable.to_string())
}
#[tauri::command]
fn set_efficiency_mode(pid: u32, enable: bool) -> bool {
    apply_rule_native(pid, "efficiency_mode", &enable.to_string())
}
#[tauri::command]
fn set_power_plan(id: String) -> bool {
    set_power_plan_native(&id)
}
#[tauri::command]
fn terminate_process(pid: u32) -> bool {
    on_proc(pid, |h| unsafe { TerminateProcess(h, 1).is_ok() })
}

#[derive(Serialize, Default)]
struct ProcessInfo {
    pid: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exe_path: Option<String>,
    cpu: f32,
    priority: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    affinity: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    io_priority: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mem_priority: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    boost: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    efficiency_mode: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gpu_priority: Option<i32>,
}

struct AppState {
    sys: Mutex<System>,
    static_cache: Mutex<HashMap<u32, u64>>,
}

fn refresh_kind() -> ProcessRefreshKind {
    ProcessRefreshKind::nothing()
        .with_cpu()
        .with_exe(UpdateKind::OnlyIfNotSet)
}

fn process_info(pid: u32, cpu: f32) -> ProcessInfo {
    let base = |priority| ProcessInfo {
        pid: pid as i32,
        cpu,
        priority,
        ..Default::default()
    };
    with_process_handle(pid, |h| unsafe {
        ProcessInfo {
            affinity: {
                let (mut proc_mask, mut sys_mask) = (0usize, 0usize);
                GetProcessAffinityMask(h, &mut proc_mask, &mut sys_mask)
                    .ok()
                    .map(|_| proc_mask as u64)
            },
            io_priority: {
                let (mut v, mut n) = (0u32, 0u32);
                (NtQueryInformationProcess(
                    h,
                    IO_CLASS,
                    &mut v as *mut u32 as *mut c_void,
                    4,
                    &mut n,
                ) == 0)
                    .then_some(v as i32)
            },
            mem_priority: query(
                h,
                ProcessMemoryPriority,
                MEMORY_PRIORITY_INFORMATION::default(),
            )
            .map(|i| i.MemoryPriority.0 as i32),
            boost: {
                let mut disabled = BOOL::default();
                GetProcessPriorityBoost(h, &mut disabled)
                    .ok()
                    .map(|_| !disabled.as_bool())
            },
            efficiency_mode: query(h, ProcessPowerThrottling, throttling(0, 0))
                .map(|s| s.ControlMask & s.StateMask & EFFICIENCY_FLAG != 0),
            gpu_priority: {
                let mut class = 2;
                (D3DKMTGetProcessSchedulingPriorityClass(h, &mut class) == 0).then_some(class)
            },
            ..base(match GetPriorityClass(h) {
                0x40 => 19,
                0x4000 => 10,
                0x8000 => -5,
                0x80 => -10,
                0x100 => -20,
                _ => 0,
            })
        }
    })
    .unwrap_or_else(|| base(-1))
}

fn named(mut info: ProcessInfo, p: &Process) -> ProcessInfo {
    info.name = Some(p.name().to_string_lossy().into_owned());
    info.exe_path = p.exe().map(|e| e.to_string_lossy().into_owned());
    info
}

#[tauri::command]
fn reset_process_cache(state: State<AppState>) {
    state.static_cache.lock().unwrap().clear();
}

#[tauri::command]
fn get_process_list(state: State<AppState>) -> Vec<ProcessInfo> {
    let mut sys = state.sys.lock().unwrap();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh_kind());
    let mut cache = state.static_cache.lock().unwrap();
    let procs = sys.processes();
    cache.retain(|pid, _| procs.contains_key(&Pid::from_u32(*pid)));

    let jobs: Vec<_> = procs
        .iter()
        .map(|(pid, p)| {
            let pid = pid.as_u32();
            (
                pid,
                p,
                cache.insert(pid, p.start_time()) != Some(p.start_time()),
            )
        })
        .collect();
    jobs.into_par_iter()
        .map(|(pid, p, fresh)| {
            let cpu_count = sys.cpus().len().max(1) as f32;
            let info = process_info(pid, p.cpu_usage() / cpu_count);
            if fresh {
                named(info, p)
            } else {
                info
            }
        })
        .collect()
}

#[tauri::command]
fn get_process_by_pid(pid: i32, state: State<AppState>) -> Option<ProcessInfo> {
    let id = Pid::from_u32(pid as u32);
    let mut sys = state.sys.lock().unwrap();
    sys.refresh_processes_specifics(ProcessesToUpdate::Some(&[id]), true, refresh_kind());
    let p = sys.process(id)?;
    let cpu_count = sys.cpus().len().max(1) as f32;
    Some(named(
        process_info(pid as u32, p.cpu_usage() / cpu_count),
        p,
    ))
}

#[derive(Serialize)]
struct StartupEntry {
    name: String,
    command: String,
    source: String,
    enabled: bool,
    registry_path: Option<String>,
}

fn read_services() -> Vec<StartupEntry> {
    const P: &str = r"SYSTEM\CurrentControlSet\Services";
    let Ok(root) = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(P) else {
        return vec![];
    };
    root.enum_keys()
        .flatten()
        .filter_map(|name| {
            let k = root.open_subkey(&name).ok()?;
            let (start, ty): (u32, u32) = (k.get_value("Start").ok()?, k.get_value("Type").ok()?);
            let command: String = k.get_value("ImagePath").ok()?;
            let l = command.to_ascii_lowercase();
            if !(2..=4).contains(&start)
                || ty & 0x30 == 0
                || l.contains(r"\windows\")
                || l.contains("systemroot")
                || l.starts_with("system32")
            {
                return None;
            }
            Some(StartupEntry {
                registry_path: Some(format!(r"HKLM\{P}\{name}")),
                name,
                command,
                source: "Service".into(),
                enabled: start != 4,
            })
        })
        .collect()
}

fn read_run_key(hive: HKEY, path: &str, enabled: bool) -> Vec<StartupEntry> {
    let Ok(key) = RegKey::predef(hive).open_subkey(path) else {
        return vec![];
    };
    let registry_path = format!(
        r"{}\{path}",
        if hive == HKEY_CURRENT_USER {
            "HKCU"
        } else {
            "HKLM"
        }
    );
    key.enum_values()
        .flatten()
        .filter_map(|(name, v)| {
            if !matches!(v.vtype, RegType::REG_SZ | RegType::REG_EXPAND_SZ) {
                return None;
            }
            let command = String::from_reg_value(&v).ok().filter(|c| !c.is_empty())?;
            Some(StartupEntry {
                name,
                command,
                source: "Registry".into(),
                enabled,
                registry_path: Some(registry_path.clone()),
            })
        })
        .collect()
}

fn read_startup_folder(path: &str, source: &str) -> Vec<StartupEntry> {
    let (mut out, mut dirs) = (vec![], vec![std::path::PathBuf::from(path)]);
    while let Some(dir) = dirs.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                dirs.push(p);
            } else if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("ini")) {
                let command = p.to_string_lossy().into_owned();
                out.push(StartupEntry {
                    name: e.file_name().to_string_lossy().into_owned(),
                    enabled: !(command.contains("AutorunsDisabled")
                        || command.contains("WinProcessDisabled")),
                    command,
                    source: source.into(),
                    registry_path: None,
                });
            }
        }
    }
    out
}

#[tauri::command]
fn get_startup_entries() -> Vec<StartupEntry> {
    const RUN: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run";
    const ONCE: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\RunOnce";
    const WOW: &str = r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Run";
    let (lm, cu) = (HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER);

    let mut entries: Vec<StartupEntry> = [(cu, RUN), (lm, RUN), (cu, ONCE), (lm, ONCE), (lm, WOW)]
        .into_iter()
        .flat_map(|(h, p)| read_run_key(h, p, true))
        .collect();
    for base in [RUN, ONCE, WOW] {
        for suffix in ["WinProcessDisabled", "AutorunsDisabled"] {
            for hive in [lm, cu].into_iter().filter(|&h| base != WOW || h == lm) {
                entries.extend(read_run_key(hive, &format!(r"{base}\{suffix}"), false));
            }
        }
    }

    if let Ok(appdata) = std::env::var("APPDATA") {
        let user = format!(r"{appdata}\Microsoft\Windows\Start Menu\Programs\Startup");
        entries.extend(read_startup_folder(&user, "User Folder"));
    }
    entries.extend(read_startup_folder(
        r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Startup",
        "Startup Folder",
    ));
    entries.extend(read_services());

    let states: HashMap<String, bool> = RegKey::predef(cu)
        .open_subkey(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run")
        .map(|k| {
            k.enum_values()
                .flatten()
                .map(|(name, v)| (name, v.bytes.first() == Some(&2)))
                .collect()
        })
        .unwrap_or_default();
    for e in entries
        .iter_mut()
        .filter(|e| e.source == "Registry" && e.enabled)
    {
        if let Some(&state) = states.get(&e.name) {
            e.enabled = state;
        }
    }
    entries
}

struct ComGuard;
impl Drop for ComGuard {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

fn with_com<T>(f: impl FnOnce() -> WinResult<T>) -> Result<T, String> {
    let init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if init.is_err() && init != RPC_E_CHANGED_MODE {
        return Err(format!("CoInitializeEx failed: {init:?}"));
    }
    let _guard = init.is_ok().then_some(ComGuard);
    f().map_err(|e| format!("{e:?}"))
}

fn task_service() -> WinResult<ITaskService> {
    unsafe {
        let service: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)?;
        let v = VARIANT::default();
        service.Connect(&v, &v, &v, &v)?;
        Ok(service)
    }
}

fn with_task_folder<T>(
    f: impl FnOnce(&ITaskService, &ITaskFolder) -> WinResult<T>,
) -> Result<T, String> {
    with_com(|| unsafe {
        let service = task_service()?;
        let folder = service.GetFolder(&BSTR::from("\\"))?;
        f(&service, &folder)
    })
}

#[derive(Clone, Serialize)]
struct ScheduledTaskInfo {
    name: String,
    path: String,
    state: String,
    enabled: bool,
    author: String,
    last_run_time: Option<String>,
}

type Tasks = Result<Vec<ScheduledTaskInfo>, String>;

fn ole_date_to_iso(ole: f64) -> Option<String> {
    if ole == 0.0 {
        return None;
    }
    let epoch = chrono::NaiveDate::from_ymd_opt(1899, 12, 30)?.and_hms_opt(0, 0, 0)?;
    let ms = ole.trunc() as i64 * 86_400_000 + (ole.fract().abs() * 86_400_000.0).round() as i64;
    Some(
        (epoch + chrono::Duration::milliseconds(ms))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string(),
    )
}

fn extract_xml_field(xml: &str, tag: &str) -> Option<String> {
    Some(
        xml.split_once(&format!("<{tag}>"))?
            .1
            .split_once(&format!("</{tag}>"))?
            .0
            .trim()
            .into(),
    )
}

static AUTHORS: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Default::default);

fn author_of(task: &IRegisteredTask, path: &str) -> String {
    if let Some(a) = AUTHORS.lock().unwrap().get(path) {
        return a.clone();
    }
    let author = unsafe { task.Xml() }
        .ok()
        .and_then(|xml| extract_xml_field(&xml.to_string(), "Author"))
        .unwrap_or_default();
    AUTHORS.lock().unwrap().insert(path.into(), author.clone());
    author
}

fn child_path(parent: &str, name: &str) -> String {
    format!("{}\\{name}", if parent == "\\" { "" } else { parent })
}

unsafe fn collect_tasks(
    folder: &ITaskFolder,
    parent: &str,
    out: &mut Vec<ScheduledTaskInfo>,
) -> WinResult<()> {
    let tasks = folder.GetTasks(0)?;
    for i in 1..=tasks.Count()? {
        let task = tasks.get_Item(&VARIANT::from(i))?;
        let name = task.Name()?.to_string();
        let path = child_path(parent, &name);
        out.push(ScheduledTaskInfo {
            state: format!("{:?}", task.State()?),
            enabled: task.Enabled()?.as_bool(),
            last_run_time: task.LastRunTime().ok().and_then(ole_date_to_iso),
            author: author_of(&task, &path),
            name,
            path,
        });
    }
    let subfolders = folder.GetFolders(0)?;
    for i in 1..=subfolders.Count()? {
        let sub: ITaskFolder = subfolders.get_Item(&VARIANT::from(i))?;
        collect_tasks(&sub, &child_path(parent, &sub.Name()?.to_string()), out)?;
    }
    Ok(())
}

fn list_tasks(service: &ITaskService) -> WinResult<Vec<ScheduledTaskInfo>> {
    let mut out = Vec::with_capacity(64);
    unsafe { collect_tasks(&service.GetFolder(&BSTR::from("\\"))?, "\\", &mut out)? };
    let live: HashSet<&str> = out.iter().map(|t| t.path.as_str()).collect();
    AUTHORS
        .lock()
        .unwrap()
        .retain(|path, _| live.contains(path.as_str()));
    Ok(out)
}

static TASKS: RwLock<Vec<ScheduledTaskInfo>> = RwLock::new(Vec::new());
static WORKER: OnceLock<mpsc::Sender<(bool, mpsc::Sender<Tasks>)>> = OnceLock::new();

fn refresh_tasks(service: &ITaskService) -> WinResult<()> {
    *TASKS.write().unwrap() = list_tasks(service)?;
    Ok(())
}

fn worker() -> &'static mpsc::Sender<(bool, mpsc::Sender<Tasks>)> {
    WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<(bool, mpsc::Sender<Tasks>)>();
        std::thread::spawn(move || {
            if unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_err() {
                return;
            }
            let _guard = ComGuard;
            let Ok(service) = task_service() else { return };
            let _ = refresh_tasks(&service);
            loop {
                match rx.recv_timeout(Duration::from_secs(2)) {
                    Ok((true, reply)) => {
                        let result = refresh_tasks(&service).map(|_| TASKS.read().unwrap().clone());
                        let _ = reply.send(result.map_err(|e| e.to_string()));
                    }
                    Ok((false, reply)) => drop(reply.send(Ok(TASKS.read().unwrap().clone()))),
                    Err(mpsc::RecvTimeoutError::Timeout) => drop(refresh_tasks(&service)),
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });
        tx
    })
}

async fn query_tasks(force: bool) -> Tasks {
    let (tx, rx) = mpsc::channel();
    worker().send((force, tx)).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || rx.recv().map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())??
}

#[tauri::command]
async fn get_scheduled_tasks() -> Tasks {
    query_tasks(false).await
}

#[tauri::command]
async fn refresh_scheduled_tasks() -> Tasks {
    query_tasks(true).await
}

#[derive(Serialize)]
struct SystemStats {
    cpu_usage: f32,
    ram_total: u64,
    ram_used: u64,
    ram_percentage: f32,
    uptime: u64,
    os_name: String,
}

#[tauri::command]
fn get_system_stats(state: State<AppState>) -> SystemStats {
    let mut sys = state.sys.lock().unwrap();
    sys.refresh_cpu_usage();
    sys.refresh_memory();
    let (ram_total, ram_used) = (sys.total_memory(), sys.used_memory());
    SystemStats {
        cpu_usage: sys.global_cpu_usage(),
        ram_total,
        ram_used,
        ram_percentage: if ram_total > 0 {
            ram_used as f32 / ram_total as f32 * 100.0
        } else {
            0.0
        },
        uptime: System::uptime(),
        os_name: System::long_os_version().unwrap_or_else(|| "Windows".into()),
    }
}

#[derive(Serialize)]
struct PowerPlan {
    id: String,
    name: String,
    active: bool,
}

#[tauri::command]
fn get_power_plans() -> Vec<PowerPlan> {
    let Ok(out) = Command::new("powercfg")
        .arg("/list")
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    else {
        return vec![];
    };
    if !out.status.success() {
        return vec![];
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let (id, tail) = line
                .trim()
                .strip_prefix("Power Scheme GUID:")?
                .trim()
                .split_once(' ')?;
            let tail = tail.trim();
            let (tail, active) = tail
                .strip_suffix('*')
                .map_or((tail, false), |t| (t.trim(), true));
            let name = tail
                .strip_prefix('(')
                .and_then(|s| s.strip_suffix(')'))
                .unwrap_or(tail)
                .trim();
            Some(PowerPlan {
                id: id.into(),
                name: name.into(),
                active,
            })
        })
        .collect()
}

#[derive(Default)]
struct RuleEngine {
    sys: System,
    known_names: HashMap<u32, String>,
    applied_rules: HashMap<u32, HashMap<String, String>>,
    applied_power_plans: Vec<(String, String)>,
    current_plan_id: Option<String>,
    default_plan: Option<String>,
    needs_reset: bool,
}

impl RuleEngine {
    fn tick(&mut self, app: &tauri::AppHandle) {
        self.sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing(),
        );
        let procs = self.sys.processes();
        self.known_names
            .retain(|pid, _| procs.contains_key(&Pid::from_u32(*pid)));
        self.applied_rules
            .retain(|pid, _| procs.contains_key(&Pid::from_u32(*pid)));
        for (pid, p) in procs {
            self.known_names
                .entry(pid.as_u32())
                .or_insert_with(|| p.name().to_string_lossy().into_owned());
        }

        let Ok(store) = app.store("always.json") else {
            return;
        };
        if self.default_plan.is_none() {
            self.default_plan = get_power_plans()
                .into_iter()
                .find(|p| p.active)
                .map(|p| p.id);
        }

        let running: HashSet<&str> = self.known_names.values().map(String::as_str).collect();
        let mut plans = std::mem::take(&mut self.applied_power_plans);
        plans.retain(|(name, _)| running.contains(name.as_str()));
        for (_, id) in &plans {
            if self.current_plan_id.as_deref() != Some(id.as_str()) {
                set_power_plan_native(id);
                self.current_plan_id = Some(id.clone());
                self.needs_reset = true;
            }
        }
        self.applied_power_plans = plans;

        if self.needs_reset && self.applied_power_plans.is_empty() {
            if let Some(default) = &self.default_plan {
                if self.current_plan_id.as_deref() != Some(default.as_str()) {
                    set_power_plan_native(default);
                    self.current_plan_id = Some(default.clone());
                }
            }
            self.needs_reset = false;
        }

        let rules: Vec<(String, String, String)> = store
            .keys()
            .into_iter()
            .filter_map(|key| {
                let v = store.get(&key)?;
                let ty = v.get("type")?.as_str()?.to_owned();
                let val = match v.get("value")? {
                    n if n.is_number() => n.to_string(),
                    s => s.as_str()?.to_owned(),
                };
                Some((key, ty, val))
            })
            .collect();

        for (&pid, name) in &self.known_names {
            let prefix = format!("{name}_");
            for (key, ty, val) in rules.iter().filter(|r| r.0.starts_with(&prefix)) {
                let done = self.applied_rules.entry(pid).or_default();
                if done.get(key) == Some(val) {
                    continue;
                }
                apply_rule_native(pid, ty, val);
                done.insert(key.clone(), val.clone());
                if ty == "powerplan" {
                    self.applied_power_plans.push((name.clone(), val.clone()));
                    self.current_plan_id = Some(val.clone());
                    self.needs_reset = true;
                }
            }
        }
    }
}

fn spawn_rule_engine(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        let mut engine = RuleEngine::default();
        loop {
            engine.tick(&app);
            let ms = app
                .store("settings.json")
                .ok()
                .and_then(|s| s.get("polling"))
                .and_then(|v| v.as_u64());
            std::thread::sleep(Duration::from_millis(ms.unwrap_or(2000)));
        }
    });
}

fn create_main_window<R: Runtime>(app: &impl Manager<R>) -> tauri::Result<WebviewWindow<R>> {
    WebviewWindowBuilder::new(app, "main", WebviewUrl::App(Default::default()))
        .title("WinProcess")
        .inner_size(1200.0, 776.0)
        .min_inner_size(750.0, 650.0)
        .visible(false) // shown by the frontend
        .decorations(false)
        .center()
        .build()
}

fn show_main<R: Runtime>(app: &impl Manager<R>) {
    match app.get_webview_window("main") {
        Some(w) => drop((w.show(), w.unminimize(), w.set_focus())),
        None => drop(create_main_window(app)),
    }
}

fn existing_task(folder: &ITaskFolder) -> WinResult<Option<IRegisteredTask>> {
    match unsafe { folder.GetTask(&BSTR::from(AUTOSTART_TASK_NAME)) } {
        Ok(task) => Ok(Some(task)),
        Err(e) if e.code() == HRESULT::from_win32(ERROR_FILE_NOT_FOUND.0) => Ok(None),
        Err(e) => Err(e),
    }
}

#[tauri::command]
fn enable_autostart() -> Result<(), String> {
    let exe = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .to_string_lossy()
        .into_owned();
    with_task_folder(|service, folder| unsafe {
        let def = service.NewTask(0)?;
        let info = def.RegistrationInfo()?;
        info.SetAuthor(&BSTR::from("WinProcess"))?;
        info.SetDescription(&BSTR::from(
            "Launch WinProcess at logon with elevated privileges.",
        ))?;
        def.Principal()?.SetRunLevel(TASK_RUNLEVEL_HIGHEST)?;

        let settings = def.Settings()?;
        settings.SetStartWhenAvailable(true.into())?;
        settings.SetDisallowStartIfOnBatteries(false.into())?;
        settings.SetStopIfGoingOnBatteries(false.into())?;

        let trigger: ILogonTrigger = def.Triggers()?.Create(TASK_TRIGGER_LOGON)?.cast()?;
        trigger.SetUserId(&BSTR::from(
            std::env::var("USERNAME").unwrap_or_default().as_str(),
        ))?;
        trigger.SetEnabled(true.into())?;

        let action: IExecAction = def.Actions()?.Create(TASK_ACTION_EXEC)?.cast()?;
        action.SetPath(&BSTR::from(exe.as_str()))?;
        action.SetArguments(&BSTR::from("--tray"))?;

        let v = VARIANT::default();
        folder.RegisterTaskDefinition(
            &BSTR::from(AUTOSTART_TASK_NAME),
            &def,
            TASK_CREATE_OR_UPDATE.0,
            &v,
            &v,
            TASK_LOGON_INTERACTIVE_TOKEN,
            &v,
        )?;
        Ok(())
    })
}

#[tauri::command]
fn disable_autostart() -> Result<(), String> {
    with_task_folder(|_, folder| unsafe {
        if existing_task(folder)?.is_some() {
            folder.DeleteTask(&BSTR::from(AUTOSTART_TASK_NAME), 0)?;
        }
        Ok(())
    })
}

#[tauri::command]
fn is_autostart_enabled() -> Result<bool, String> {
    with_task_folder(|_, folder| {
        existing_task(folder)?.map_or(Ok(false), |task| {
            unsafe { task.Enabled() }.map(|e| e.as_bool())
        })
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app)
        }))
        .plugin(tauri_plugin_autostart::Builder::new().build())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_os::init())
        .setup(|app| {
            for privilege in ["SeDebugPrivilege", "SeIncreaseBasePriorityPrivilege"] {
                if let Err(e) = enable_privilege(privilege) {
                    eprintln!("Failed to enable {privilege}: {e:?}");
                }
            }

            // first-time default settings
            let settings = app.store("settings.json")?;
            app.store("always.json")?;
            if settings.get("startup").is_none() {
                match enable_autostart() {
                    Ok(_) => settings.set("startup", true),
                    Err(err) => eprintln!("Failed to enable autostart: {err}"),
                }
            }
            if settings.get("polling").is_none() {
                settings.set("polling", 2000);
            }
            settings.save()?;

            spawn_rule_engine(app.handle().clone());

            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            if !std::env::args().any(|arg| arg == "--tray") {
                create_main_window(app)?;
            }

            let show = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            TrayIconBuilder::with_id("main-tray")
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("WinProcess")
                .menu(&Menu::with_items(app, &[&show, &quit])?)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => app.exit(0),
                    "show" => show_main(app),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::DoubleClick { .. } = event {
                        show_main(tray.app_handle());
                    }
                })
                .build(app)?;
            Ok(())
        })
        .manage(AppState {
            sys: Mutex::new(System::new_all()),
            static_cache: Default::default(),
        })
        .invoke_handler(tauri::generate_handler![
            get_process_list,
            get_process_by_pid,
            get_process_icon,
            reset_process_cache,
            get_startup_entries,
            get_scheduled_tasks,
            refresh_scheduled_tasks,
            get_system_stats,
            get_power_plans,
            set_cpu_priority,
            set_process_affinity,
            set_io_priority,
            set_gpu_priority,
            set_memory_priority,
            set_process_boost,
            set_efficiency_mode,
            set_power_plan,
            terminate_process,
            enable_autostart,
            disable_autostart,
            is_autostart_enabled,
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|_app, event| {
            if let RunEvent::ExitRequested {
                code: None, api, ..
            } = event
            {
                api.prevent_exit(); // keep running in the tray
            }
        });
}
