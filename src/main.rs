#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use hidapi::HidApi;
use std::collections::VecDeque;
use std::ffi::c_void;
use std::fs::{self, OpenOptions};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SETTINGS_PATH: &str = r"D:\Apps\Armory VC\settings.cfg";
const WAKEUP_SIGNAL_PATH: &str = r"D:\Apps\Armory VC\.wakeup";
const WINDOW_TITLE: &str = "ROG Control Center v4.2";

static MAIN_HWND: AtomicUsize = AtomicUsize::new(0);
static WINDOW_HIDDEN: AtomicBool = AtomicBool::new(false);
static PIN_ALWAYS_ON_TOP: AtomicBool = AtomicBool::new(true);
static TRAY_ACTION: Mutex<Option<usize>> = Mutex::new(None);
static HOTKEY_EVENT: Mutex<Option<usize>> = Mutex::new(None);
static HID_WRITE_LOCK: Mutex<()> = Mutex::new(());

#[repr(C)]
struct MemoryStatusEx {
    dw_length: u32,
    dw_memory_load: u32,
    ull_total_phys: u64,
    ull_avail_phys: u64,
    ull_total_page_file: u64,
    ull_avail_page_file: u64,
    ull_total_virtual: u64,
    ull_avail_virtual: u64,
    ull_avail_extended_virtual: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FileTime {
    dw_low_date_time: u32,
    dw_high_date_time: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SystemPowerStatus {
    ac_line_status: u8,
    battery_flag: u8,
    battery_life_percent: u8,
    system_status_flag: u8,
    battery_life_time: u32,
    battery_full_life_time: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SystemBatteryState {
    ac_on_line: u8,
    battery_present: u8,
    charging: u8,
    discharging: u8,
    spare1: [u8; 3],
    tag: u8,
    max_capacity: u32,
    remaining_capacity: u32,
    rate: i32,
    estimated_time: u32,
    default_alert1: u32,
    default_alert2: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LastInputInfo {
    cb_size: u32,
    dw_time: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct DevModeW {
    dm_device_name: [u16; 32],
    dm_spec_version: u16,
    dm_driver_version: u16,
    dm_size: u16,
    dm_driver_extra: u16,
    dm_fields: u32,
    dm_union1: [u8; 16],
    dm_color: i16,
    dm_duplex: i16,
    dm_y_resolution: i16,
    dm_tt_option: i16,
    dm_collate: i16,
    dm_form_name: [u16; 32],
    dm_log_pixels: u16,
    dm_bits_per_pel: u32,
    dm_pels_width: u32,
    dm_pels_height: u32,
    dm_display_flags: u32,
    dm_display_frequency: u32,
    dm_padding: [u32; 8],
}

impl Default for DevModeW {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

#[repr(C)]
struct NotifyIconDataW {
    cb_size: u32,
    h_wnd: *mut c_void,
    u_id: u32,
    u_flags: u32,
    u_callback_message: u32,
    h_icon: *mut c_void,
    sz_tip: [u16; 128],
    dw_state: u32,
    dw_state_mask: u32,
    sz_info: [u16; 256],
    u_timeout_or_version: u32,
    sz_info_title: [u16; 64],
    dw_info_flags: u32,
    guid_item: Guid,
    h_balloon_icon: *mut c_void,
}

#[repr(C)]
struct WndClassW {
    style: u32,
    lpfn_wnd_proc: unsafe extern "system" fn(*mut c_void, u32, usize, isize) -> isize,
    cb_cls_extra: i32,
    cb_wnd_extra: i32,
    h_instance: *mut c_void,
    h_icon: *mut c_void,
    h_cursor: *mut c_void,
    hbr_background: *mut c_void,
    lpsz_menu_name: *const u16,
    lpsz_class_name: *const u16,
}

#[repr(C)]
struct Msg {
    hwnd: *mut c_void,
    message: u32,
    w_param: usize,
    l_param: isize,
    time: u32,
    pt: Point,
}

#[link(name = "kernel32")]
extern "system" {
    fn DeviceIoControl(
        hDevice: *mut c_void,
        dwIoControlCode: u32,
        lpInBuffer: *const u8,
        nInBufferSize: u32,
        lpOutBuffer: *mut u8,
        nOutBufferSize: u32,
        lpBytesReturned: *mut u32,
        lpOverlapped: *mut c_void,
    ) -> i32;
    fn GlobalMemoryStatusEx(lpBuffer: *mut MemoryStatusEx) -> i32;
    fn GetSystemTimes(
        lpIdleTime: *mut FileTime,
        lpKernelTime: *mut FileTime,
        lpUserTime: *mut FileTime,
    ) -> i32;
    fn GetSystemPowerStatus(lpSystemPowerStatus: *mut SystemPowerStatus) -> i32;
    fn GetTickCount() -> u32;
    fn GetCurrentThreadId() -> u32;
    fn LoadLibraryA(lpLibFileName: *const u8) -> *mut c_void;
    fn GetProcAddress(hModule: *mut c_void, lpProcName: *const u8) -> *mut c_void;
}

#[link(name = "powrprof")]
extern "system" {
    fn CallNtPowerInformation(
        information_level: i32,
        input_buffer: *const c_void,
        input_buffer_length: u32,
        output_buffer: *mut c_void,
        output_buffer_length: u32,
    ) -> i32;
}

#[link(name = "user32")]
extern "system" {
    fn FindWindowW(lpClassName: *const u16, lpWindowName: *const u16) -> *mut c_void;
    fn ShowWindow(hWnd: *mut c_void, nCmdShow: i32) -> i32;
    fn SetForegroundWindow(hWnd: *mut c_void) -> i32;
    fn GetForegroundWindow() -> *mut c_void;
    fn GetWindowThreadProcessId(hWnd: *mut c_void, lpdwProcessId: *mut u32) -> u32;
    fn AttachThreadInput(idAttach: u32, idAttachTo: u32, fAttach: i32) -> i32;
    fn BringWindowToTop(hWnd: *mut c_void) -> i32;
    fn SetWindowPos(
        hWnd: *mut c_void,
        hWndInsertAfter: *mut c_void,
        X: i32,
        Y: i32,
        cx: i32,
        cy: i32,
        uFlags: u32,
    ) -> i32;
    fn PostMessageW(hWnd: *mut c_void, Msg: u32, wParam: usize, lParam: isize) -> i32;
    fn GetLastInputInfo(plii: *mut LastInputInfo) -> i32;
    fn EnumDisplaySettingsW(
        lpszDeviceName: *const u16,
        iModeNum: u32,
        lpDevMode: *mut DevModeW,
    ) -> i32;
    fn ChangeDisplaySettingsW(lpDevMode: *mut DevModeW, dwFlags: u32) -> i32;
    fn RegisterClassW(lpWndClass: *const WndClassW) -> u16;
    fn CreateWindowExW(
        dwExStyle: u32,
        lpClassName: *const u16,
        lpWindowName: *const u16,
        dwStyle: u32,
        x: i32,
        y: i32,
        nWidth: i32,
        nHeight: i32,
        hWndParent: *mut c_void,
        hMenu: *mut c_void,
        hInstance: *mut c_void,
        lpParam: *mut c_void,
    ) -> *mut c_void;
    fn DefWindowProcW(hWnd: *mut c_void, msg: u32, wParam: usize, lParam: isize) -> isize;
    fn GetMessageW(lpMsg: *mut Msg, hWnd: *mut c_void, wMsgFilterMin: u32, wMsgFilterMax: u32) -> i32;
    fn TranslateMessage(lpMsg: *const Msg) -> i32;
    fn DispatchMessageW(lpMsg: *const Msg) -> isize;
    fn CreatePopupMenu() -> *mut c_void;
    fn AppendMenuW(hMenu: *mut c_void, uFlags: u32, uIDNewItem: usize, lpNewItem: *const u16) -> i32;
    fn TrackPopupMenu(
        hMenu: *mut c_void,
        uFlags: u32,
        x: i32,
        y: i32,
        nReserved: i32,
        hWnd: *mut c_void,
        prcRect: *const c_void,
    ) -> i32;
    fn DestroyMenu(hMenu: *mut c_void) -> i32;
    fn GetCursorPos(lpPoint: *mut Point) -> i32;
    fn LoadImageW(
        hInst: *mut c_void,
        name: *const u16,
        type_: u32,
        cx: i32,
        cy: i32,
        fuLoad: u32,
    ) -> *mut c_void;
    fn LoadIconW(hInstance: *mut c_void, lpIconName: *const u16) -> *mut c_void;
    fn keybd_event(bVk: u8, bScan: u8, dwFlags: u32, dwExtraInfo: usize);
    fn RegisterHotKey(hWnd: *mut c_void, id: i32, fsModifiers: u32, vk: u32) -> i32;
}

#[link(name = "shell32")]
extern "system" {
    fn Shell_NotifyIconW(dwMessage: u32, lpData: *const NotifyIconDataW) -> i32;
}

#[repr(C)]
struct NvmlUtilization {
    gpu: u32,
    memory: u32,
}

#[repr(C)]
struct NvmlMemory {
    total: u64,
    free: u64,
    used: u64,
}

struct NvmlReader {
    dev: *mut c_void,
    fn_temp: Option<unsafe extern "C" fn(*mut c_void, u32, *mut u32) -> i32>,
    fn_clock: Option<unsafe extern "C" fn(*mut c_void, u32, *mut u32) -> i32>,
    fn_power: Option<unsafe extern "C" fn(*mut c_void, *mut u32) -> i32>,
    fn_util: Option<unsafe extern "C" fn(*mut c_void, *mut NvmlUtilization) -> i32>,
    fn_mem: Option<unsafe extern "C" fn(*mut c_void, *mut NvmlMemory) -> i32>,
}

impl NvmlReader {
    fn new() -> Option<Self> {
        unsafe {
            let mut lib = LoadLibraryA(b"nvml.dll\0".as_ptr());
            if lib.is_null() {
                lib = LoadLibraryA(b"C:\\Windows\\System32\\nvml.dll\0".as_ptr());
            }
            if lib.is_null() {
                return None;
            }

            let init_ptr = GetProcAddress(lib, b"nvmlInit_v2\0".as_ptr());
            let get_dev_ptr = GetProcAddress(lib, b"nvmlDeviceGetHandleByIndex_v2\0".as_ptr());
            if init_ptr.is_null() || get_dev_ptr.is_null() {
                return None;
            }

            let nvml_init: unsafe extern "C" fn() -> i32 = std::mem::transmute(init_ptr);
            let nvml_get_dev: unsafe extern "C" fn(u32, *mut *mut c_void) -> i32 =
                std::mem::transmute(get_dev_ptr);

            if nvml_init() != 0 {
                return None;
            }
            let mut dev: *mut c_void = std::ptr::null_mut();
            if nvml_get_dev(0, &mut dev) != 0 || dev.is_null() {
                return None;
            }

            let load_fn = |name: &[u8]| -> *mut c_void { GetProcAddress(lib, name.as_ptr()) };

            let p_temp = load_fn(b"nvmlDeviceGetTemperature\0");
            let p_clock = load_fn(b"nvmlDeviceGetClockInfo\0");
            let p_power = load_fn(b"nvmlDeviceGetPowerUsage\0");
            let p_util = load_fn(b"nvmlDeviceGetUtilizationRates\0");
            let p_mem = load_fn(b"nvmlDeviceGetMemoryInfo\0");

            Some(Self {
                dev,
                fn_temp: if !p_temp.is_null() { Some(std::mem::transmute(p_temp)) } else { None },
                fn_clock: if !p_clock.is_null() { Some(std::mem::transmute(p_clock)) } else { None },
                fn_power: if !p_power.is_null() { Some(std::mem::transmute(p_power)) } else { None },
                fn_util: if !p_util.is_null() { Some(std::mem::transmute(p_util)) } else { None },
                fn_mem: if !p_mem.is_null() { Some(std::mem::transmute(p_mem)) } else { None },
            })
        }
    }

    fn query(&self) -> (Option<u32>, String, String, String, String, String) {
        unsafe {
            let mut temp = None;
            if let Some(f) = self.fn_temp {
                let mut t = 0u32;
                if f(self.dev, 0, &mut t) == 0 && t > 0 && t < 120 {
                    temp = Some(t);
                }
            }

            let mut core_clock = "-".to_string();
            let mut mem_clock = "-".to_string();
            if let Some(f) = self.fn_clock {
                let mut c = 0u32;
                if f(self.dev, 0, &mut c) == 0 {
                    core_clock = format!("{} MHz", c);
                }
                let mut m = 0u32;
                if f(self.dev, 2, &mut m) == 0 {
                    mem_clock = format!("{} MHz", m);
                }
            }

            let mut power = "-".to_string();
            if let Some(f) = self.fn_power {
                let mut mw = 0u32;
                if f(self.dev, &mut mw) == 0 {
                    power = format!("{:.1} W", (mw as f32) / 1000.0);
                }
            }

            let mut util = "0 %".to_string();
            if let Some(f) = self.fn_util {
                let mut u = NvmlUtilization { gpu: 0, memory: 0 };
                if f(self.dev, &mut u) == 0 {
                    util = format!("{} %", u.gpu);
                }
            }

            let mut vram = "-".to_string();
            if let Some(f) = self.fn_mem {
                let mut mem = NvmlMemory { total: 0, free: 0, used: 0 };
                if f(self.dev, &mut mem) == 0 && mem.total > 0 {
                    vram = format!(
                        "{} / {} MB",
                        mem.used / (1024 * 1024),
                        mem.total / (1024 * 1024)
                    );
                }
            }

            (temp, core_clock, mem_clock, power, util, vram)
        }
    }
}

const ATK_IOCTL: u32 = 0x0022240C;
const DEV_GPU_ECO: u32 = 0x00090020;
const DEV_PERF_MODE: u32 = 0x00120075;
const DEV_BATTERY_LIMIT: u32 = 0x00120057;

const DEV_CPU_TEMP: u32 = 0x00120094;
const DEV_GPU_TEMP: u32 = 0x00120097;
const DEV_CPU_FAN_RPM: u32 = 0x00110013;
const DEV_GPU_FAN_RPM: u32 = 0x00110014;
const DEV_CPU_FAN_CURVE: u32 = 0x00110024;
const DEV_GPU_FAN_CURVE: u32 = 0x00110025;

const CURVE_TEMPS: [u8; 8] = [30, 40, 50, 60, 70, 80, 90, 100];

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn get_main_hwnd() -> *mut c_void {
    let cached = MAIN_HWND.load(Ordering::SeqCst);
    if cached != 0 {
        return cached as *mut c_void;
    }
    let title = to_wide(WINDOW_TITLE);
    let hwnd = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
    if !hwnd.is_null() {
        MAIN_HWND.store(hwnd as usize, Ordering::SeqCst);
    }
    hwnd
}

fn force_window_to_front_on_top() {
    let hwnd = get_main_hwnd();
    if !hwnd.is_null() {
        WINDOW_HIDDEN.store(false, Ordering::SeqCst);
        PIN_ALWAYS_ON_TOP.store(true, Ordering::SeqCst);
        unsafe {
            ShowWindow(hwnd, 9);
            ShowWindow(hwnd, 5);

            keybd_event(0, 0, 0, 0);
            let fg_hwnd = GetForegroundWindow();
            let fg_tid = if !fg_hwnd.is_null() {
                GetWindowThreadProcessId(fg_hwnd, std::ptr::null_mut())
            } else {
                0
            };
            let cur_tid = GetCurrentThreadId();

            if fg_tid != 0 && fg_tid != cur_tid {
                AttachThreadInput(fg_tid, cur_tid, 1);
                BringWindowToTop(hwnd);
                SetForegroundWindow(hwnd);
                AttachThreadInput(fg_tid, cur_tid, 0);
            } else {
                BringWindowToTop(hwnd);
                SetForegroundWindow(hwnd);
            }

            SetWindowPos(
                hwnd,
                (-1isize) as *mut c_void,
                0,
                0,
                0,
                0,
                0x0001 | 0x0002 | 0x0040,
            );

            PostMessageW(hwnd, 0x000F, 0, 0);
        }
    }
}

fn apply_topmost_state(always_on_top: bool) {
    let hwnd = get_main_hwnd();
    if !hwnd.is_null() {
        let insert_after = if always_on_top {
            (-1isize) as *mut c_void
        } else {
            (-2isize) as *mut c_void
        };
        unsafe {
            SetWindowPos(hwnd, insert_after, 0, 0, 0, 0, 0x0001 | 0x0002);
        }
    }
}

fn hide_main_window_to_tray() {
    let hwnd = get_main_hwnd();
    if !hwnd.is_null() {
        WINDOW_HIDDEN.store(true, Ordering::SeqCst);
        unsafe {
            ShowWindow(hwnd, 0);
        }
    }
}

unsafe extern "system" fn tray_wnd_proc(
    hwnd: *mut c_void,
    msg: u32,
    w_param: usize,
    l_param: isize,
) -> isize {
    const WM_TRAY: u32 = 0x0400 + 1;
    const WM_HOTKEY: u32 = 0x0312;
    const WM_LBUTTONUP: isize = 0x0202;
    const WM_RBUTTONUP: isize = 0x0205;
    const TPM_RETURNCMD: u32 = 0x0100;
    const TPM_NONOTIFY: u32 = 0x0080;
    const MF_STRING: u32 = 0x0000;
    const MF_SEPARATOR: u32 = 0x0800;

    if msg == WM_HOTKEY {
        match w_param {
            1 | 2 => {
                force_window_to_front_on_top();
                if let Ok(mut act) = TRAY_ACTION.lock() {
                    *act = Some(1);
                }
            }
            3 => {
                if let Ok(mut hk) = HOTKEY_EVENT.lock() {
                    *hk = Some(3);
                }
                let main_h = get_main_hwnd();
                if !main_h.is_null() {
                    PostMessageW(main_h, 0x000F, 0, 0);
                }
            }
            _ => {}
        }
        return 0;
    }

    if msg == WM_TRAY {
        let event = l_param & 0xFFFF;
        if event == WM_LBUTTONUP {
            force_window_to_front_on_top();
            if let Ok(mut act) = TRAY_ACTION.lock() {
                *act = Some(1);
            }
        } else if event == WM_RBUTTONUP {
            let menu = CreatePopupMenu();
            AppendMenuW(menu, MF_STRING, 1, to_wide("⚡ Deschide Peste Tot (Ctrl+Shift+F12)").as_ptr());
            AppendMenuW(menu, MF_STRING, 2, to_wide("🎯 Comută Mini-HUD (Overlay Jocuri)").as_ptr());
            AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
            AppendMenuW(menu, MF_STRING, 10, to_wide("🤫 Profil: Silent").as_ptr());
            AppendMenuW(menu, MF_STRING, 11, to_wide("⚖️ Profil: Balanced").as_ptr());
            AppendMenuW(menu, MF_STRING, 12, to_wide("🔥 Profil: Turbo").as_ptr());
            AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
            AppendMenuW(menu, MF_STRING, 20, to_wide("🌿 GPU: Eco Mode (0W)").as_ptr());
            AppendMenuW(menu, MF_STRING, 21, to_wide("🔥 GPU: Standard Mode (NVIDIA)").as_ptr());
            AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
            AppendMenuW(menu, MF_STRING, 30, to_wide("🖥️ Ecran: 60 Hz (Economie)").as_ptr());
            AppendMenuW(menu, MF_STRING, 31, to_wide("🚀 Ecran: Max Hz (Gaming)").as_ptr());
            AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
            AppendMenuW(menu, MF_STRING, 99, to_wide("❌ Închide Complet (Exit)").as_ptr());

            let mut pt = Point::default();
            GetCursorPos(&mut pt);
            SetForegroundWindow(hwnd);
            let cmd = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_NONOTIFY,
                pt.x,
                pt.y,
                0,
                hwnd,
                std::ptr::null(),
            ) as usize;
            DestroyMenu(menu);

            if cmd == 99 {
                let mut nid: NotifyIconDataW = std::mem::zeroed();
                nid.cb_size = std::mem::size_of::<NotifyIconDataW>() as u32;
                nid.h_wnd = hwnd;
                nid.u_id = 1;
                Shell_NotifyIconW(2, &nid);
                std::process::exit(0);
            } else if cmd > 0 {
                force_window_to_front_on_top();
                if let Ok(mut act) = TRAY_ACTION.lock() {
                    *act = Some(cmd);
                }
            }
        }
        return 0;
    }
    DefWindowProcW(hwnd, msg, w_param, l_param)
}

fn spawn_system_tray_and_hotkeys_thread() {
    std::thread::spawn(|| unsafe {
        let class_name = to_wide("ROGControlTrayClassV42");
        let wc = WndClassW {
            style: 0,
            lpfn_wnd_proc: tray_wnd_proc,
            cb_cls_extra: 0,
            cb_wnd_extra: 0,
            h_instance: std::ptr::null_mut(),
            h_icon: std::ptr::null_mut(),
            h_cursor: std::ptr::null_mut(),
            hbr_background: std::ptr::null_mut(),
            lpsz_menu_name: std::ptr::null(),
            lpsz_class_name: class_name.as_ptr(),
        };
        RegisterClassW(&wc);

        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            to_wide("ROGTrayHiddenV42").as_ptr(),
            0,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );

        const MOD_CONTROL: u32 = 0x0002;
        const MOD_SHIFT: u32 = 0x0004;
        const MOD_NOREPEAT: u32 = 0x4000;
        let _ = RegisterHotKey(hwnd, 1, MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT, 0x7B); // Ctrl+Shift+F12
        let _ = RegisterHotKey(hwnd, 2, MOD_NOREPEAT, 0xB7); // VK_LAUNCH_APP2
        let _ = RegisterHotKey(hwnd, 3, MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT, 0x73); // Ctrl+Shift+F4 (Aura)

        let ico_path = to_wide(r"C:\ROGControl\rog-icon.ico");
        let mut h_icon = LoadImageW(
            std::ptr::null_mut(),
            ico_path.as_ptr(),
            1,
            32,
            32,
            0x00000010,
        );
        if h_icon.is_null() {
            h_icon = LoadIconW(std::ptr::null_mut(), 32512 as *const u16);
        }

        let mut nid: NotifyIconDataW = std::mem::zeroed();
        nid.cb_size = std::mem::size_of::<NotifyIconDataW>() as u32;
        nid.h_wnd = hwnd;
        nid.u_id = 1;
        nid.u_flags = 0x01 | 0x02 | 0x04;
        nid.u_callback_message = 0x0400 + 1;
        nid.h_icon = h_icon;

        let tip = to_wide("ROG Control v4.2 (Ctrl+Shift+F12 pentru deschidere peste tot)");
        for (i, &c) in tip.iter().take(127).enumerate() {
            nid.sz_tip[i] = c;
        }

        Shell_NotifyIconW(0, &nid);

        let mut msg: Msg = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });
}

fn get_display_refresh_rates() -> (u32, u32) {
    unsafe {
        let mut dm = DevModeW::default();
        dm.dm_size = std::mem::size_of::<DevModeW>() as u16;
        let mut current_hz = 60;
        let mut cur_w = 1920;
        let mut cur_h = 1080;

        if EnumDisplaySettingsW(std::ptr::null(), 0xFFFFFFFF, &mut dm) != 0 {
            current_hz = dm.dm_display_frequency;
            cur_w = dm.dm_pels_width;
            cur_h = dm.dm_pels_height;
        }

        let mut max_hz = current_hz;
        let mut idx = 0;
        while EnumDisplaySettingsW(std::ptr::null(), idx, &mut dm) != 0 {
            if dm.dm_pels_width == cur_w
                && dm.dm_pels_height == cur_h
                && dm.dm_display_frequency > max_hz
            {
                max_hz = dm.dm_display_frequency;
            }
            idx += 1;
        }
        (current_hz, max_hz.max(144))
    }
}

fn set_display_refresh_rate(target_hz: u32) -> Result<u32, String> {
    unsafe {
        let mut dm = DevModeW::default();
        dm.dm_size = std::mem::size_of::<DevModeW>() as u16;
        if EnumDisplaySettingsW(std::ptr::null(), 0xFFFFFFFF, &mut dm) == 0 {
            return Err("Nu am putut citi rata de refresh.".into());
        }
        dm.dm_display_frequency = target_hz;
        dm.dm_fields = 0x00400000;
        let res = ChangeDisplaySettingsW(&mut dm, 0);
        if res == 0 {
            Ok(target_hz)
        } else {
            Err(format!("Ecranul nu suportă {} Hz.", target_hz))
        }
    }
}

fn has_admin_acpi_access() -> bool {
    OpenOptions::new().read(true).write(true).open(r"\\.\ATKACPI").is_ok()
}

fn acpi_set(device_id: u32, value: u32) -> Result<u32, String> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(r"\\.\ATKACPI")
        .map_err(|_| "Necesită drepturi de Administrator.".to_string())?;

    let mut in_buf = [0u8; 16];
    in_buf[0..4].copy_from_slice(b"DEVS");
    in_buf[4..8].copy_from_slice(&8u32.to_le_bytes());
    in_buf[8..12].copy_from_slice(&device_id.to_le_bytes());
    in_buf[12..16].copy_from_slice(&value.to_le_bytes());

    let mut out_buf = [0u8; 16];
    let mut returned = 0u32;

    let ok = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            ATK_IOCTL,
            in_buf.as_ptr(),
            16,
            out_buf.as_mut_ptr(),
            16,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err("Comanda ACPI a eșuat.".into());
    }
    Ok(u32::from_le_bytes([out_buf[0], out_buf[1], out_buf[2], out_buf[3]]))
}

fn acpi_set_fan_curve(device_id: u32, temps: &[u8; 8], speeds: &[u8; 8]) -> Result<(), String> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(r"\\.\ATKACPI")
        .map_err(|_| "Necesită drepturi de Administrator.".to_string())?;

    let mut in_buf = [0u8; 28];
    in_buf[0..4].copy_from_slice(b"DEVS");
    in_buf[4..8].copy_from_slice(&20u32.to_le_bytes());
    in_buf[8..12].copy_from_slice(&device_id.to_le_bytes());
    in_buf[12..20].copy_from_slice(temps);

    let mut safe_speeds = *speeds;
    for i in 0..8 {
        if temps[i] >= 85 && safe_speeds[i] < 90 {
            safe_speeds[i] = 90;
        } else if temps[i] >= 75 && safe_speeds[i] < 70 {
            safe_speeds[i] = 70;
        }
        if safe_speeds[i] > 100 {
            safe_speeds[i] = 100;
        }
    }
    in_buf[20..28].copy_from_slice(&safe_speeds);

    let mut out_buf = [0u8; 16];
    let mut returned = 0u32;

    let ok = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            ATK_IOCTL,
            in_buf.as_ptr(),
            28,
            out_buf.as_mut_ptr(),
            16,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err("Setarea curbei a eșuat.".into());
    }
    Ok(())
}

fn acpi_get(device_id: u32) -> Option<u32> {
    let file = OpenOptions::new().read(true).write(true).open(r"\\.\ATKACPI").ok()?;
    let mut in_buf = [0u8; 12];
    in_buf[0..4].copy_from_slice(b"DSTS");
    in_buf[4..8].copy_from_slice(&4u32.to_le_bytes());
    in_buf[8..12].copy_from_slice(&device_id.to_le_bytes());

    let mut out_buf = [0u8; 16];
    let mut returned = 0u32;

    let ok = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            ATK_IOCTL,
            in_buf.as_ptr(),
            12,
            out_buf.as_mut_ptr(),
            16,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return None;
    }
    let raw = u32::from_le_bytes([out_buf[0], out_buf[1], out_buf[2], out_buf[3]]);
    if raw == 0xFFFFFFFF {
        return None;
    }
    Some(raw & 0xFFFF)
}

// ==================== PROTOCOL AURA HID 64-BYTE (FĂRĂ CONFLICTE ȘI FĂRĂ RESET LA OPRIT) ====================
fn init_asus_keyboard_hid() {
    let _guard = HID_WRITE_LOCK.lock().ok();
    if let Ok(api) = HidApi::new() {
        for dev in api.device_list() {
            if dev.vendor_id() == 0x0B05 && dev.usage_page() == 0xFF31 {
                if let Ok(device) = dev.open_device(&api) {
                    let mut init1 = [0u8; 64];
                    init1[0] = 0x5A;
                    init1[1..15].copy_from_slice(b"ASUS Tech.Inc.");
                    let _ = device.send_feature_report(&init1);

                    let mut init2 = [0u8; 64];
                    init2[0] = 0x5D;
                    init2[1..15].copy_from_slice(b"ASUS Tech.Inc.");
                    let _ = device.send_feature_report(&init2);
                }
            }
        }
    }
}

/// Setează strict luminozitatea prin pachetul oficial ASUS de 64 de octeți (fără apelul ACPI care stinge tastatura!)
fn set_kbd_brightness(level: u8) {
    let _guard = HID_WRITE_LOCK.lock().ok();
    let clean_lvl = level.min(3);
    if let Ok(api) = HidApi::new() {
        for dev in api.device_list() {
            if dev.vendor_id() == 0x0B05 && dev.usage_page() == 0xFF31 {
                if let Ok(device) = dev.open_device(&api) {
                    let mut msg = [0u8; 64];
                    msg[0] = 0x5A;
                    msg[1] = 0xBA;
                    msg[2] = 0xC5;
                    msg[3] = 0xC4;
                    msg[4] = clean_lvl;
                    let _ = device.send_feature_report(&msg);
                }
            }
        }
    }
}

/// Setează culoarea + modul Aura pe 64 de octeți și reconfirmă treapta de luminozitate ca să nu sară niciodată pe Oprit!
fn set_aura_rgb_and_keep_brightness(mode: u8, r: u8, g: u8, b: u8, current_brightness: u8) {
    let _guard = HID_WRITE_LOCK.lock().ok();
    let active_lvl = if current_brightness == 0 { 2 } else { current_brightness.min(3) };

    if let Ok(api) = HidApi::new() {
        for dev in api.device_list() {
            if dev.vendor_id() == 0x0B05 && dev.usage_page() == 0xFF31 {
                if let Ok(device) = dev.open_device(&api) {
                    // 1. Pachetul de culoare 0x5D (64 bytes, cu ambele culori C1 și C2 populate pentru Breathing/Strobe)
                    let mut msg = [0u8; 64];
                    msg[0] = 0x5D;
                    msg[1] = 0xB3;
                    msg[2] = 0x00; // Zona 0 (Toată tastatura + Lightbar)
                    msg[3] = mode.min(3);
                    msg[4] = r;
                    msg[5] = g;
                    msg[6] = b;
                    msg[7] = 0xEB; // Viteză medie
                    msg[8] = 0x00; // Direcție
                    msg[9] = 0x00;
                    msg[10] = r;   // Culoare secundară (previne stingerea în modul Breathing)
                    msg[11] = g;
                    msg[12] = b;
                    let _ = device.send_feature_report(&msg);

                    // 2. Comanda SET (0xB5)
                    let mut set_m = [0u8; 64];
                    set_m[0] = 0x5D;
                    set_m[1] = 0xB5;
                    let _ = device.send_feature_report(&set_m);

                    // 3. Comanda APPLY (0xB4)
                    let mut apply_m = [0u8; 64];
                    apply_m[0] = 0x5D;
                    apply_m[1] = 0xB4;
                    let _ = device.send_feature_report(&apply_m);

                    // 4. Reconfirmăm luminozitatea imediat după schimbarea culorii ca MCU-ul să nu o treacă pe 0!
                    let mut br_msg = [0u8; 64];
                    br_msg[0] = 0x5A;
                    br_msg[1] = 0xBA;
                    br_msg[2] = 0xC5;
                    br_msg[3] = 0xC4;
                    br_msg[4] = active_lvl;
                    let _ = device.send_feature_report(&br_msg);
                }
            }
        }
    }
}

#[derive(Clone)]
struct AppSettings {
    ui_theme: u8,
    perf_mode: u32,
    battery_limit: u32,
    kbd_level: u8,
    rgb_color: [u8; 3],
    aura_mode: u8,
    cpu_curve: [u8; 8],
    gpu_curve: [u8; 8],
    auto_power_switch: bool,
    ac_perf_mode: u32,
    ac_gpu_eco: u32,
    ac_hz_max: bool,
    dc_perf_mode: u32,
    dc_gpu_eco: u32,
    dc_hz_60: bool,
    idle_kbd_off_mins: u32,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            ui_theme: 0,
            perf_mode: 0,
            battery_limit: 80,
            kbd_level: 3, // Pornim pe Maxim (100%)
            rgb_color: [10, 132, 255],
            aura_mode: 0,
            cpu_curve: [20, 30, 40, 50, 65, 80, 90, 100],
            gpu_curve: [20, 30, 45, 55, 70, 85, 95, 100],
            auto_power_switch: true,
            ac_perf_mode: 0,
            ac_gpu_eco: 0,
            ac_hz_max: true,
            dc_perf_mode: 2,
            dc_gpu_eco: 1,
            dc_hz_60: true,
            idle_kbd_off_mins: 0, // Dezactivat implicit ca să nu stingă luminile neintenționat!
        }
    }
}

impl AppSettings {
    fn save(&self) {
        let cpu_s: Vec<String> = self.cpu_curve.iter().map(|v| v.to_string()).collect();
        let gpu_s: Vec<String> = self.gpu_curve.iter().map(|v| v.to_string()).collect();
        let content = format!(
            "ui_theme={}\nperf_mode={}\nbattery_limit={}\nkbd_level={}\nrgb={},{},{}\naura_mode={}\ncpu_curve={}\ngpu_curve={}\nauto_power_switch={}\nac_perf_mode={}\nac_gpu_eco={}\nac_hz_max={}\ndc_perf_mode={}\ndc_gpu_eco={}\ndc_hz_60={}\nidle_kbd_off_mins={}\n",
            self.ui_theme,
            self.perf_mode,
            self.battery_limit,
            self.kbd_level,
            self.rgb_color[0],
            self.rgb_color[1],
            self.rgb_color[2],
            self.aura_mode,
            cpu_s.join(","),
            gpu_s.join(","),
            self.auto_power_switch,
            self.ac_perf_mode,
            self.ac_gpu_eco,
            self.ac_hz_max,
            self.dc_perf_mode,
            self.dc_gpu_eco,
            self.dc_hz_60,
            self.idle_kbd_off_mins,
        );
        let _ = fs::write(SETTINGS_PATH, content);
    }

    fn load() -> Self {
        let mut s = Self::default();
        if let Ok(txt) = fs::read_to_string(SETTINGS_PATH) {
            for line in txt.lines() {
                if let Some((k, v)) = line.split_once('=') {
                    match k.trim() {
                        "ui_theme" => s.ui_theme = v.parse().unwrap_or(0),
                        "perf_mode" => s.perf_mode = v.parse().unwrap_or(s.perf_mode),
                        "battery_limit" => s.battery_limit = v.parse().unwrap_or(s.battery_limit),
                        "kbd_level" => s.kbd_level = v.parse::<u8>().unwrap_or(3).min(3),
                        "rgb" => {
                            let p: Vec<u8> = v.split(',').filter_map(|x| x.parse().ok()).collect();
                            if p.len() == 3 {
                                s.rgb_color = [p[0], p[1], p[2]];
                            }
                        }
                        "aura_mode" => s.aura_mode = v.parse().unwrap_or(s.aura_mode),
                        "cpu_curve" => {
                            let p: Vec<u8> = v.split(',').filter_map(|x| x.parse().ok()).collect();
                            if p.len() == 8 {
                                s.cpu_curve.copy_from_slice(&p);
                            }
                        }
                        "gpu_curve" => {
                            let p: Vec<u8> = v.split(',').filter_map(|x| x.parse().ok()).collect();
                            if p.len() == 8 {
                                s.gpu_curve.copy_from_slice(&p);
                            }
                        }
                        "auto_power_switch" => s.auto_power_switch = v == "true",
                        "ac_perf_mode" => s.ac_perf_mode = v.parse().unwrap_or(s.ac_perf_mode),
                        "ac_gpu_eco" => s.ac_gpu_eco = v.parse().unwrap_or(s.ac_gpu_eco),
                        "ac_hz_max" => s.ac_hz_max = v == "true",
                        "dc_perf_mode" => s.dc_perf_mode = v.parse().unwrap_or(s.dc_perf_mode),
                        "dc_gpu_eco" => s.dc_gpu_eco = v.parse().unwrap_or(s.dc_gpu_eco),
                        "dc_hz_60" => s.dc_hz_60 = v == "true",
                        "idle_kbd_off_mins" => s.idle_kbd_off_mins = v.parse().unwrap_or(0),
                        _ => {}
                    }
                }
            }
        }
        // Dacă în fișierul vechi rămăsese blocat pe 0 din cauza bugului v4.1, îl pornim pe 2 (Mediu)
        if s.kbd_level == 0 {
            s.kbd_level = 2;
        }
        s
    }
}

#[derive(Clone, Default)]
struct HardwareStats {
    cpu_temp: u32,
    gpu_temp: u32,
    cpu_fan_rpm: u32,
    gpu_fan_rpm: u32,
    cpu_usage: f32,
    ram_used_gb: f32,
    ram_total_gb: f32,
    ram_load_pct: u32,
    ac_online: bool,
    battery_pct: u8,
    battery_rate_w: f32,
    battery_rem_wh: f32,
    battery_full_wh: f32,
    battery_design_wh: f32,
    battery_health_pct: f32,
    gpu_clock_mhz: String,
    gpu_mem_clock_mhz: String,
    gpu_power_w: String,
    gpu_vram_used: String,
    gpu_util_pct: String,
    safety_triggered: bool,
    cpu_history: VecDeque<f32>,
    gpu_history: VecDeque<f32>,
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Dashboard,
    PowerDisplay,
    Fans,
    Lighting,
    Stats,
}

struct SharedState {
    gpu_eco: u32,
    gpu_switching: bool,
    status_msg: String,
    stats: HardwareStats,
    custom_fan_enabled: bool,
    custom_fan_mode: u8,
    manual_cpu_pct: u8,
    manual_gpu_pct: u8,
    kbd_level: u8,
    aura_mode: u8,
    rgb_color: [u8; 3],
    perf_mode: u32,
    osd_banner: Option<(String, Instant)>,
}

struct ThemePalette {
    bg: egui::Color32,
    card_bg: egui::Color32,
    card_border: egui::Color32,
    accent: egui::Color32,
    text_main: egui::Color32,
    text_sub: egui::Color32,
    btn_inactive: egui::Color32,
    rounding: f32,
}

fn get_palette(theme_id: u8) -> ThemePalette {
    match theme_id {
        1 => ThemePalette {
            bg: egui::Color32::from_rgb(242, 242, 247),
            card_bg: egui::Color32::from_rgb(255, 255, 255),
            card_border: egui::Color32::from_rgb(212, 212, 220),
            accent: egui::Color32::from_rgb(0, 122, 255),
            text_main: egui::Color32::from_rgb(28, 28, 30),
            text_sub: egui::Color32::from_rgb(100, 100, 110),
            btn_inactive: egui::Color32::from_rgb(229, 229, 234),
            rounding: 12.0,
        },
        2 => ThemePalette {
            bg: egui::Color32::from_rgb(12, 12, 16),
            card_bg: egui::Color32::from_rgb(20, 21, 28),
            card_border: egui::Color32::from_rgb(110, 20, 35),
            accent: egui::Color32::from_rgb(235, 25, 50),
            text_main: egui::Color32::from_rgb(245, 245, 250),
            text_sub: egui::Color32::from_rgb(155, 160, 175),
            btn_inactive: egui::Color32::from_rgb(32, 34, 45),
            rounding: 4.0,
        },
        3 => ThemePalette {
            bg: egui::Color32::from_rgb(30, 34, 42),
            card_bg: egui::Color32::from_rgb(40, 46, 58),
            card_border: egui::Color32::from_rgb(67, 76, 94),
            accent: egui::Color32::from_rgb(0, 185, 235),
            text_main: egui::Color32::from_rgb(236, 239, 244),
            text_sub: egui::Color32::from_rgb(165, 178, 198),
            btn_inactive: egui::Color32::from_rgb(52, 60, 76),
            rounding: 9.0,
        },
        _ => ThemePalette {
            bg: egui::Color32::from_rgb(24, 24, 29),
            card_bg: egui::Color32::from_rgb(36, 36, 44),
            card_border: egui::Color32::from_rgb(62, 62, 74),
            accent: egui::Color32::from_rgb(10, 132, 255),
            text_main: egui::Color32::from_rgb(245, 245, 247),
            text_sub: egui::Color32::from_rgb(152, 152, 162),
            btn_inactive: egui::Color32::from_rgb(50, 50, 60),
            rounding: 12.0,
        },
    }
}

fn draw_card(ui: &mut egui::Ui, pal: &ThemePalette, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::none()
        .fill(pal.card_bg)
        .stroke(egui::Stroke::new(1.0, pal.card_border))
        .rounding(egui::Rounding::same(pal.rounding))
        .inner_margin(egui::Margin::same(12.0))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            add_contents(ui);
        });
}

struct RogApp {
    tab: Tab,
    shared: Arc<Mutex<SharedState>>,
    settings: AppSettings,
    last_rgb_color: [u8; 3],
    last_aura_mode: u8,
    last_rgb_send: Instant,
    current_hz: u32,
    max_hz: u32,
    trip_mode_100: bool,
    saved_limit_before_trip: u32,
    last_ac_state: Option<bool>,
    kbd_turned_off_by_idle: bool,
    mini_hud_mode: bool,
}

fn filetime_to_u64(ft: FileTime) -> u64 {
    ((ft.dw_high_date_time as u64) << 32) | (ft.dw_low_date_time as u64)
}

fn switch_gpu_in_background(shared: Arc<Mutex<SharedState>>, target_eco: u32) {
    {
        let mut s = shared.lock().unwrap();
        if s.gpu_switching {
            return;
        }
        s.gpu_switching = true;
        s.status_msg = if target_eco == 1 {
            "⏳ Se oprește placa video NVIDIA (Eco Mode 0W)...".into()
        } else {
            "⏳ Se pornește placa video NVIDIA (Standard Mode)...".into()
        };
    }

    std::thread::spawn(move || {
        let res = acpi_set(DEV_GPU_ECO, target_eco);
        std::thread::sleep(Duration::from_millis(600));
        let current = acpi_get(DEV_GPU_ECO).unwrap_or(target_eco);

        if let Ok(mut s) = shared.lock() {
            s.gpu_switching = false;
            s.gpu_eco = current;
            match res {
                Ok(_) => {
                    if target_eco == 1 && current != 1 {
                        s.status_msg = "⚠️ O aplicație folosește placa NVIDIA! Închide jocurile/browserele.".into();
                    } else if current == 1 {
                        s.status_msg = "🌿 Eco Mode ACTIVAT: Placa video NVIDIA este oprită (0W).".into();
                        s.osd_banner = Some(("🌿 GPU: ECO MODE (0W)".into(), Instant::now()));
                    } else {
                        s.status_msg = "🔥 Standard Mode ACTIVAT: Placa video NVIDIA este activă!".into();
                        s.osd_banner = Some(("🔥 GPU: STANDARD MODE".into(), Instant::now()));
                    }
                }
                Err(e) => s.status_msg = e,
            }
        }
    });
}

impl RogApp {
    fn new(start_hidden: bool) -> Self {
        spawn_system_tray_and_hotkeys_thread();
        init_asus_keyboard_hid();

        let settings = AppSettings::load();

        let _ = acpi_set(DEV_BATTERY_LIMIT, settings.battery_limit);
        set_aura_rgb_and_keep_brightness(
            settings.aura_mode,
            settings.rgb_color[0],
            settings.rgb_color[1],
            settings.rgb_color[2],
            settings.kbd_level,
        );
        set_kbd_brightness(settings.kbd_level);

        let gpu_eco = acpi_get(DEV_GPU_ECO).unwrap_or(0);
        let perf_mode = acpi_get(DEV_PERF_MODE).unwrap_or(settings.perf_mode);
        let _ = acpi_set(DEV_PERF_MODE, perf_mode);

        let (current_hz, max_hz) = get_display_refresh_rates();

        let shared = Arc::new(Mutex::new(SharedState {
            gpu_eco,
            gpu_switching: false,
            status_msg: "ROG Control v4.2 activ (Lumini 64-byte stabilizate | Ventilatoare Default BIOS | Ctrl+Shift+F12).".into(),
            stats: HardwareStats {
                cpu_history: VecDeque::from(vec![46.0; 60]),
                gpu_history: VecDeque::from(vec![42.0; 60]),
                ..Default::default()
            },
            custom_fan_enabled: false,
            custom_fan_mode: 0,
            manual_cpu_pct: 50,
            manual_gpu_pct: 50,
            kbd_level: settings.kbd_level,
            aura_mode: settings.aura_mode,
            rgb_color: settings.rgb_color,
            perf_mode,
            osd_banner: None,
        }));

        // THREAD 1: Monitorizare Senzori (FĂRĂ citirea eronată DEV_KBD_BRIGHT care reseta luminile la 0!)
        let shared_clone = Arc::clone(&shared);
        std::thread::spawn(move || {
            let nvml = NvmlReader::new();
            let design_wh = 76.0f32;
            let mut prev_idle = 0u64;
            let mut prev_kernel = 0u64;
            let mut prev_user = 0u64;

            loop {
                let cpu_temp = acpi_get(DEV_CPU_TEMP).unwrap_or(0);
                let gpu_temp_acpi = acpi_get(DEV_GPU_TEMP).unwrap_or(0);
                let cpu_fan_raw = acpi_get(DEV_CPU_FAN_RPM).unwrap_or(0);
                let gpu_fan_raw = acpi_get(DEV_GPU_FAN_RPM).unwrap_or(0);

                let cpu_fan_rpm = if cpu_fan_raw < 150 { cpu_fan_raw * 100 } else { 0 };
                let gpu_fan_rpm = if gpu_fan_raw < 150 { gpu_fan_raw * 100 } else { 0 };

                let mut mem = MemoryStatusEx {
                    dw_length: std::mem::size_of::<MemoryStatusEx>() as u32,
                    dw_memory_load: 0,
                    ull_total_phys: 0,
                    ull_avail_phys: 0,
                    ull_total_page_file: 0,
                    ull_avail_page_file: 0,
                    ull_total_virtual: 0,
                    ull_avail_virtual: 0,
                    ull_avail_extended_virtual: 0,
                };
                unsafe {
                    GlobalMemoryStatusEx(&mut mem);
                }
                let ram_total_gb = mem.ull_total_phys as f32 / (1024.0 * 1024.0 * 1024.0);
                let ram_avail_gb = mem.ull_avail_phys as f32 / (1024.0 * 1024.0 * 1024.0);
                let ram_used_gb = (ram_total_gb - ram_avail_gb).max(0.0);

                let (mut idle, mut kernel, mut user) =
                    (FileTime::default(), FileTime::default(), FileTime::default());
                let mut cpu_usage = 0.0f32;
                unsafe {
                    if GetSystemTimes(&mut idle, &mut kernel, &mut user) != 0 {
                        let i = filetime_to_u64(idle);
                        let k = filetime_to_u64(kernel);
                        let u = filetime_to_u64(user);
                        let sys = (k - prev_kernel) + (u - prev_user);
                        let idl = i - prev_idle;
                        if sys > 0 && prev_kernel > 0 {
                            cpu_usage = ((sys.saturating_sub(idl)) as f32 / sys as f32) * 100.0;
                        }
                        prev_idle = i;
                        prev_kernel = k;
                        prev_user = u;
                    }
                }

                let mut sps = SystemPowerStatus::default();
                let mut batt_state = SystemBatteryState::default();
                unsafe {
                    GetSystemPowerStatus(&mut sps);
                    CallNtPowerInformation(
                        5,
                        std::ptr::null(),
                        0,
                        &mut batt_state as *mut _ as *mut c_void,
                        std::mem::size_of::<SystemBatteryState>() as u32,
                    );
                }

                let ac_online = sps.ac_line_status == 1 || batt_state.ac_on_line == 1;
                let battery_pct = sps.battery_life_percent.min(100);
                let battery_rate_w = (batt_state.rate as f32) / 1000.0;
                let battery_rem_wh = (batt_state.remaining_capacity as f32) / 1000.0;
                let battery_full_wh = if batt_state.max_capacity > 1000 {
                    (batt_state.max_capacity as f32) / 1000.0
                } else {
                    design_wh
                };
                let final_design_wh = design_wh.max(battery_full_wh);
                let battery_health_pct = ((battery_full_wh / final_design_wh) * 100.0).clamp(10.0, 100.0);

                let (gpu_eco, custom_fan_enabled, man_cpu, man_gpu) = {
                    let s = shared_clone.lock().unwrap();
                    (
                        s.gpu_eco,
                        s.custom_fan_enabled,
                        s.manual_cpu_pct,
                        s.manual_gpu_pct,
                    )
                };

                let mut safety_triggered = false;
                if custom_fan_enabled && (cpu_temp >= 75 || gpu_temp_acpi >= 75) {
                    safety_triggered = true;
                    let safe_c = man_cpu.max(85);
                    let safe_g = man_gpu.max(85);
                    let _ = acpi_set_fan_curve(DEV_CPU_FAN_CURVE, &CURVE_TEMPS, &[safe_c; 8]);
                    let _ = acpi_set_fan_curve(DEV_GPU_FAN_CURVE, &CURVE_TEMPS, &[safe_g; 8]);
                }

                let mut gpu_clock_mhz = "Eco / Inactiv".to_string();
                let mut gpu_mem_clock_mhz = "-".to_string();
                let mut gpu_power_w = "0 W".to_string();
                let mut gpu_vram_used = "0 MB".to_string();
                let mut gpu_util_pct = "0 %".to_string();
                let mut final_gpu_temp = gpu_temp_acpi;

                if gpu_eco == 0 {
                    if let Some(ref reader) = nvml {
                        let (t_opt, c_clk, m_clk, pwr, util, vram) = reader.query();
                        if let Some(t) = t_opt {
                            final_gpu_temp = t;
                        }
                        gpu_clock_mhz = c_clk;
                        gpu_mem_clock_mhz = m_clk;
                        gpu_power_w = pwr;
                        gpu_util_pct = util;
                        gpu_vram_used = vram;
                    }
                }

                if let Ok(mut s) = shared_clone.lock() {
                    let mut cpu_hist = s.stats.cpu_history.clone();
                    let mut gpu_hist = s.stats.gpu_history.clone();
                    if cpu_hist.len() >= 60 {
                        cpu_hist.pop_front();
                    }
                    if gpu_hist.len() >= 60 {
                        gpu_hist.pop_front();
                    }
                    cpu_hist.push_back(cpu_temp as f32);
                    gpu_hist.push_back(final_gpu_temp as f32);

                    s.stats = HardwareStats {
                        cpu_temp,
                        gpu_temp: final_gpu_temp,
                        cpu_fan_rpm,
                        gpu_fan_rpm,
                        cpu_usage,
                        ram_used_gb,
                        ram_total_gb,
                        ram_load_pct: mem.dw_memory_load,
                        ac_online,
                        battery_pct,
                        battery_rate_w,
                        battery_rem_wh,
                        battery_full_wh,
                        battery_design_wh: final_design_wh,
                        battery_health_pct,
                        gpu_clock_mhz,
                        gpu_mem_clock_mhz,
                        gpu_power_w,
                        gpu_vram_used,
                        gpu_util_pct,
                        safety_triggered,
                        cpu_history: cpu_hist,
                        gpu_history: gpu_hist,
                    };
                }

                std::thread::sleep(Duration::from_millis(1200));
            }
        });

        // THREAD 2: Citire Taste Fizice ASUS (Cu codurile 0xC5 = UP și 0xC4 = DOWN în ordinea corectă!)
        let shared_keys = Arc::clone(&shared);
        std::thread::spawn(move || {
            if start_hidden {
                std::thread::sleep(Duration::from_millis(600));
                hide_main_window_to_tray();
            }

            let mut open_devices = Vec::new();
            if let Ok(api) = HidApi::new() {
                for dev in api.device_list() {
                    if dev.vendor_id() == 0x0B05 && dev.usage_page() == 0xFF31 {
                        if let Ok(d) = dev.open_device(&api) {
                            let _ = d.set_blocking_mode(false);
                            open_devices.push(d);
                        }
                    }
                }
            }

            let mut last_key_instant = Instant::now();

            loop {
                if fs::metadata(WAKEUP_SIGNAL_PATH).is_ok() {
                    let _ = fs::remove_file(WAKEUP_SIGNAL_PATH);
                    force_window_to_front_on_top();
                }

                for device in &open_devices {
                    let mut buf = [0u8; 64];
                    if let Ok(len) = device.read(&mut buf) {
                        if len >= 2
                            && (buf[0] == 0x5A || buf[0] == 0x5D)
                            && buf[1] != 0
                            && last_key_instant.elapsed() >= Duration::from_millis(150)
                        {
                            let code = buf[1];
                            match code {
                                // Tasta ROG (0x38)
                                0x38 => {
                                    last_key_instant = Instant::now();
                                    force_window_to_front_on_top();
                                }
                                // Fn + F4 / Tasta Aura (0xB3 sau 0xB2)
                                0xB3 | 0xB2 => {
                                    last_key_instant = Instant::now();
                                    let (new_mode, rgb, kbd_lvl) = {
                                        let mut s = shared_keys.lock().unwrap();
                                        s.aura_mode = (s.aura_mode + 1) % 4;
                                        if s.kbd_level == 0 {
                                            s.kbd_level = 2;
                                        }
                                        let m_name = match s.aura_mode {
                                            0 => "STATIC",
                                            1 => "BREATHING",
                                            2 => "COLOR CYCLE",
                                            _ => "STROBE",
                                        };
                                        s.osd_banner = Some((
                                            format!("🎨 AURA MODE: {}", m_name),
                                            Instant::now(),
                                        ));
                                        (s.aura_mode, s.rgb_color, s.kbd_level)
                                    };
                                    set_aura_rgb_and_keep_brightness(new_mode, rgb[0], rgb[1], rgb[2], kbd_lvl);
                                    let hwnd = get_main_hwnd();
                                    if !hwnd.is_null() {
                                        unsafe { PostMessageW(hwnd, 0x000F, 0, 0); }
                                    }
                                }
                                // 0xC5 = Fn + F3 / Fn + F8 (CREȘTE Luminozitatea +)
                                0xC5 | 0x20 => {
                                    last_key_instant = Instant::now();
                                    let lvl = {
                                        let mut s = shared_keys.lock().unwrap();
                                        if s.kbd_level < 3 {
                                            s.kbd_level += 1;
                                        }
                                        let pct = (s.kbd_level as u32) * 33 + if s.kbd_level == 3 { 1 } else { 0 };
                                        s.osd_banner = Some((
                                            format!("☀️ LUMINOZITATE TASTATURĂ: {}%", pct),
                                            Instant::now(),
                                        ));
                                        s.kbd_level
                                    };
                                    set_kbd_brightness(lvl);
                                    let hwnd = get_main_hwnd();
                                    if !hwnd.is_null() {
                                        unsafe { PostMessageW(hwnd, 0x000F, 0, 0); }
                                    }
                                }
                                // 0xC4 = Fn + F2 / Fn + F7 (SCADE Luminozitatea -)
                                0xC4 | 0x10 => {
                                    last_key_instant = Instant::now();
                                    let lvl = {
                                        let mut s = shared_keys.lock().unwrap();
                                        if s.kbd_level > 0 {
                                            s.kbd_level -= 1;
                                        }
                                        let pct = (s.kbd_level as u32) * 33;
                                        s.osd_banner = Some((
                                            format!("🔉 LUMINOZITATE TASTATURĂ: {}%", pct),
                                            Instant::now(),
                                        ));
                                        s.kbd_level
                                    };
                                    set_kbd_brightness(lvl);
                                    let hwnd = get_main_hwnd();
                                    if !hwnd.is_null() {
                                        unsafe { PostMessageW(hwnd, 0x000F, 0, 0); }
                                    }
                                }
                                // Fn + F5 / Butonul de Fan -> Strict Silent <-> Balanced <-> Turbo
                                0xAE => {
                                    last_key_instant = Instant::now();
                                    let cur_perf = {
                                        let s = shared_keys.lock().unwrap();
                                        s.perf_mode
                                    };
                                    let next = match cur_perf {
                                        2 => 0,
                                        0 => 1,
                                        _ => 2,
                                    };
                                    let _ = acpi_set(DEV_PERF_MODE, next);
                                    let name = match next {
                                        2 => "🤫 SILENT",
                                        1 => "🔥 TURBO",
                                        _ => "⚖️ BALANCED",
                                    };
                                    if let Ok(mut s) = shared_keys.lock() {
                                        s.perf_mode = next;
                                        s.custom_fan_enabled = false;
                                        s.osd_banner = Some((
                                            format!("🌀 PROFIL: {}", name),
                                            Instant::now(),
                                        ));
                                    }
                                    let hwnd = get_main_hwnd();
                                    if !hwnd.is_null() {
                                        unsafe { PostMessageW(hwnd, 0x000F, 0, 0); }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }

                std::thread::sleep(Duration::from_millis(60));
            }
        });

        Self {
            tab: Tab::Dashboard,
            shared,
            last_rgb_color: settings.rgb_color,
            last_aura_mode: settings.aura_mode,
            last_rgb_send: Instant::now(),
            saved_limit_before_trip: settings.battery_limit,
            settings,
            current_hz,
            max_hz,
            trip_mode_100: false,
            last_ac_state: None,
            kbd_turned_off_by_idle: false,
            mini_hud_mode: false,
        }
    }

    fn trigger_osd(&self, text: impl Into<String>) {
        if let Ok(mut s) = self.shared.lock() {
            s.osd_banner = Some((text.into(), Instant::now()));
        }
    }

    fn apply_perf_mode(&mut self, id: u32) {
        self.settings.perf_mode = id;
        self.settings.save();
        let _ = acpi_set(DEV_PERF_MODE, id);
        let name = match id {
            2 => "🤫 SILENT",
            1 => "🔥 TURBO",
            _ => "⚖️ BALANCED",
        };
        if let Ok(mut s) = self.shared.lock() {
            s.custom_fan_enabled = false;
            s.perf_mode = id;
            s.status_msg = format!("Profilul {} a fost activat (Ventilatoare pe Default BIOS).", name);
            s.osd_banner = Some((format!("🌀 PROFIL: {}", name), Instant::now()));
        }
    }

    fn set_mini_hud(&mut self, ctx: &egui::Context, enable: bool) {
        self.mini_hud_mode = enable;
        if enable {
            PIN_ALWAYS_ON_TOP.store(true, Ordering::SeqCst);
            apply_topmost_state(true);
            ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
                egui::WindowLevel::AlwaysOnTop,
            ));
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(520.0, 44.0)));
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(16.0, 16.0)));
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(true));
            let level = if PIN_ALWAYS_ON_TOP.load(Ordering::SeqCst) {
                egui::WindowLevel::AlwaysOnTop
            } else {
                egui::WindowLevel::Normal
            };
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(740.0, 550.0)));
        }
    }
}

impl eframe::App for RogApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let _ = get_main_hwnd();

        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            hide_main_window_to_tray();
        }

        // Scurtătura Ctrl+Shift+F4 pentru Aura Mode
        let hk_ev = HOTKEY_EVENT.lock().ok().and_then(|mut h| h.take());
        if hk_ev == Some(3) {
            self.settings.aura_mode = (self.settings.aura_mode + 1) % 4;
            self.last_aura_mode = self.settings.aura_mode;
            if self.settings.kbd_level == 0 {
                self.settings.kbd_level = 2;
            }
            let [r, g, b] = self.settings.rgb_color;
            let mode = self.settings.aura_mode;
            let lvl = self.settings.kbd_level;
            if let Ok(mut s) = self.shared.lock() {
                s.aura_mode = mode;
                s.kbd_level = lvl;
            }
            self.settings.save();
            std::thread::spawn(move || {
                set_aura_rgb_and_keep_brightness(mode, r, g, b, lvl);
            });
            let m_name = match mode {
                0 => "STATIC",
                1 => "BREATHING",
                2 => "COLOR CYCLE",
                _ => "STROBE",
            };
            self.trigger_osd(format!("🎨 AURA MODE SCHIMBAT: {}", m_name));
        }

        let tray_cmd = TRAY_ACTION.lock().ok().and_then(|mut a| a.take());
        if let Some(cmd) = tray_cmd {
            match cmd {
                1 => {
                    if self.mini_hud_mode {
                        self.set_mini_hud(ctx, false);
                    }
                }
                2 => self.set_mini_hud(ctx, !self.mini_hud_mode),
                10 => self.apply_perf_mode(2),
                11 => self.apply_perf_mode(0),
                12 => self.apply_perf_mode(1),
                20 => switch_gpu_in_background(Arc::clone(&self.shared), 1),
                21 => switch_gpu_in_background(Arc::clone(&self.shared), 0),
                30 => {
                    if let Ok(hz) = set_display_refresh_rate(60) {
                        self.current_hz = hz;
                        self.trigger_osd("🖥️ ECRAN: 60 Hz");
                    }
                }
                31 => {
                    if let Ok(hz) = set_display_refresh_rate(self.max_hz) {
                        self.current_hz = hz;
                        self.trigger_osd(format!("🚀 ECRAN: {} Hz", hz));
                    }
                }
                _ => {}
            }
        }

        let (gpu_eco, gpu_switching, status_msg, stats, osd_banner) = {
            let mut s = self.shared.lock().unwrap();
            let mut need_save = false;
            if s.kbd_level != self.settings.kbd_level && !self.kbd_turned_off_by_idle {
                self.settings.kbd_level = s.kbd_level;
                need_save = true;
            }
            if s.aura_mode != self.settings.aura_mode {
                self.settings.aura_mode = s.aura_mode;
                self.last_aura_mode = s.aura_mode;
                need_save = true;
            }
            if s.perf_mode != self.settings.perf_mode {
                self.settings.perf_mode = s.perf_mode;
                need_save = true;
            }
            if need_save {
                self.settings.save();
            }

            (
                s.gpu_eco,
                s.gpu_switching,
                s.status_msg.clone(),
                s.stats.clone(),
                s.osd_banner.clone(),
            )
        };

        // Auto-Switch Priză (AC) vs Baterie (DC)
        if let Some(prev_ac) = self.last_ac_state {
            if prev_ac != stats.ac_online {
                if !stats.ac_online && self.trip_mode_100 {
                    self.trip_mode_100 = false;
                    self.settings.battery_limit = self.saved_limit_before_trip;
                    let _ = acpi_set(DEV_BATTERY_LIMIT, self.settings.battery_limit);
                    self.settings.save();
                }

                if self.settings.auto_power_switch {
                    if stats.ac_online {
                        self.apply_perf_mode(self.settings.ac_perf_mode);
                        if gpu_eco != self.settings.ac_gpu_eco {
                            switch_gpu_in_background(Arc::clone(&self.shared), self.settings.ac_gpu_eco);
                        }
                        if self.settings.ac_hz_max {
                            if let Ok(hz) = set_display_refresh_rate(self.max_hz) {
                                self.current_hz = hz;
                            }
                        }
                        self.trigger_osd("🔌 CONECTAT LA PRIZĂ: Profil Performanță Activat");
                    } else {
                        self.apply_perf_mode(self.settings.dc_perf_mode);
                        if gpu_eco != self.settings.dc_gpu_eco {
                            switch_gpu_in_background(Arc::clone(&self.shared), self.settings.dc_gpu_eco);
                        }
                        if self.settings.dc_hz_60 {
                            if let Ok(hz) = set_display_refresh_rate(60) {
                                self.current_hz = hz;
                            }
                        }
                        self.trigger_osd("🔋 PE BATERIE: Mod Economie Activat Automat");
                    }
                }
            }
        }
        self.last_ac_state = Some(stats.ac_online);

        // Stingere Automată a Tastaturii la Inactivitate (doar dacă utilizatorul a setat > 0 minute!)
        if self.settings.idle_kbd_off_mins > 0 && self.settings.kbd_level > 0 {
            unsafe {
                let mut lii = LastInputInfo {
                    cb_size: std::mem::size_of::<LastInputInfo>() as u32,
                    dw_time: 0,
                };
                if GetLastInputInfo(&mut lii) != 0 {
                    let idle_ms = GetTickCount().wrapping_sub(lii.dw_time);
                    let threshold_ms = self.settings.idle_kbd_off_mins * 60 * 1000;
                    if idle_ms >= threshold_ms && !self.kbd_turned_off_by_idle {
                        self.kbd_turned_off_by_idle = true;
                        set_kbd_brightness(0);
                    } else if idle_ms < 800 && self.kbd_turned_off_by_idle {
                        self.kbd_turned_off_by_idle = false;
                        set_kbd_brightness(self.settings.kbd_level);
                    }
                }
            }
        } else if self.kbd_turned_off_by_idle {
            self.kbd_turned_off_by_idle = false;
            set_kbd_brightness(self.settings.kbd_level);
        }

        // Schimbare LIVE a culorii RGB sau efectului din UI (aprinde automat tastatura dacă era pe 0!)
        if (self.settings.rgb_color != self.last_rgb_color
            || self.settings.aura_mode != self.last_aura_mode)
            && self.last_rgb_send.elapsed() >= Duration::from_millis(90)
        {
            self.last_rgb_color = self.settings.rgb_color;
            self.last_aura_mode = self.settings.aura_mode;
            self.last_rgb_send = Instant::now();

            // Dacă utilizatorul alege o culoare sau un efect dar luminozitatea era pe Oprit (0), o trecem automat pe Mediu (2)
            if self.settings.kbd_level == 0 {
                self.settings.kbd_level = 2;
            }
            let lvl = self.settings.kbd_level;
            if let Ok(mut s) = self.shared.lock() {
                s.aura_mode = self.settings.aura_mode;
                s.rgb_color = self.settings.rgb_color;
                s.kbd_level = lvl;
            }
            self.settings.save();
            let mode = self.settings.aura_mode;
            let [r, g, b] = self.settings.rgb_color;
            std::thread::spawn(move || {
                set_aura_rgb_and_keep_brightness(mode, r, g, b, lvl);
            });
        }

        let pal = get_palette(self.settings.ui_theme);
        let mut style = (*ctx.style()).clone();
        style.visuals.dark_mode = self.settings.ui_theme != 1;
        style.visuals.window_fill = pal.bg;
        style.visuals.panel_fill = pal.bg;
        style.visuals.override_text_color = Some(pal.text_main);
        style.visuals.widgets.inactive.bg_fill = pal.btn_inactive;
        style.visuals.widgets.inactive.rounding = egui::Rounding::same(pal.rounding * 0.7);
        style.visuals.widgets.hovered.bg_fill = pal.accent;
        style.visuals.widgets.hovered.rounding = egui::Rounding::same(pal.rounding * 0.7);
        style.visuals.widgets.active.bg_fill = pal.accent;
        style.visuals.widgets.active.rounding = egui::Rounding::same(pal.rounding * 0.7);
        style.visuals.selection.bg_fill = pal.accent;
        ctx.set_style(style);

        if self.mini_hud_mode {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "🍎 CPU: {}°C ({:.0}%) | 🎮 GPU: {}°C | 🌀 {} RPM | 🔋 {:.1}W ({}%) | 💾 RAM: {}%",
                            stats.cpu_temp,
                            stats.cpu_usage,
                            stats.gpu_temp,
                            stats.cpu_fan_rpm,
                            stats.battery_rate_w,
                            stats.battery_pct,
                            stats.ram_load_pct
                        ))
                        .size(13.0)
                        .strong(),
                    );
                    if ui.small_button("🔙 Mărește").clicked() {
                        self.set_mini_hud(ctx, false);
                    }
                });
            });
            ctx.request_repaint_after(Duration::from_millis(800));
            return;
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            // BARA DE SUS ÎN STIL macOS
            ui.horizontal(|ui| {
                let traffic = [
                    (egui::Color32::from_rgb(255, 95, 86), "🔴 Ascunde în System Tray (lângă ceas)", 1u8),
                    (egui::Color32::from_rgb(255, 189, 46), "🟡 Minimizează fereastra", 2u8),
                    (egui::Color32::from_rgb(39, 201, 63), "🟢 Activează Mini-HUD Overlay peste jocuri", 3u8),
                ];
                for (col, tip, act_id) in traffic {
                    let (rect, resp) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::click());
                    let radius = if resp.hovered() { 7.5 } else { 6.5 };
                    ui.painter().circle_filled(rect.center(), radius, col);
                    if resp.on_hover_text(tip).clicked() {
                        match act_id {
                            1 => hide_main_window_to_tray(),
                            2 => ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
                            _ => self.set_mini_hud(ctx, true),
                        }
                    }
                }

                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("ROG Control v4.2")
                        .size(17.5)
                        .strong()
                        .color(pal.accent),
                );

                let is_pinned = PIN_ALWAYS_ON_TOP.load(Ordering::SeqCst);
                let pin_btn = egui::Button::new(
                    egui::RichText::new(if is_pinned {
                        "📌 Peste Tot: ACTIV"
                    } else {
                        "📌 Peste Tot: Oprit"
                    })
                    .size(12.0)
                    .color(if is_pinned { egui::Color32::WHITE } else { pal.text_main }),
                )
                .fill(if is_pinned { pal.accent } else { pal.btn_inactive });

                if ui.add(pin_btn).clicked() {
                    let next_pin = !is_pinned;
                    PIN_ALWAYS_ON_TOP.store(next_pin, Ordering::SeqCst);
                    apply_topmost_state(next_pin);
                }

                let theme_name = match self.settings.ui_theme {
                    1 => "🎨 UI: 🍏 macOS Light",
                    2 => "🎨 UI: 🔴 ROG Cyberpunk",
                    3 => "🎨 UI: ❄️ Nord Ice",
                    _ => "🎨 UI: 🍎 macOS Dark",
                };
                if ui.button(egui::RichText::new(theme_name).size(12.0).strong()).clicked() {
                    self.settings.ui_theme = (self.settings.ui_theme + 1) % 4;
                    self.settings.save();
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let pwr_txt = if stats.ac_online {
                        format!("🔌 {}% ({:+.1}W)", stats.battery_pct, stats.battery_rate_w)
                    } else {
                        format!("🔋 {}% ({:.1}W)", stats.battery_pct, stats.battery_rate_w)
                    };
                    ui.label(
                        egui::RichText::new(format!(
                            "CPU {}°C • GPU {}°C • {}",
                            stats.cpu_temp, stats.gpu_temp, pwr_txt
                        ))
                        .size(12.0)
                        .strong()
                        .color(pal.text_sub),
                    );
                });
            });

            ui.add_space(4.0);

            if let Some((banner_txt, when)) = osd_banner {
                if when.elapsed() < Duration::from_millis(2200) {
                    egui::Frame::none()
                        .fill(pal.accent)
                        .rounding(egui::Rounding::same(8.0))
                        .inner_margin(egui::Margin::symmetric(10.0, 5.0))
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    egui::RichText::new(banner_txt)
                                        .size(13.5)
                                        .strong()
                                        .color(egui::Color32::WHITE),
                                );
                            });
                        });
                } else {
                    ui.label(egui::RichText::new(&status_msg).size(11.5).color(pal.text_sub));
                }
            } else {
                ui.label(egui::RichText::new(&status_msg).size(11.5).color(pal.text_sub));
            }

            ui.add_space(4.0);

            // BARA DE TAB-URI ÎN STIL macOS
            egui::Frame::none()
                .fill(pal.card_bg)
                .stroke(egui::Stroke::new(1.0, pal.card_border))
                .rounding(egui::Rounding::same(pal.rounding))
                .inner_margin(egui::Margin::same(4.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let tabs = [
                            (Tab::Dashboard, "🏠 Principal"),
                            (Tab::PowerDisplay, "⚡ Baterie & Ecran"),
                            (Tab::Fans, "🌀 Ventilatoare"),
                            (Tab::Lighting, "💡 Lumini & Taste"),
                            (Tab::Stats, "📊 Statistici & Grafic"),
                        ];
                        for (t, label) in tabs {
                            let selected = self.tab == t;
                            let txt_col = if selected {
                                egui::Color32::WHITE
                            } else {
                                pal.text_main
                            };
                            let btn = egui::Button::new(
                                egui::RichText::new(label).size(13.0).strong().color(txt_col),
                            )
                            .fill(if selected {
                                pal.accent
                            } else {
                                egui::Color32::TRANSPARENT
                            })
                            .rounding(egui::Rounding::same(pal.rounding * 0.75))
                            .min_size(egui::vec2(134.0, 31.0));

                            if ui.add(btn).clicked() {
                                self.tab = t;
                            }
                        }
                    });
                });

            ui.add_space(8.0);

            match self.tab {
                // ==================== TAB 1: PRINCIPAL ====================
                Tab::Dashboard => {
                    draw_card(ui, &pal, |ui| {
                        ui.label(
                            egui::RichText::new("🎮 Placă Video & Mod Economie Baterie")
                                .size(14.5)
                                .strong(),
                        );
                        ui.add_space(6.0);

                        ui.horizontal(|ui| {
                            let std_active = gpu_eco == 0;
                            let std_btn = egui::Button::new(
                                egui::RichText::new("🔥 Standard Mode (NVIDIA Activ)")
                                    .size(13.0)
                                    .color(if std_active { egui::Color32::WHITE } else { pal.text_main }),
                            )
                            .fill(if std_active { pal.accent } else { pal.btn_inactive })
                            .min_size(egui::vec2(215.0, 36.0));

                            if ui.add_enabled(!gpu_switching, std_btn).clicked() {
                                switch_gpu_in_background(Arc::clone(&self.shared), 0);
                            }

                            let eco_active = gpu_eco == 1;
                            let eco_btn = egui::Button::new(
                                egui::RichText::new("🌿 Eco Mode (NVIDIA Oprit 0W)")
                                    .size(13.0)
                                    .color(if eco_active { egui::Color32::WHITE } else { pal.text_main }),
                            )
                            .fill(if eco_active {
                                egui::Color32::from_rgb(40, 180, 95)
                            } else {
                                pal.btn_inactive
                            })
                            .min_size(egui::vec2(215.0, 36.0));

                            if ui.add_enabled(!gpu_switching, eco_btn).clicked() {
                                switch_gpu_in_background(Arc::clone(&self.shared), 1);
                            }

                            let super_eco = egui::Button::new(
                                egui::RichText::new("🍃 Super Economie (1-Click)")
                                    .size(13.0)
                                    .color(egui::Color32::WHITE),
                            )
                            .fill(egui::Color32::from_rgb(20, 140, 110))
                            .min_size(egui::vec2(210.0, 36.0));

                            if ui.add(super_eco).clicked() {
                                self.apply_perf_mode(2);
                                switch_gpu_in_background(Arc::clone(&self.shared), 1);
                                if let Ok(hz) = set_display_refresh_rate(60) {
                                    self.current_hz = hz;
                                }
                                self.settings.kbd_level = 0;
                                if let Ok(mut s) = self.shared.lock() {
                                    s.kbd_level = 0;
                                }
                                set_kbd_brightness(0);
                                self.settings.save();
                                self.trigger_osd("🍃 SUPER ECONOMIE: Silent + Eco + 60Hz + Lumini 0%");
                            }
                        });
                    });

                    ui.add_space(8.0);

                    draw_card(ui, &pal, |ui| {
                        ui.label(
                            egui::RichText::new("🚀 Profil Performanță BIOS (Butonul Fan schimbă doar între aceste 3 moduri)")
                                .size(14.5)
                                .strong(),
                        );
                        ui.add_space(6.0);

                        ui.horizontal(|ui| {
                            let modes = [
                                (2u32, "🤫 Silent"),
                                (0u32, "⚖️ Balanced"),
                                (1u32, "🔥 Turbo"),
                            ];
                            for (id, label) in modes {
                                let active = self.settings.perf_mode == id;
                                let btn = egui::Button::new(
                                    egui::RichText::new(label)
                                        .size(13.0)
                                        .color(if active { egui::Color32::WHITE } else { pal.text_main }),
                                )
                                .fill(if active { pal.accent } else { pal.btn_inactive })
                                .min_size(egui::vec2(135.0, 35.0));

                                if ui.add(btn).clicked() {
                                    self.apply_perf_mode(id);
                                }
                            }

                            ui.separator();

                            let is_60 = self.current_hz <= 65;
                            let btn_60 = egui::Button::new(
                                egui::RichText::new("🖥️ 60 Hz")
                                    .color(if is_60 { egui::Color32::WHITE } else { pal.text_main }),
                            )
                            .fill(if is_60 {
                                egui::Color32::from_rgb(40, 180, 95)
                            } else {
                                pal.btn_inactive
                            })
                            .min_size(egui::vec2(105.0, 35.0));

                            if ui.add(btn_60).clicked() {
                                if let Ok(hz) = set_display_refresh_rate(60) {
                                    self.current_hz = hz;
                                    self.trigger_osd("🖥️ ECRAN SETAT LA 60 Hz");
                                }
                            }

                            let btn_max = egui::Button::new(
                                egui::RichText::new(format!("🚀 {} Hz", self.max_hz))
                                    .color(if !is_60 { egui::Color32::WHITE } else { pal.text_main }),
                            )
                            .fill(if !is_60 { pal.accent } else { pal.btn_inactive })
                            .min_size(egui::vec2(105.0, 35.0));

                            if ui.add(btn_max).clicked() {
                                if let Ok(hz) = set_display_refresh_rate(self.max_hz) {
                                    self.current_hz = hz;
                                    self.trigger_osd(format!("🚀 ECRAN SETAT LA {} Hz", hz));
                                }
                            }
                        });
                    });

                    ui.add_space(8.0);

                    draw_card(ui, &pal, |ui| {
                        ui.label(
                            egui::RichText::new("🔋 Limită Baterie & Scurtătură Globală (Ctrl + Shift + F12)")
                                .size(14.5)
                                .strong(),
                        );
                        ui.add_space(6.0);

                        ui.horizontal(|ui| {
                            ui.label("Limită încărcare:");
                            if ui
                                .add(egui::Slider::new(&mut self.settings.battery_limit, 40..=100).suffix("%"))
                                .changed()
                            {
                                let _ = acpi_set(DEV_BATTERY_LIMIT, self.settings.battery_limit);
                                self.settings.save();
                            }

                            let trip_btn = egui::Button::new(
                                egui::RichText::new(if self.trip_mode_100 {
                                    "🎒 Plecare la Drum ACTIV (100% -> revine la 80%)"
                                } else {
                                    "🎒 Încarcă 100% O Singură Dată"
                                })
                                .color(if self.trip_mode_100 { egui::Color32::WHITE } else { pal.text_main }),
                            )
                            .fill(if self.trip_mode_100 {
                                egui::Color32::from_rgb(40, 180, 95)
                            } else {
                                pal.btn_inactive
                            });

                            if ui.add(trip_btn).clicked() {
                                self.trip_mode_100 = !self.trip_mode_100;
                                if self.trip_mode_100 {
                                    self.saved_limit_before_trip = self.settings.battery_limit.min(80);
                                    self.settings.battery_limit = 100;
                                    let _ = acpi_set(DEV_BATTERY_LIMIT, 100);
                                    self.trigger_osd("🎒 PLECARE LA DRUM: Încarcă 100% acum, revine la 80% când scoți priza");
                                } else {
                                    self.settings.battery_limit = self.saved_limit_before_trip;
                                    let _ = acpi_set(DEV_BATTERY_LIMIT, self.settings.battery_limit);
                                }
                                self.settings.save();
                            }
                        });

                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            if ui
                                .button("🛑 Oprește DOAR Armoury Crate (Păstrează tastele Fn+F2/F3 active!)")
                                .clicked()
                            {
                                let _ = Command::new("powershell")
                                    .args([
                                        "-NoProfile",
                                        "-Command",
                                        "Stop-Service -Name 'ArmouryCrateService','AsusAppService' -Force -ErrorAction SilentlyContinue; Start-Service -Name 'AsusOptimization' -ErrorAction SilentlyContinue",
                                    ])
                                    .creation_flags(0x08000000)
                                    .spawn();
                                self.trigger_osd("🛑 Armoury Crate oprit, iar tastele Fn sunt active!");
                            }

                            ui.label(
                                egui::RichText::new("⌨️ Scurtătură Principală: Ctrl + Shift + F12")
                                    .size(12.0)
                                    .strong()
                                    .color(pal.accent),
                            );
                        });
                    });
                }

                // ==================== TAB 2: BATERIE & ECRAN ====================
                Tab::PowerDisplay => {
                    draw_card(ui, &pal, |ui| {
                        ui.label(
                            egui::RichText::new("🔄 Comutare Automată: Profil Priză (AC) vs. Profil Baterie (DC)")
                                .size(14.5)
                                .strong(),
                        );
                        ui.add_space(4.0);
                        if ui
                            .checkbox(
                                &mut self.settings.auto_power_switch,
                                "Schimbă automat profilul, placa video și Hz când bagi sau scoți încărcătorul",
                            )
                            .changed()
                        {
                            self.settings.save();
                        }

                        ui.add_space(6.0);
                        ui.columns(2, |cols| {
                            cols[0].label(egui::RichText::new("🔌 La Priză (AC)").strong());
                            cols[0].horizontal(|ui| {
                                ui.label("Profil:");
                                for (id, name) in [(2, "Silent"), (0, "Balanced"), (1, "Turbo")] {
                                    if ui.selectable_label(self.settings.ac_perf_mode == id, name).clicked() {
                                        self.settings.ac_perf_mode = id;
                                        self.settings.save();
                                    }
                                }
                            });
                            cols[0].horizontal(|ui| {
                                ui.label("GPU:");
                                if ui.selectable_label(self.settings.ac_gpu_eco == 0, "Standard").clicked() {
                                    self.settings.ac_gpu_eco = 0;
                                    self.settings.save();
                                }
                                if ui.selectable_label(self.settings.ac_gpu_eco == 1, "Eco Mode").clicked() {
                                    self.settings.ac_gpu_eco = 1;
                                    self.settings.save();
                                }
                            });
                            if cols[0]
                                .checkbox(&mut self.settings.ac_hz_max, format!("Trece ecranul pe {} Hz", self.max_hz))
                                .changed()
                            {
                                self.settings.save();
                            }

                            cols[1].label(egui::RichText::new("🔋 Pe Baterie (DC)").strong());
                            cols[1].horizontal(|ui| {
                                ui.label("Profil:");
                                for (id, name) in [(2, "Silent"), (0, "Balanced"), (1, "Turbo")] {
                                    if ui.selectable_label(self.settings.dc_perf_mode == id, name).clicked() {
                                        self.settings.dc_perf_mode = id;
                                        self.settings.save();
                                    }
                                }
                            });
                            cols[1].horizontal(|ui| {
                                ui.label("GPU:");
                                if ui.selectable_label(self.settings.dc_gpu_eco == 1, "Eco Mode (0W)").clicked() {
                                    self.settings.dc_gpu_eco = 1;
                                    self.settings.save();
                                }
                                if ui.selectable_label(self.settings.dc_gpu_eco == 0, "Standard").clicked() {
                                    self.settings.dc_gpu_eco = 0;
                                    self.settings.save();
                                }
                            });
                            if cols[1]
                                .checkbox(&mut self.settings.dc_hz_60, "Trece ecranul pe 60 Hz")
                                .changed()
                            {
                                self.settings.save();
                            }
                        });
                    });

                    ui.add_space(8.0);

                    draw_card(ui, &pal, |ui| {
                        ui.label(
                            egui::RichText::new("🔬 Consum Real în Wați & Sănătatea Bateriei")
                                .size(14.5)
                                .strong(),
                        );
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(format!("⚡ Rată curentă baterie: {:+.1} W", stats.battery_rate_w))
                                .size(14.0)
                                .strong()
                                .color(pal.accent),
                        );
                        ui.label(format!(
                            "💚 Sănătate baterie: {:.1}%  ({:.1} Wh actual / {:.1} Wh din fabrică)",
                            stats.battery_health_pct, stats.battery_full_wh, stats.battery_design_wh
                        ));
                        ui.label(format!(
                            "🔋 Capacitate rămasă acum: {:.1} Wh ({}%)",
                            stats.battery_rem_wh, stats.battery_pct
                        ));
                    });
                }

                // ==================== TAB 3: VENTILATOARE (DOAR LA BIFĂ, ÎN REST 100% DEFAULT BIOS) ====================
                Tab::Fans => {
                    draw_card(ui, &pal, |ui| {
                        let (mut custom_enabled, mut custom_mode, mut man_cpu, mut man_gpu) = {
                            let s = self.shared.lock().unwrap();
                            (
                                s.custom_fan_enabled,
                                s.custom_fan_mode,
                                s.manual_cpu_pct,
                                s.manual_gpu_pct,
                            )
                        };

                        let mut changed = false;
                        if ui
                            .checkbox(
                                &mut custom_enabled,
                                egui::RichText::new(
                                    "☑️ Activează Controlul Personalizat al Ventilatoarelor și Curbei (Debifat = 100% Default din Fabrică BIOS)",
                                )
                                .size(14.0)
                                .strong(),
                            )
                            .changed()
                        {
                            changed = true;
                            if !custom_enabled {
                                let _ = acpi_set(DEV_PERF_MODE, self.settings.perf_mode);
                                self.trigger_osd("🔄 Ventilatoarele sunt acum 100% pe Default BIOS!");
                            }
                        }

                        if !custom_enabled {
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(
                                    "🔒 În acest moment ventilatoarele sunt 100% pe DEFAULT BIOS (identic cu Armoury Crate). Bifează căsuța de mai sus doar când vrei să le controlezi manual.",
                                )
                                .size(12.5)
                                .color(egui::Color32::from_rgb(40, 195, 100)),
                            );
                        }

                        ui.add_space(6.0);
                        ui.add_enabled_ui(custom_enabled, |ui| {
                            ui.horizontal(|ui| {
                                ui.label("Mod Control Personalizat:");
                                if ui
                                    .selectable_label(custom_mode == 0, "🎛️ Viteză Fixă în Timp Real (%)")
                                    .clicked()
                                {
                                    custom_mode = 0;
                                    changed = true;
                                }
                                if ui
                                    .selectable_label(custom_mode == 1, "📈 Curbă pe 8 Praguri de Temperatură")
                                    .clicked()
                                {
                                    custom_mode = 1;
                                    changed = true;
                                }
                            });

                            ui.add_space(6.0);
                            if custom_mode == 0 {
                                ui.horizontal(|ui| {
                                    ui.label("🌀 CPU Fan:");
                                    if ui.add(egui::Slider::new(&mut man_cpu, 0..=100).suffix("%")).changed() {
                                        changed = true;
                                    }
                                    ui.label(format!("({} RPM)", stats.cpu_fan_rpm));

                                    ui.separator();

                                    ui.label("🌀 GPU Fan:");
                                    if ui.add(egui::Slider::new(&mut man_gpu, 0..=100).suffix("%")).changed() {
                                        changed = true;
                                    }
                                    ui.label(format!("({} RPM)", stats.gpu_fan_rpm));
                                });
                            } else {
                                ui.columns(2, |cols| {
                                    cols[0].label(egui::RichText::new("Curbă CPU").strong());
                                    for i in 0..8 {
                                        cols[0].horizontal(|ui| {
                                            ui.label(format!("{}°C:", CURVE_TEMPS[i]));
                                            let min_v = if CURVE_TEMPS[i] >= 85 {
                                                90
                                            } else if CURVE_TEMPS[i] >= 75 {
                                                70
                                            } else {
                                                0
                                            };
                                            if self.settings.cpu_curve[i] < min_v {
                                                self.settings.cpu_curve[i] = min_v;
                                            }
                                            if ui
                                                .add(
                                                    egui::Slider::new(&mut self.settings.cpu_curve[i], min_v..=100)
                                                        .suffix("%"),
                                                )
                                                .changed()
                                            {
                                                changed = true;
                                            }
                                        });
                                    }

                                    cols[1].label(egui::RichText::new("Curbă GPU").strong());
                                    for i in 0..8 {
                                        cols[1].horizontal(|ui| {
                                            ui.label(format!("{}°C:", CURVE_TEMPS[i]));
                                            let min_v = if CURVE_TEMPS[i] >= 85 {
                                                90
                                            } else if CURVE_TEMPS[i] >= 75 {
                                                70
                                            } else {
                                                0
                                            };
                                            if self.settings.gpu_curve[i] < min_v {
                                                self.settings.gpu_curve[i] = min_v;
                                            }
                                            if ui
                                                .add(
                                                    egui::Slider::new(&mut self.settings.gpu_curve[i], min_v..=100)
                                                        .suffix("%"),
                                                )
                                                .changed()
                                            {
                                                changed = true;
                                            }
                                        });
                                    }
                                });
                            }
                        });

                        if changed {
                            if let Ok(mut s) = self.shared.lock() {
                                s.custom_fan_enabled = custom_enabled;
                                s.custom_fan_mode = custom_mode;
                                s.manual_cpu_pct = man_cpu;
                                s.manual_gpu_pct = man_gpu;
                            }
                            if custom_enabled {
                                if custom_mode == 0 {
                                    let _ = acpi_set_fan_curve(DEV_CPU_FAN_CURVE, &CURVE_TEMPS, &[man_cpu; 8]);
                                    let _ = acpi_set_fan_curve(DEV_GPU_FAN_CURVE, &CURVE_TEMPS, &[man_gpu; 8]);
                                } else {
                                    self.settings.save();
                                    let _ = acpi_set_fan_curve(
                                        DEV_CPU_FAN_CURVE,
                                        &CURVE_TEMPS,
                                        &self.settings.cpu_curve,
                                    );
                                    let _ = acpi_set_fan_curve(
                                        DEV_GPU_FAN_CURVE,
                                        &CURVE_TEMPS,
                                        &self.settings.gpu_curve,
                                    );
                                }
                            }
                        }
                    });
                }

                // ==================== TAB 4: LUMINI & TASTE (STABILIZAT 100%) ====================
                Tab::Lighting => {
                    draw_card(ui, &pal, |ui| {
                        ui.label(
                            egui::RichText::new("💡 Luminozitate Tastatură (Fără reset la Oprit, sincronizat corect cu Fn+F2 / Fn+F3)")
                                .size(14.5)
                                .strong(),
                        );
                        ui.add_space(6.0);

                        ui.horizontal(|ui| {
                            for (lvl, name) in [
                                (0u8, "🌑 Oprit (0%)"),
                                (1, "🔉 Scăzut (33%)"),
                                (2, "🌗 Mediu (66%)"),
                                (3, "☀️ Maxim (100%)"),
                            ] {
                                let active = self.settings.kbd_level == lvl;
                                let btn = egui::Button::new(
                                    egui::RichText::new(name)
                                        .color(if active { egui::Color32::WHITE } else { pal.text_main }),
                                )
                                .fill(if active { pal.accent } else { pal.btn_inactive })
                                .min_size(egui::vec2(120.0, 35.0));

                                if ui.add(btn).clicked() {
                                    self.settings.kbd_level = lvl;
                                    self.kbd_turned_off_by_idle = false;
                                    if let Ok(mut s) = self.shared.lock() {
                                        s.kbd_level = lvl;
                                    }
                                    self.settings.save();
                                    let [r, g, b] = self.settings.rgb_color;
                                    let mode = self.settings.aura_mode;
                                    std::thread::spawn(move || {
                                        set_kbd_brightness(lvl);
                                        if lvl > 0 {
                                            set_aura_rgb_and_keep_brightness(mode, r, g, b, lvl);
                                        }
                                    });
                                }
                            }

                            ui.separator();
                            ui.label("Stingere la filme (0 = Oprit):");
                            if ui
                                .add(egui::Slider::new(&mut self.settings.idle_kbd_off_mins, 0..=5).suffix("m"))
                                .changed()
                            {
                                self.settings.save();
                            }
                        });

                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            ui.label("Culoare Live:");
                            ui.color_edit_button_srgb(&mut self.settings.rgb_color);

                            ui.separator();
                            ui.label("Efect Aura (Ctrl+Shift+F4):");
                            for (m_id, m_name) in [
                                (0u8, "Static"),
                                (1, "Breathing"),
                                (2, "Color Cycle"),
                                (3, "Strobe"),
                            ] {
                                if ui
                                    .selectable_label(self.settings.aura_mode == m_id, m_name)
                                    .clicked()
                                {
                                    self.settings.aura_mode = m_id;
                                    if self.settings.kbd_level == 0 {
                                        self.settings.kbd_level = 2;
                                    }
                                    if let Ok(mut s) = self.shared.lock() {
                                        s.aura_mode = m_id;
                                        s.kbd_level = self.settings.kbd_level;
                                    }
                                    self.settings.save();
                                }
                            }
                        });

                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            for (name, col) in [
                                ("🔵 Cupertino Blue", [10, 132, 255]),
                                ("🔴 Roșu ROG", [235, 20, 40]),
                                ("🟣 Mov Neon", [170, 20, 255]),
                                ("🟢 Matrix", [15, 235, 60]),
                                ("🟠 Gold", [255, 145, 0]),
                                ("⚪ Alb Pur", [255, 255, 255]),
                            ] {
                                if ui.button(name).clicked() {
                                    self.settings.rgb_color = col;
                                    if self.settings.kbd_level == 0 {
                                        self.settings.kbd_level = 2;
                                    }
                                }
                            }
                        });
                    });

                    ui.add_space(8.0);

                    draw_card(ui, &pal, |ui| {
                        ui.label(
                            egui::RichText::new("⌨️ Scurtături Active în Sistem (0% Consum CPU)")
                                .size(14.5)
                                .strong(),
                        );
                        ui.add_space(4.0);
                        ui.label("• Ctrl + Shift + F12  —  Deschide sau aduce ROG Control PESTE TOATE ferestrele (inclusiv peste Brave)");
                        ui.label("• Ctrl + Shift + F4   —  Schimbă modul de iluminare Aura (înlocuiește tasta Fn+F4)");
                        ui.label("• Fn + F2 / Fn + F3   —  Scade (-) / Crește (+) luminozitatea tastaturii în ordinea corectă");
                        ui.label("• Butonul Fan (Fn+F5) —  Schimbă strict între Silent, Balanced și Turbo");
                    });
                }

                // ==================== TAB 5: STATISTICI & GRAFIC INTERACTIV PRO ====================
                Tab::Stats => {
                    ui.columns(2, |cols| {
                        draw_card(&mut cols[0], &pal, |ui| {
                            ui.label(egui::RichText::new("🧠 Procesor, RAM & Baterie").strong());
                            ui.separator();
                            ui.label(format!(
                                "🌡️ Temperatură CPU:  {} °C   |   🌀 Fan: {} RPM",
                                stats.cpu_temp, stats.cpu_fan_rpm
                            ));
                            ui.label(format!("⚡ Utilizare CPU:    {:.1} %", stats.cpu_usage));
                            ui.label(format!(
                                "💾 Memorie RAM:      {:.2} / {:.2} GB ({}%)",
                                stats.ram_used_gb, stats.ram_total_gb, stats.ram_load_pct
                            ));
                            ui.label(format!(
                                "🔋 Consum Sistem:    {:+.1} W (Sănătate: {:.1}%)",
                                stats.battery_rate_w, stats.battery_health_pct
                            ));
                        });

                        draw_card(&mut cols[1], &pal, |ui| {
                            ui.label(egui::RichText::new("🎮 Placă Video NVIDIA (Citire Directă NVML)").strong());
                            ui.separator();
                            if gpu_eco == 1 {
                                ui.label(
                                    egui::RichText::new("🌿 Eco Mode Activ (Placa NVIDIA este oprită - 0W)")
                                        .color(egui::Color32::from_rgb(40, 195, 95)),
                                );
                            } else {
                                ui.label(format!(
                                    "🌡️ Temp GPU: {} °C   |   🌀 Fan: {} RPM",
                                    stats.gpu_temp, stats.gpu_fan_rpm
                                ));
                                ui.label(format!(
                                    "⚡ Utilizare: {}   |   🔌 Putere: {}",
                                    stats.gpu_util_pct, stats.gpu_power_w
                                ));
                                ui.label(format!(
                                    "🚀 Core: {}   |   🚀 Mem: {}",
                                    stats.gpu_clock_mhz, stats.gpu_mem_clock_mhz
                                ));
                                ui.label(format!("📦 VRAM: {}", stats.gpu_vram_used));
                            }
                        });
                    });

                    ui.add_space(8.0);

                    draw_card(ui, &pal, |ui| {
                        let cpu_min = stats.cpu_history.iter().cloned().fold(100.0f32, f32::min);
                        let cpu_max = stats.cpu_history.iter().cloned().fold(0.0f32, f32::max);
                        let gpu_min = stats.gpu_history.iter().cloned().fold(100.0f32, f32::min);
                        let gpu_max = stats.gpu_history.iter().cloned().fold(0.0f32, f32::max);

                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("📈 Grafic Temperaturi Live (60s — Pune mouse-ul pe grafic pentru detalii)")
                                    .strong(),
                            );
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "🟢 GPU: {}°C (Min {:.0}° / Max {:.0}°)",
                                        stats.gpu_temp, gpu_min, gpu_max
                                    ))
                                    .size(12.0)
                                    .strong()
                                    .color(egui::Color32::from_rgb(40, 210, 105)),
                                );
                                ui.label(" | ");
                                ui.label(
                                    egui::RichText::new(format!(
                                        "🔴 CPU: {}°C (Min {:.0}° / Max {:.0}°)",
                                        stats.cpu_temp, cpu_min, cpu_max
                                    ))
                                    .size(12.0)
                                    .strong()
                                    .color(egui::Color32::from_rgb(245, 60, 80)),
                                );
                            });
                        });

                        ui.add_space(4.0);
                        let desired = egui::vec2(ui.available_width(), 175.0);
                        let (rect, response) = ui.allocate_exact_size(desired, egui::Sense::hover());
                        let painter = ui.painter_at(rect);

                        painter.rect_filled(rect, 8.0, pal.bg);

                        let plot_left = rect.left() + 38.0;
                        let plot_right = rect.right() - 8.0;
                        let plot_top = rect.top() + 10.0;
                        let plot_bottom = rect.bottom() - 18.0;
                        let plot_w = (plot_right - plot_left).max(10.0);
                        let plot_h = (plot_bottom - plot_top).max(10.0);

                        for temp_line in [30.0f32, 50.0, 70.0, 90.0] {
                            let norm = ((temp_line - 20.0) / 80.0).clamp(0.0, 1.0);
                            let y = plot_bottom - norm * plot_h;
                            painter.line_segment(
                                [egui::pos2(plot_left, y), egui::pos2(plot_right, y)],
                                egui::Stroke::new(1.0, pal.card_border),
                            );
                            painter.text(
                                egui::pos2(rect.left() + 4.0, y),
                                egui::Align2::LEFT_CENTER,
                                format!("{:.0}°C", temp_line),
                                egui::FontId::proportional(11.0),
                                pal.text_sub,
                            );
                        }

                        let draw_pro_series =
                            |hist: &VecDeque<f32>, line_col: egui::Color32, fill_col: egui::Color32| {
                                if hist.len() < 2 {
                                    return;
                                }
                                let step_x = plot_w / 59.0;
                                let pts: Vec<egui::Pos2> = hist
                                    .iter()
                                    .enumerate()
                                    .map(|(i, &val)| {
                                        let x = plot_left + (i as f32) * step_x;
                                        let norm = ((val - 20.0) / 80.0).clamp(0.0, 1.0);
                                        let y = plot_bottom - norm * plot_h;
                                        egui::pos2(x, y)
                                    })
                                    .collect();

                                for p in &pts {
                                    painter.line_segment(
                                        [*p, egui::pos2(p.x, plot_bottom)],
                                        egui::Stroke::new(step_x * 0.85, fill_col),
                                    );
                                }

                                for w in pts.windows(2) {
                                    painter.line_segment([w[0], w[1]], egui::Stroke::new(2.4, line_col));
                                }
                                if let Some(&last_pt) = pts.last() {
                                    painter.circle_filled(last_pt, 4.0, line_col);
                                }
                            };

                        draw_pro_series(
                            &stats.gpu_history,
                            egui::Color32::from_rgb(40, 210, 105),
                            egui::Color32::from_rgba_unmultiplied(40, 210, 105, 18),
                        );
                        draw_pro_series(
                            &stats.cpu_history,
                            egui::Color32::from_rgb(245, 60, 80),
                            egui::Color32::from_rgba_unmultiplied(245, 60, 80, 22),
                        );

                        if let Some(mouse_pos) = response.hover_pos() {
                            if mouse_pos.x >= plot_left && mouse_pos.x <= plot_right {
                                let ratio = ((mouse_pos.x - plot_left) / plot_w).clamp(0.0, 1.0);
                                let idx = ((ratio * 59.0).round() as usize).min(59);
                                let x_snap = plot_left + (idx as f32) * (plot_w / 59.0);

                                painter.line_segment(
                                    [egui::pos2(x_snap, plot_top), egui::pos2(x_snap, plot_bottom)],
                                    egui::Stroke::new(1.2, pal.accent),
                                );

                                let c_val = stats.cpu_history.get(idx).copied().unwrap_or(0.0);
                                let g_val = stats.gpu_history.get(idx).copied().unwrap_or(0.0);
                                let secs_ago = 59usize.saturating_sub(idx);

                                let cy = plot_bottom - ((c_val - 20.0) / 80.0).clamp(0.0, 1.0) * plot_h;
                                let gy = plot_bottom - ((g_val - 20.0) / 80.0).clamp(0.0, 1.0) * plot_h;
                                painter.circle_filled(
                                    egui::pos2(x_snap, cy),
                                    4.5,
                                    egui::Color32::from_rgb(245, 60, 80),
                                );
                                painter.circle_filled(
                                    egui::pos2(x_snap, gy),
                                    4.5,
                                    egui::Color32::from_rgb(40, 210, 105),
                                );

                                let tip_text = format!(
                                    "⏱️ Acum {}s   |   🔴 CPU: {:.0}°C   |   🟢 GPU: {:.0}°C",
                                    secs_ago, c_val, g_val
                                );
                                painter.text(
                                    egui::pos2(plot_left + 8.0, plot_top + 8.0),
                                    egui::Align2::LEFT_TOP,
                                    tip_text,
                                    egui::FontId::proportional(12.5),
                                    pal.text_main,
                                );
                            }
                        }
                    });
                }
            }
        });

        if !WINDOW_HIDDEN.load(Ordering::SeqCst) {
            ctx.request_repaint_after(Duration::from_millis(1000));
        }
    }
}

fn build_rog_window_icon() -> egui::IconData {
    let w = 64u32;
    let h = 64u32;
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let idx = ((y * w + x) * 4) as usize;
            let dx = (x as i32 - 32).abs();
            let dy = (y as i32 - 32).abs();
            if dx + dy < 28 {
                let is_border = dx + dy >= 24;
                let is_bolt = (x as i32 - y as i32).abs() <= 4 && dx < 14 && dy < 16;
                if is_border || is_bolt {
                    rgba[idx] = 235;
                    rgba[idx + 1] = 25;
                    rgba[idx + 2] = 50;
                    rgba[idx + 3] = 255;
                } else {
                    rgba[idx] = 22;
                    rgba[idx + 1] = 24;
                    rgba[idx + 2] = 32;
                    rgba[idx + 3] = 255;
                }
            }
        }
    }
    egui::IconData {
        rgba,
        width: w,
        height: h,
    }
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let start_hidden = args.iter().any(|a| a == "--tray");

    let existing = get_main_hwnd();
    if !existing.is_null() {
        let _ = fs::write(WAKEUP_SIGNAL_PATH, "wake");
        force_window_to_front_on_top();
        return Ok(());
    }

    if !has_admin_acpi_access() {
        let _ = fs::write(WAKEUP_SIGNAL_PATH, "wake");
        let _ = Command::new("schtasks")
            .args(["/run", "/tn", "ROGControl_Admin"])
            .creation_flags(0x08000000)
            .spawn();
        return Ok(());
    }

    let icon = Arc::new(build_rog_window_icon());
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([740.0, 550.0])
            .with_resizable(true)
            .with_window_level(egui::WindowLevel::AlwaysOnTop)
            .with_icon(icon),
        ..Default::default()
    };
    eframe::run_native(
        WINDOW_TITLE,
        options,
        Box::new(move |_cc| Ok(Box::new(RogApp::new(start_hidden)))),
    )
}
