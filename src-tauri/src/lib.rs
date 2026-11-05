use base64::{engine::general_purpose, Engine as _};
use chrono::{Duration, NaiveDate};
use rayon::prelude::*;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::os::windows::process::CommandExt;
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::SystemTime;
#[cfg_attr(mobile, tauri::mobile_entry_point)]
use sysinfo::System;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, UpdateKind};
use tauri::command;
use tauri::State;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{TrayIconBuilder, TrayIconEvent},
    Manager, RunEvent, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};
use tauri_plugin_store::StoreExt;
use windows::core::Interface;
use windows::core::HRESULT;
use windows::core::PCWSTR;
use windows::core::{Result as WinResult, BSTR};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_FILE_NOT_FOUND, LUID, RPC_E_CHANGED_MODE, S_FALSE,
};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, BITMAPINFO, BITMAPINFOHEADER,
    DIB_RGB_COLORS, RGBQUAD,
};
use windows::Win32::Security::{
    AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED,
    TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED, COINIT_MULTITHREADED,
};
use windows::Win32::System::TaskScheduler::{
    IAction, IActionCollection, IExecAction, ILogonTrigger, IPrincipal, IRegisteredTask,
    IRegistrationInfo, ITaskDefinition, ITaskFolder, ITaskService, ITaskSettings, ITrigger,
    ITriggerCollection, TaskScheduler, TASK_ACTION_EXEC, TASK_CREATE_OR_UPDATE,
    TASK_LOGON_INTERACTIVE_TOKEN, TASK_RUNLEVEL_HIGHEST, TASK_TRIGGER_LOGON,
};
use windows::Win32::System::Threading::PROCESS_ALL_ACCESS;
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetProcessInformation, GetProcessPriorityBoost, OpenProcessToken,
    ProcessMemoryPriority, ProcessPowerThrottling, MEMORY_PRIORITY, MEMORY_PRIORITY_INFORMATION,
    PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
    PROCESS_POWER_THROTTLING_STATE,
};
use windows::Win32::System::Threading::{
    GetPriorityClass, GetProcessAffinityMask, OpenProcess, SetPriorityClass,
    SetProcessAffinityMask, SetProcessInformation, SetProcessPriorityBoost, TerminateProcess,
    PROCESS_CREATION_FLAGS,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Shell::ExtractIconExW;
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, ICONINFO};
use winreg::HKEY;
use winreg::{enums::*, RegKey, RegValue};

fn enable_privilege(name: &str) -> windows::core::Result<()> {
    unsafe {
        let mut token = Default::default();
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        )?;

        let wide_name: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let mut luid = LUID::default();
        LookupPrivilegeValueW(PCWSTR::null(), PCWSTR(wide_name.as_ptr()), &mut luid)?;

        let tp = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES {
                Luid: luid,
                Attributes: SE_PRIVILEGE_ENABLED,
            }],
        };

        AdjustTokenPrivileges(token, false, Some(&tp), 0, None, None)?;
        CloseHandle(token)?;
        Ok(())
    }
}

type IconCacheKey = (String, u32, Option<u64>);

static ICON_CACHE: OnceLock<Mutex<HashMap<IconCacheKey, Option<String>>>> = OnceLock::new();

fn icon_cache() -> &'static Mutex<HashMap<IconCacheKey, Option<String>>> {
    ICON_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn file_mtime_secs(path: &str) -> Option<u64> {
    std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

#[tauri::command]
fn get_process_icon(exe_path: String) -> Option<String> {
    let (icon_source_path, icon_index) = resolve_icon_source(&exe_path);
    let mtime = file_mtime_secs(&icon_source_path);
    let key: IconCacheKey = (icon_source_path.clone(), icon_index, mtime);

    if let Some(cached) = icon_cache().lock().unwrap().get(&key) {
        return cached.clone();
    }

    let result = extract_icon_uncached(&icon_source_path, icon_index);

    icon_cache().lock().unwrap().insert(key, result.clone());
    result
}

unsafe fn extract_icon_uncached_inner(icon_source_path: &str, icon_index: u32) -> Option<String> {
    let wide: Vec<u16> = icon_source_path
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut large_icon = windows::Win32::UI::WindowsAndMessaging::HICON::default();
    let mut small_icon = windows::Win32::UI::WindowsAndMessaging::HICON::default();

    let count = ExtractIconExW(
        PCWSTR(wide.as_ptr()),
        icon_index as i32,
        Some(&mut large_icon),
        Some(&mut small_icon),
        1,
    );

    if count == 0 || large_icon.is_invalid() {
        return None;
    }

    let icon = large_icon;

    let mut icon_info = ICONINFO::default();
    if GetIconInfo(icon, &mut icon_info).is_err() {
        DestroyIcon(icon).ok();
        return None;
    }

    let hdc = CreateCompatibleDC(None);

    let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: 32,
            biHeight: -32,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: 0,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [RGBQUAD::default(); 1],
    };

    let mut pixels: Vec<u8> = vec![0u8; 32 * 32 * 4];

    let lines = GetDIBits(
        hdc,
        icon_info.hbmColor,
        0,
        32,
        Some(pixels.as_mut_ptr() as *mut _),
        &mut bmi,
        DIB_RGB_COLORS,
    );

    let _ = DeleteDC(hdc);
    let _ = DeleteObject(icon_info.hbmColor.into());
    let _ = DeleteObject(icon_info.hbmMask.into());
    DestroyIcon(icon).ok();

    if lines == 0 {
        return None;
    }

    for chunk in pixels.chunks_mut(4) {
        chunk.swap(0, 2);
    }

    let img = image::RgbaImage::from_raw(32, 32, pixels)?;
    let mut png_bytes: Vec<u8> = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut png_bytes),
        image::ImageFormat::Png,
    )
    .ok()?;

    Some(format!(
        "data:image/png;base64,{}",
        general_purpose::STANDARD.encode(&png_bytes)
    ))
}

fn extract_icon_uncached(icon_source_path: &str, icon_index: u32) -> Option<String> {
    unsafe { extract_icon_uncached_inner(icon_source_path, icon_index) }
}

fn resolve_icon_source(exe_path: &str) -> (String, u32) {
    (exe_path.to_string(), 0)
}

#[link(name = "ntdll")]
extern "system" {
    fn NtQueryInformationProcess(
        process_handle: HANDLE,
        process_information_class: u32,
        process_information: *mut core::ffi::c_void,
        process_information_length: u32,
        return_length: *mut u32,
    ) -> i32;
}

const PROCESS_IO_PRIORITY: u32 = 33;

const CREATE_NO_WINDOW: u32 = 0x08000000;

fn get_io_priority(handle: HANDLE) -> Option<i32> {
    unsafe {
        let mut io_priority: u32 = 0;
        let mut return_length: u32 = 0;
        let status = NtQueryInformationProcess(
            handle,
            PROCESS_IO_PRIORITY,
            &mut io_priority as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<u32>() as u32,
            &mut return_length,
        );
        (status == 0).then_some(io_priority as i32)
    }
}

fn get_memory_priority(handle: HANDLE) -> Option<i32> {
    unsafe {
        let mut info = MEMORY_PRIORITY_INFORMATION::default();
        GetProcessInformation(
            handle,
            ProcessMemoryPriority,
            &mut info as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<MEMORY_PRIORITY_INFORMATION>() as u32,
        )
        .ok()
        .map(|_| info.MemoryPriority.0 as i32)
    }
}

fn get_boost_enabled(handle: HANDLE) -> Option<bool> {
    unsafe {
        let mut disabled: windows::core::BOOL = Default::default();
        GetProcessPriorityBoost(handle, &mut disabled)
            .ok()
            .map(|_| !disabled.as_bool())
    }
}

fn get_efficiency_mode(handle: HANDLE) -> Option<bool> {
    unsafe {
        let mut state = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: 0,
            StateMask: 0,
        };
        GetProcessInformation(
            handle,
            ProcessPowerThrottling,
            &mut state as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        )
        .ok()
        .map(|_| {
            (state.ControlMask & PROCESS_POWER_THROTTLING_EXECUTION_SPEED) != 0
                && (state.StateMask & PROCESS_POWER_THROTTLING_EXECUTION_SPEED) != 0
        })
    }
}

#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
enum D3dkmtSchedulingPriorityClass {
    Idle = 0,
    BelowNormal = 1,
    Normal = 2,
    AboveNormal = 3,
    High = 4,
    RealTime = 5,
}

#[link(name = "gdi32")]
extern "system" {
    fn D3DKMTGetProcessSchedulingPriorityClass(
        h_process: HANDLE,
        priority_class: *mut D3dkmtSchedulingPriorityClass,
    ) -> i32;

    #[allow(dead_code)]
    fn D3DKMTSetProcessSchedulingPriorityClass(
        h_process: HANDLE,
        priority_class: D3dkmtSchedulingPriorityClass,
    ) -> i32;
}

fn get_gpu_priority(handle: HANDLE) -> Option<i32> {
    unsafe {
        let mut priority_class = D3dkmtSchedulingPriorityClass::Normal;
        let status = D3DKMTGetProcessSchedulingPriorityClass(handle, &mut priority_class);
        (status == 0).then_some(priority_class as i32)
    }
}

fn get_affinity_mask(handle: HANDLE) -> Option<u64> {
    unsafe {
        let mut proc_mask: usize = 0;
        let mut sys_mask: usize = 0;
        GetProcessAffinityMask(handle, &mut proc_mask, &mut sys_mask)
            .ok()
            .map(|_| proc_mask as u64)
    }
}

#[link(name = "ntdll")]
extern "system" {
    fn NtSetInformationProcess(
        process_handle: HANDLE,
        process_information_class: u32,
        process_information: *mut core::ffi::c_void,
        process_information_length: u32,
    ) -> i32;
}

fn cpu_priority_class_for_label(label: &str) -> Option<u32> {
    Some(match label {
        "realtime" => 0x00000100,
        "high" => 0x00000080,
        "abovenormal" => 0x00008000,
        "normal" => 0x00000020,
        "belownormal" => 0x00004000,
        "idle" => 0x00000040,
        _ => return None,
    })
}

unsafe fn with_process_handle<T>(pid: u32, f: impl FnOnce(HANDLE) -> T) -> Option<T> {
    let handle = OpenProcess(PROCESS_ALL_ACCESS, false, pid).ok()?;
    let result = f(handle);
    let _ = CloseHandle(handle);
    Some(result)
}

fn set_cpu_priority_native(pid: u32, label: &str) -> bool {
    let Some(class) = cpu_priority_class_for_label(label) else {
        return false;
    };
    unsafe {
        with_process_handle(pid, |handle| {
            SetPriorityClass(handle, PROCESS_CREATION_FLAGS(class)).is_ok()
        })
        .unwrap_or(false)
    }
}

fn set_affinity_native(pid: u32, mask: u64) -> bool {
    unsafe {
        with_process_handle(pid, |handle| {
            SetProcessAffinityMask(handle, mask as usize).is_ok()
        })
        .unwrap_or(false)
    }
}

fn io_priority_value_for_label(label: &str) -> Option<u32> {
    Some(match label {
        "verylow" => 0,
        "low" => 1,
        "normal" => 2,
        "high" => 3,
        _ => return None,
    })
}

fn set_io_priority_native(pid: u32, label: &str) -> bool {
    let Some(mut value) = io_priority_value_for_label(label) else {
        return false;
    };
    unsafe {
        with_process_handle(pid, |handle| {
            NtSetInformationProcess(
                handle,
                PROCESS_IO_PRIORITY,
                &mut value as *mut _ as *mut core::ffi::c_void,
                std::mem::size_of::<u32>() as u32,
            ) == 0
        })
        .unwrap_or(false)
    }
}

fn mem_priority_value_for_label(label: &str) -> Option<u32> {
    Some(match label {
        "verylow" => 1,
        "low" => 2,
        "medium" => 3,
        "belownormal" => 4,
        "normal" => 5,
        _ => return None,
    })
}

fn set_mem_priority_native(pid: u32, label: &str) -> bool {
    let Some(value) = mem_priority_value_for_label(label) else {
        return false;
    };
    unsafe {
        with_process_handle(pid, |handle| {
            let mut info = MEMORY_PRIORITY_INFORMATION {
                MemoryPriority: MEMORY_PRIORITY(value),
            };
            SetProcessInformation(
                handle,
                ProcessMemoryPriority,
                &mut info as *mut _ as *mut core::ffi::c_void,
                std::mem::size_of::<MEMORY_PRIORITY_INFORMATION>() as u32,
            )
            .is_ok()
        })
        .unwrap_or(false)
    }
}

fn gpu_priority_class_for_label(label: &str) -> Option<D3dkmtSchedulingPriorityClass> {
    Some(match label {
        "idle" => D3dkmtSchedulingPriorityClass::Idle,
        "belownormal" => D3dkmtSchedulingPriorityClass::BelowNormal,
        "normal" => D3dkmtSchedulingPriorityClass::Normal,
        "abovenormal" => D3dkmtSchedulingPriorityClass::AboveNormal,
        "high" => D3dkmtSchedulingPriorityClass::High,
        "realtime" => D3dkmtSchedulingPriorityClass::RealTime,
        _ => return None,
    })
}

fn set_gpu_priority_native(pid: u32, label: &str) -> bool {
    let Some(class) = gpu_priority_class_for_label(label) else {
        return false;
    };
    unsafe {
        with_process_handle(pid, |handle| {
            D3DKMTSetProcessSchedulingPriorityClass(handle, class) == 0
        })
        .unwrap_or(false)
    }
}

fn set_boost_native(pid: u32, enable: bool) -> bool {
    unsafe {
        with_process_handle(pid, |handle| {
            SetProcessPriorityBoost(handle, !enable).is_ok()
        })
        .unwrap_or(false)
    }
}

fn set_efficiency_mode_native(pid: u32, enable: bool) -> bool {
    unsafe {
        with_process_handle(pid, |handle| {
            let priority_class = if enable { 0x00000040 } else { 0x00000020 };
            let priority_ok =
                SetPriorityClass(handle, PROCESS_CREATION_FLAGS(priority_class)).is_ok();

            let mut state = PROCESS_POWER_THROTTLING_STATE {
                Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
                ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
                StateMask: if enable {
                    PROCESS_POWER_THROTTLING_EXECUTION_SPEED
                } else {
                    0
                },
            };
            let throttling_ok = SetProcessInformation(
                handle,
                ProcessPowerThrottling,
                &mut state as *mut _ as *mut core::ffi::c_void,
                std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
            )
            .is_ok();

            priority_ok && throttling_ok
        })
        .unwrap_or(false)
    }
}

fn set_power_plan_native(id: &str) -> bool {
    std::process::Command::new("powercfg")
        .args(["/setactive", id])
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn apply_rule_native(pid: u32, rule_type: &str, value: &str) -> bool {
    match rule_type {
        "cpu_priority" => set_cpu_priority_native(pid, value),
        "cpu_affinity" => value
            .parse::<u64>()
            .map(|mask| set_affinity_native(pid, mask))
            .unwrap_or(false),
        "io_priority" => set_io_priority_native(pid, value),
        "gpu_priority" => set_gpu_priority_native(pid, value),
        "mem_priority" => set_mem_priority_native(pid, value),
        "boost" => set_boost_native(pid, value == "true"),
        "efficiency_mode" => set_efficiency_mode_native(pid, value == "true"),
        "powerplan" => set_power_plan_native(value),
        _ => false,
    }
}

fn terminate_process_native(pid: u32) -> bool {
    unsafe {
        with_process_handle(pid, |handle| TerminateProcess(handle, 1).is_ok()).unwrap_or(false)
    }
}

#[tauri::command]
fn set_cpu_priority(pid: u32, priority: String) -> bool {
    set_cpu_priority_native(pid, &priority)
}

#[tauri::command]
fn set_process_affinity(pid: u32, affinity: u64) -> bool {
    set_affinity_native(pid, affinity)
}

#[tauri::command]
fn set_io_priority(pid: u32, priority: String) -> bool {
    set_io_priority_native(pid, &priority)
}

#[tauri::command]
fn set_gpu_priority(pid: u32, priority: String) -> bool {
    set_gpu_priority_native(pid, &priority)
}

#[tauri::command]
fn set_memory_priority(pid: u32, priority: String) -> bool {
    set_mem_priority_native(pid, &priority)
}

#[tauri::command]
fn set_process_boost(pid: u32, enable: bool) -> bool {
    set_boost_native(pid, enable)
}

#[tauri::command]
fn set_efficiency_mode(pid: u32, enable: bool) -> bool {
    set_efficiency_mode_native(pid, enable)
}

#[tauri::command]
fn set_power_plan(id: String) -> bool {
    set_power_plan_native(&id)
}

#[tauri::command]
fn terminate_process(pid: u32) -> bool {
    terminate_process_native(pid)
}

fn get_process_details(
    pid: u32,
) -> (
    i32,
    Option<u64>,
    Option<i32>,
    Option<i32>,
    Option<bool>,
    Option<bool>,
    Option<i32>,
) {
    let details = unsafe {
        with_process_handle(pid, |handle| {
            let priority_class = GetPriorityClass(handle);
            let priority = match priority_class {
                0x00000040 => 19,
                0x00004000 => 10,
                0x00000020 => 0,
                0x00008000 => -5,
                0x00000080 => -10,
                0x00000100 => -20,
                _ => 0,
            };

            (
                priority,
                get_affinity_mask(handle),
                get_io_priority(handle),
                get_memory_priority(handle),
                get_boost_enabled(handle),
                get_efficiency_mode(handle),
                get_gpu_priority(handle),
            )
        })
    };

    details.unwrap_or((-1, None, None, None, None, None, None))
}

pub struct AppState {
    sys: Mutex<System>,
    static_cache: Mutex<HashMap<u32, u64>>,
}

#[allow(dead_code)]
impl AppState {
    pub fn new() -> Self {
        Self {
            sys: Mutex::new(System::new()),
            static_cache: Mutex::new(HashMap::new()),
        }
    }
}

#[command]
fn reset_process_cache(state: State<AppState>) {
    state.static_cache.lock().unwrap().clear();
}

#[derive(serde::Serialize)]
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

#[command]
fn get_process_list(state: State<AppState>) -> Vec<ProcessInfo> {
    let mut sys = state.sys.lock().unwrap();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_cpu()
            .with_exe(UpdateKind::OnlyIfNotSet),
    );

    let mut cache = state.static_cache.lock().unwrap();

    let live_pids: std::collections::HashSet<u32> =
        sys.processes().keys().map(|p| p.as_u32()).collect();
    cache.retain(|pid, _| live_pids.contains(pid));

    let results: Vec<ProcessInfo> = sys
        .processes()
        .par_iter()
        .map(|(pid, process)| {
            let pid_u32 = pid.as_u32();
            let (
                priority,
                affinity,
                io_priority,
                mem_priority,
                boost,
                efficiency_mode,
                gpu_priority,
            ) = get_process_details(pid_u32);

            ProcessInfo {
                pid: pid_u32 as i32,
                name: None,
                exe_path: None,
                cpu: process.cpu_usage(),
                priority,
                affinity,
                io_priority,
                mem_priority,
                boost,
                efficiency_mode,
                gpu_priority,
            }
        })
        .collect();

    let mut results = results;
    for info in results.iter_mut() {
        let pid_u32 = info.pid as u32;
        let process = match sys.process(sysinfo::Pid::from_u32(pid_u32)) {
            Some(p) => p,
            None => continue,
        };
        let start_time = process.start_time();

        let needs_refresh = match cache.get(&pid_u32) {
            Some(&cached_start_time) => cached_start_time != start_time,
            None => true,
        };

        if needs_refresh {
            cache.insert(pid_u32, start_time);

            info.name = Some(process.name().to_string_lossy().into_owned());
            info.exe_path = process.exe().map(|p| p.to_string_lossy().into_owned());
        }
    }

    results
}

#[command]
fn get_process_by_pid(pid: i32, state: State<AppState>) -> Option<ProcessInfo> {
    let pid_u32 = pid as u32;
    let sys_pid = sysinfo::Pid::from_u32(pid_u32);

    let mut sys = state.sys.lock().unwrap();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[sys_pid]),
        true,
        ProcessRefreshKind::nothing()
            .with_cpu()
            .with_exe(UpdateKind::OnlyIfNotSet),
    );

    let process = sys.process(sys_pid)?;

    let (priority, affinity, io_priority, mem_priority, boost, efficiency_mode, gpu_priority) =
        get_process_details(pid_u32);

    Some(ProcessInfo {
        pid: pid_u32 as i32,
        name: Some(process.name().to_string_lossy().into_owned()),
        exe_path: process.exe().map(|p| p.to_string_lossy().into_owned()),
        cpu: process.cpu_usage(),
        priority,
        affinity,
        io_priority,
        mem_priority,
        boost,
        efficiency_mode,
        gpu_priority,
    })
}

#[derive(Debug, Serialize, Clone)]
pub struct StartupEntry {
    pub name: String,
    pub command: String,
    pub source: String,
    pub enabled: bool,
    pub registry_path: Option<String>,
}

type StateMap = HashMap<String, bool>;

#[tauri::command]
fn get_startup_entries() -> Vec<StartupEntry> {
    let read_run_key =
        |hive: HKEY, path: &str, source: &str, default_enabled: bool| -> Vec<StartupEntry> {
            let key = match RegKey::predef(hive).open_subkey(path) {
                Ok(k) => k,
                Err(_) => return vec![],
            };

            let hive_name = if hive == HKEY_CURRENT_USER {
                "HKCU"
            } else if hive == HKEY_LOCAL_MACHINE {
                "HKLM"
            } else {
                "HKEY"
            };
            let full_path = format!(r"{}\{}", hive_name, path);

            key.enum_values()
                .filter_map(|r| r.ok())
                .filter_map(|(name, value): (String, RegValue)| {
                    let command = match value.vtype {
                        RegType::REG_SZ | RegType::REG_EXPAND_SZ => {
                            let wide_chars: Vec<u16> = value
                                .bytes
                                .chunks_exact(2)
                                .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
                                .collect();
                            match String::from_utf16(&wide_chars) {
                                Ok(s) => s.trim_end_matches('\0').to_string(),
                                Err(_) => return None,
                            }
                        }
                        _ => return None,
                    };

                    if command.is_empty() {
                        return None;
                    }
                    Some(StartupEntry {
                        name,
                        command,
                        source: source.to_string(),
                        enabled: default_enabled,
                        registry_path: Some(full_path.clone()),
                    })
                })
                .collect()
        };

    let read_startup_folder = |path: &str, source: &str| -> Vec<StartupEntry> {
        let mut results = vec![];
        let mut dirs_to_visit = vec![std::path::PathBuf::from(path)];

        while let Some(dir_path) = dirs_to_visit.pop() {
            let dir = match std::fs::read_dir(&dir_path) {
                Ok(d) => d,
                Err(_) => continue,
            };

            for entry in dir.filter_map(|e| e.ok()) {
                let entry_path = entry.path();

                if entry_path.is_dir() {
                    dirs_to_visit.push(entry_path);
                    continue;
                }

                let is_ini = entry_path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| ext.eq_ignore_ascii_case("ini"))
                    .unwrap_or(false);
                if is_ini {
                    continue;
                }

                let path_str = entry_path.to_string_lossy();
                let enabled = !(path_str.contains("AutorunsDisabled")
                    || path_str.contains("WinProcessDisabled"));

                results.push(StartupEntry {
                    name: entry.file_name().to_string_lossy().into_owned(),
                    command: entry_path.to_string_lossy().into_owned(),
                    source: source.to_string(),
                    enabled,
                    registry_path: None, // not a registry entry
                });
            }
        }

        results
    };

    let mut entries: Vec<StartupEntry> = vec![];

    // Registry Run keys (enabled by default)
    entries.extend(read_run_key(
        HKEY_CURRENT_USER,
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run",
        "Registry",
        true,
    ));
    entries.extend(read_run_key(
        HKEY_LOCAL_MACHINE,
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run",
        "Registry",
        true,
    ));

    entries.extend(read_run_key(
        HKEY_LOCAL_MACHINE,
        r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Run",
        "Registry",
        true,
    ));

    // set already by default
    entries.extend(read_run_key(
        HKEY_LOCAL_MACHINE,
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run\WinProcessDisabled",
        "Registry",
        false,
    ));
    entries.extend(read_run_key(
        HKEY_CURRENT_USER,
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run\WinProcessDisabled",
        "Registry",
        false,
    ));
    entries.extend(read_run_key(
        HKEY_LOCAL_MACHINE,
        r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Run\WinProcessDisabled",
        "Registry",
        false,
    ));
    entries.extend(read_run_key(
        HKEY_CURRENT_USER,
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run\AutorunsDisabled",
        "Registry",
        false,
    ));
    entries.extend(read_run_key(
        HKEY_LOCAL_MACHINE,
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run\AutorunsDisabled",
        "Registry",
        false,
    ));
    entries.extend(read_run_key(
        HKEY_LOCAL_MACHINE,
        r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Run\AutorunsDisabled",
        "Registry",
        false,
    ));

    // Startup folders
    if let Ok(appdata) = std::env::var("APPDATA") {
        entries.extend(read_startup_folder(
            &format!(r"{}\Microsoft\Windows\Start Menu\Programs\Startup", appdata),
            "User Folder",
        ));
    }
    entries.extend(read_startup_folder(
        r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Startup",
        "Startup Folder",
    ));

    // Startup enabled/disabled state overlay (from StartupApproved\Run)
    let states: StateMap = {
        let key = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run");
        match key {
            Ok(k) => k
                .enum_values()
                .filter_map(|r| r.ok())
                .map(|(name, value): (String, RegValue)| {
                    let enabled = value.bytes.first() == Some(&2);
                    (name, enabled)
                })
                .collect(),
            Err(_) => HashMap::new(),
        }
    };

    for entry in &mut entries {
        if entry.source == "Registry" && entry.enabled {
            if let Some(state) = states.get(&entry.name) {
                entry.enabled = *state;
            }
        }
    }

    entries
}

#[derive(Clone, serde::Serialize)]
pub struct ScheduledTaskInfo {
    pub name: String,
    pub path: String,
    pub state: String,
    pub enabled: bool,
    pub author: String,
    pub last_run_time: Option<String>,
}

struct ComGuard;
impl Drop for ComGuard {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

fn ole_date_to_iso(ole: f64) -> Option<String> {
    if ole == 0.0 {
        return None;
    }
    let epoch = NaiveDate::from_ymd_opt(1899, 12, 30)?.and_hms_opt(0, 0, 0)?;
    let days = ole.trunc() as i64;
    let millis_in_day = (ole.fract().abs() * 86_400_000.0).round() as i64;
    let dt = epoch + Duration::days(days) + Duration::milliseconds(millis_in_day);
    Some(dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
}

fn extract_xml_field(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].trim().to_string())
}

static AUTHOR_CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

fn author_cache() -> &'static Mutex<HashMap<String, String>> {
    AUTHOR_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn prune_author_cache(valid_paths: &[String]) {
    let mut cache = author_cache().lock().unwrap();
    let valid: std::collections::HashSet<&str> = valid_paths.iter().map(|s| s.as_str()).collect();
    cache.retain(|path, _| valid.contains(path.as_str()));
}

unsafe fn get_author_cached(
    task: &windows::Win32::System::TaskScheduler::IRegisteredTask,
    path: &str,
) -> String {
    if let Some(a) = author_cache().lock().unwrap().get(path) {
        return a.clone();
    }
    let author = task
        .Xml()
        .ok()
        .map(|b| b.to_string())
        .and_then(|xml| extract_xml_field(&xml, "Author"))
        .unwrap_or_default();
    author_cache()
        .lock()
        .unwrap()
        .insert(path.to_string(), author.clone());
    author
}

fn child_task_path(folder_path: &str, child_name: &str) -> String {
    if folder_path == "\\" {
        format!("\\{}", child_name)
    } else {
        format!("{}\\{}", folder_path, child_name)
    }
}

unsafe fn collect_tasks(
    folder: &ITaskFolder,
    folder_path: &str,
    out: &mut Vec<ScheduledTaskInfo>,
) -> WinResult<()> {
    let tasks = folder.GetTasks(0)?;
    let count = tasks.Count()?;
    for i in 1..=count {
        let task = tasks.get_Item(&windows::Win32::System::Variant::VARIANT::from(i))?;
        let name = task.Name()?.to_string();
        let state = format!("{:?}", task.State()?);
        let enabled = task.Enabled()?.as_bool();
        let last_run_time = task.LastRunTime().ok().and_then(ole_date_to_iso);

        let path = child_task_path(folder_path, &name);
        let author = get_author_cached(&task, &path);

        out.push(ScheduledTaskInfo {
            name,
            path,
            state,
            enabled,
            author,
            last_run_time,
        });
    }

    let subfolders = folder.GetFolders(0)?;
    let sub_count = subfolders.Count()?;
    for i in 1..=sub_count {
        let sub: ITaskFolder =
            subfolders.get_Item(&windows::Win32::System::Variant::VARIANT::from(i))?;
        let sub_name = sub.Name()?.to_string();
        let sub_path = child_task_path(folder_path, &sub_name);
        collect_tasks(&sub, &sub_path, out)?;
    }

    Ok(())
}

fn list_tasks(service: &ITaskService) -> WinResult<Vec<ScheduledTaskInfo>> {
    unsafe {
        let root = service.GetFolder(&BSTR::from("\\"))?;
        let mut out = Vec::with_capacity(64);
        collect_tasks(&root, "\\", &mut out)?;
        let paths: Vec<String> = out.iter().map(|t| t.path.clone()).collect();
        prune_author_cache(&paths);
        Ok(out)
    }
}

enum TaskRequest {
    GetCached(mpsc::Sender<Result<Vec<ScheduledTaskInfo>, String>>),
    ForceRefresh(mpsc::Sender<Result<Vec<ScheduledTaskInfo>, String>>),
}

static TASK_CACHE: OnceLock<RwLock<Vec<ScheduledTaskInfo>>> = OnceLock::new();

fn task_cache() -> &'static RwLock<Vec<ScheduledTaskInfo>> {
    TASK_CACHE.get_or_init(|| RwLock::new(Vec::new()))
}

static WORKER: OnceLock<mpsc::Sender<TaskRequest>> = OnceLock::new();

fn worker_handle() -> &'static mpsc::Sender<TaskRequest> {
    WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<TaskRequest>();

        std::thread::spawn(move || {
            unsafe {
                if CoInitializeEx(None, COINIT_MULTITHREADED).is_err() {
                    return;
                }
            }
            let _guard = ComGuard;

            let service: ITaskService =
                match unsafe { CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER) } {
                    Ok(s) => s,
                    Err(_) => return,
                };
            let empty = windows::Win32::System::Variant::VARIANT::default();
            if unsafe { service.Connect(&empty, &empty, &empty, &empty) }.is_err() {
                return;
            }

            if let Ok(initial) = list_tasks(&service) {
                *task_cache().write().unwrap() = initial;
            }

            let refresh_interval = std::time::Duration::from_secs(2);
            let mut last_refresh = std::time::Instant::now();

            loop {
                match rx.recv_timeout(refresh_interval) {
                    Ok(TaskRequest::GetCached(reply)) => {
                        let _ = reply.send(Ok(task_cache().read().unwrap().clone()));
                    }
                    Ok(TaskRequest::ForceRefresh(reply)) => {
                        let result = list_tasks(&service).map_err(|e| e.to_string());
                        if let Ok(ref data) = result {
                            *task_cache().write().unwrap() = data.clone();
                        }
                        last_refresh = std::time::Instant::now();
                        let _ = reply.send(result);
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if last_refresh.elapsed() >= refresh_interval {
                            if let Ok(data) = list_tasks(&service) {
                                *task_cache().write().unwrap() = data;
                            }
                            last_refresh = std::time::Instant::now();
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });

        tx
    })
}

#[tauri::command]
async fn get_scheduled_tasks() -> Result<Vec<ScheduledTaskInfo>, String> {
    let (reply_tx, reply_rx) = mpsc::channel();
    worker_handle()
        .send(TaskRequest::GetCached(reply_tx))
        .map_err(|e| e.to_string())?;

    tauri::async_runtime::spawn_blocking(move || reply_rx.recv().map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())??
}

#[tauri::command]
async fn refresh_scheduled_tasks() -> Result<Vec<ScheduledTaskInfo>, String> {
    let (reply_tx, reply_rx) = mpsc::channel();
    worker_handle()
        .send(TaskRequest::ForceRefresh(reply_tx))
        .map_err(|e| e.to_string())?;

    tauri::async_runtime::spawn_blocking(move || reply_rx.recv().map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())??
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
fn get_system_stats(state: State<'_, AppState>) -> SystemStats {
    let mut sys = state.sys.lock().unwrap();

    sys.refresh_cpu_usage();
    sys.refresh_memory();

    let cpu_usage = sys.global_cpu_usage();
    let ram_total = sys.total_memory();
    let ram_used = sys.used_memory();
    let os_name = System::long_os_version().unwrap_or_else(|| "Windows".to_string());
    let ram_percentage = if ram_total > 0 {
        (ram_used as f32 / ram_total as f32) * 100.0
    } else {
        0.0
    };
    let uptime = System::uptime();

    SystemStats {
        cpu_usage,
        ram_total,
        ram_used,
        ram_percentage,
        uptime,
        os_name,
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct PowerPlan {
    pub id: String,
    pub name: String,
    pub active: bool,
}

#[tauri::command]
fn get_power_plans() -> Vec<PowerPlan> {
    let output = match std::process::Command::new("powercfg")
        .args(["/list"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        Ok(o) => o,
        Err(_) => return vec![],
    };

    if !output.status.success() {
        return vec![];
    }

    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");

    stdout
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix("Power Scheme GUID:")?.trim();

            let (id, tail) = rest.split_once(' ')?;
            let id = id.trim().to_string();

            let mut tail = tail.trim();

            let active = tail.ends_with('*');

            if active {
                tail = tail.strip_suffix('*')?.trim();
            }

            let name = tail
                .strip_prefix('(')
                .and_then(|s| s.strip_suffix(')'))
                .unwrap_or(tail)
                .trim()
                .to_string();

            Some(PowerPlan { id, name, active })
        })
        .collect()
}

struct RuleEngine {
    sys: System,
    known_names: HashMap<u32, String>,
    applied_rules: HashMap<u32, HashMap<String, String>>,
    applied_power_plans: Vec<(String, String)>,
    current_plan_id: Option<String>,
    has_resetted_to_default: bool,
    default_plan: Option<String>,
}

impl RuleEngine {
    fn new() -> Self {
        Self {
            sys: System::new(),
            known_names: HashMap::new(),
            applied_rules: HashMap::new(),
            applied_power_plans: Vec::new(),
            current_plan_id: None,
            has_resetted_to_default: true,
            default_plan: None,
        }
    }

    fn tick(&mut self, app: &tauri::AppHandle) {
        self.sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing(),
        );

        let live_pids: HashSet<u32> = self.sys.processes().keys().map(|p| p.as_u32()).collect();

        self.known_names.retain(|pid, _| live_pids.contains(pid));
        self.applied_rules.retain(|pid, _| live_pids.contains(pid));

        for (pid, process) in self.sys.processes() {
            let pid_u32 = pid.as_u32();
            self.known_names
                .entry(pid_u32)
                .or_insert_with(|| process.name().to_string_lossy().into_owned());
        }

        let Ok(store) = app.store("always.json") else {
            return;
        };

        let current_names: HashSet<&str> = self.known_names.values().map(|s| s.as_str()).collect();

        if self.default_plan.is_none() {
            if let Some(active) = get_power_plans().into_iter().find(|p| p.active) {
                self.default_plan = Some(active.id);
            }
        }

        let previously_applied = std::mem::take(&mut self.applied_power_plans);
        for (name, plan_id) in previously_applied {
            if !current_names.contains(name.as_str()) {
                continue;
            }
            if self.current_plan_id.as_deref() != Some(plan_id.as_str()) {
                set_power_plan_native(&plan_id);
                self.current_plan_id = Some(plan_id.clone());
                self.has_resetted_to_default = false;
            }
            self.applied_power_plans.push((name, plan_id));
        }

        if !self.has_resetted_to_default && self.applied_power_plans.is_empty() {
            if let Some(default_plan) = self.default_plan.clone() {
                if self.current_plan_id.as_deref() != Some(default_plan.as_str()) {
                    set_power_plan_native(&default_plan);
                    self.current_plan_id = Some(default_plan);
                }
            }
            self.has_resetted_to_default = true;
        }

        for (&pid, name) in self.known_names.iter() {
            let prefix = format!("{}_", name);
            let applied_for_pid = self.applied_rules.entry(pid).or_default();

            for key in store.keys() {
                if !key.starts_with(&prefix) {
                    continue;
                }
                let Some(value) = store.get(&key) else {
                    continue;
                };
                let rule_type = value.get("type").and_then(|v| v.as_str());
                let rule_value_str = value.get("value").and_then(|v| {
                    if v.is_number() {
                        Some(v.to_string())
                    } else {
                        v.as_str().map(String::from)
                    }
                });

                let (Some(r_type), Some(r_val)) = (rule_type, rule_value_str) else {
                    continue;
                };

                if applied_for_pid.get(&key).map_or(false, |v| v == &r_val) {
                    continue;
                }

                apply_rule_native(pid, r_type, &r_val);
                applied_for_pid.insert(key.clone(), r_val.clone());

                if r_type == "powerplan" {
                    self.applied_power_plans.push((name.clone(), r_val.clone()));
                    self.current_plan_id = Some(r_val);
                    self.has_resetted_to_default = false;
                }
            }
        }
    }
}

fn rule_engine_poll_interval(app: &tauri::AppHandle) -> u64 {
    app.store("settings.json")
        .ok()
        .and_then(|store| store.get("polling"))
        .and_then(|v| v.as_u64())
        .unwrap_or(2000)
}

fn spawn_rule_engine(app_handle: tauri::AppHandle) {
    std::thread::spawn(move || {
        let mut engine = RuleEngine::new();
        loop {
            engine.tick(&app_handle);
            let interval = rule_engine_poll_interval(&app_handle);
            std::thread::sleep(std::time::Duration::from_millis(interval));
        }
    });
}

fn create_main_window<R: Runtime>(app: &impl Manager<R>) -> tauri::Result<WebviewWindow<R>> {
    WebviewWindowBuilder::new(app, "main", WebviewUrl::App(Default::default()))
        .title("WinProcess")
        .inner_size(1200.0, 776.0)
        .min_inner_size(750.0, 650.0)
        .resizable(true)
        .visible(false)
        .decorations(false)
        .center()
        .fullscreen(false)
        .build()
}

const AUTOSTART_TASK_NAME: &str = "WinProcessAutostart";

fn with_com<T>(f: impl FnOnce() -> windows::core::Result<T>) -> Result<T, String> {
    unsafe {
        let init = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        let we_initialized = init.is_ok();
        let already_fine = init == S_FALSE || init.0 == RPC_E_CHANGED_MODE.0;

        if init.is_err() && !already_fine {
            return Err(format!("CoInitializeEx failed: {init:?}"));
        }

        let result = f();

        if we_initialized {
            CoUninitialize();
        }

        result.map_err(|e| format!("{e:?}"))
    }
}

fn task_service() -> windows::core::Result<ITaskService> {
    unsafe {
        let service: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)?;
        service.Connect(
            &VARIANT::default(),
            &VARIANT::default(),
            &VARIANT::default(),
            &VARIANT::default(),
        )?;
        Ok(service)
    }
}

fn root_folder(service: &ITaskService) -> windows::core::Result<ITaskFolder> {
    unsafe { service.GetFolder(&BSTR::from("\\")) }
}

fn get_existing_task(folder: &ITaskFolder) -> windows::core::Result<Option<IRegisteredTask>> {
    unsafe {
        match folder.GetTask(&BSTR::from(AUTOSTART_TASK_NAME)) {
            Ok(task) => Ok(Some(task)),
            Err(e) if e.code() == HRESULT::from_win32(ERROR_FILE_NOT_FOUND.0) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

#[tauri::command]
fn enable_autostart() -> Result<(), String> {
    let exe_path = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .to_string_lossy()
        .into_owned();

    with_com(|| unsafe {
        let service = task_service()?;
        let folder = root_folder(&service)?;
        let task_def: ITaskDefinition = service.NewTask(0)?;

        let reg_info: IRegistrationInfo = task_def.RegistrationInfo()?;
        reg_info.SetAuthor(&BSTR::from("WinProcess"))?;
        reg_info.SetDescription(&BSTR::from(
            "Launch WinProcess at logon with elevated privileges.",
        ))?;

        let principal: IPrincipal = task_def.Principal()?;
        principal.SetRunLevel(TASK_RUNLEVEL_HIGHEST)?;

        let settings: ITaskSettings = task_def.Settings()?;
        settings.SetStartWhenAvailable(true.into())?;
        settings.SetDisallowStartIfOnBatteries(false.into())?;
        settings.SetStopIfGoingOnBatteries(false.into())?;

        let triggers: ITriggerCollection = task_def.Triggers()?;
        let trigger: ITrigger = triggers.Create(TASK_TRIGGER_LOGON)?;
        let logon_trigger: ILogonTrigger = trigger.cast()?;
        let username = std::env::var("USERNAME").unwrap_or_default();
        logon_trigger.SetUserId(&BSTR::from(username.as_str()))?;
        logon_trigger.SetEnabled(true.into())?;

        let actions: IActionCollection = task_def.Actions()?;
        let action: IAction = actions.Create(TASK_ACTION_EXEC)?;
        let exec_action: IExecAction = action.cast()?;
        exec_action.SetPath(&BSTR::from(exe_path.as_str()))?;
        exec_action.SetArguments(&BSTR::from("--tray"))?;

        folder.RegisterTaskDefinition(
            &BSTR::from(AUTOSTART_TASK_NAME),
            &task_def,
            TASK_CREATE_OR_UPDATE.0,
            &VARIANT::default(),
            &VARIANT::default(),
            TASK_LOGON_INTERACTIVE_TOKEN,
            &VARIANT::default(),
        )?;

        Ok(())
    })
}

#[tauri::command]
fn disable_autostart() -> Result<(), String> {
    with_com(|| unsafe {
        let service = task_service()?;
        let folder = root_folder(&service)?;
        if get_existing_task(&folder)?.is_some() {
            folder.DeleteTask(&BSTR::from(AUTOSTART_TASK_NAME), 0)?;
        }
        Ok(())
    })
}

#[tauri::command]
fn is_autostart_enabled() -> Result<bool, String> {
    with_com(|| unsafe {
        let service = task_service()?;
        let folder = root_folder(&service)?;
        match get_existing_task(&folder)? {
            Some(task) => Ok(task.Enabled()?.as_bool()),
            None => Ok(false),
        }
    })
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            } else {
                let _ = create_main_window(app);
            }
        }))
        .plugin(tauri_plugin_autostart::Builder::new().build())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_os::init())
        .setup(|app| {
            for privilege in ["SeDebugPrivilege", "SeIncreaseBasePriorityPrivilege"] {
                if let Err(e) = enable_privilege(privilege) {
                    eprintln!("Failed to enable {privilege}: {:?}", e);
                }
            }

            let settings = app.store("settings.json")?;
            app.store("always.json")?;

            // first time default settings
            if settings.get("startup").is_none() {
                match enable_autostart() {
                    Ok(_) => {
                        settings.set("startup", true);
                    }
                    Err(err) => {
                        eprintln!("Failed to enable autostart: {err}");
                    }
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

            let launched_via_autostart = std::env::args().any(|arg| arg == "--tray");
            if !launched_via_autostart {
                create_main_window(app)?;
            }

            let show_i = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &quit_i])?;

            TrayIconBuilder::with_id("main-tray")
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("WinProcess")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => {
                        app.exit(0);
                    }
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        } else if let Ok(_window) = create_main_window(app) {
                            // show set by js
                        }
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::DoubleClick { .. } = event {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        } else if let Ok(_window) = create_main_window(app) {
                            // show set by js
                        }
                    }
                })
                .build(app)?;

            Ok(())
        })
        .manage(AppState {
            sys: Mutex::new(System::new_all()),
            static_cache: Mutex::new(HashMap::new()),
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
        .run(|_app_handle, event| {
            if let RunEvent::ExitRequested { code, api, .. } = event {
                if code.is_none() {
                    api.prevent_exit();
                }
            }
        });
}
