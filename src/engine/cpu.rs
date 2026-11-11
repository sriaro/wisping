use std::cell::Cell;
use std::sync::OnceLock;

use windows::Win32::System::SystemInformation::{GetLogicalProcessorInformationEx, LOGICAL_PROCESSOR_RELATIONSHIP, RelationProcessorCore, SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX};
use windows::Win32::System::Threading::{
    GetCurrentThread, SetThreadInformation, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN, THREAD_POWER_THROTTLING_CURRENT_VERSION, THREAD_POWER_THROTTLING_EXECUTION_SPEED,
    THREAD_MODE_BACKGROUND_END, THREAD_POWER_THROTTLING_STATE, THREAD_PRIORITY_BELOW_NORMAL, THREAD_PRIORITY_NORMAL, ThreadPowerThrottling,
};

pub struct Topology {
    pub logical: usize,
    pub fast_cores: usize,
}

pub fn topology() -> &'static Topology {
    static TOPO: OnceLock<Topology> = OnceLock::new();
    TOPO.get_or_init(|| shape(std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4), &cores()))
}

pub fn shape(allowed: usize, cores: &[(u8, usize)]) -> Topology {
    let logical = allowed.max(1);
    if cores.is_empty() {
        return Topology { logical, fast_cores: logical };
    }
    let total: usize = cores.iter().map(|c| c.1).sum::<usize>().max(1);
    let top = cores.iter().map(|c| c.0).max().unwrap_or(0);
    let scale = |n: usize| (n * logical).div_ceil(total).clamp(1, logical);
    let all = scale(cores.len());
    let fast = scale(cores.iter().filter(|c| c.0 == top).count()).min(all);
    Topology { logical, fast_cores: fast }
}

pub fn io_threads() -> usize {
    topology().logical.clamp(2, 16)
}

pub fn registry_threads() -> usize {
    topology().logical.clamp(2, 8)
}

pub fn compute_threads() -> usize {
    topology().fast_cores.clamp(2, 16)
}

const NORMAL: u8 = 0;
const URGENT: u8 = 1;
const ECO: u8 = 2;

thread_local! {
    static MODE: Cell<u8> = const { Cell::new(NORMAL) };
}

pub fn urgent() {
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
    throttle(Some(false));
    MODE.set(URGENT);
}

pub fn eco() {
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN);
    }
    throttle(Some(true));
    MODE.set(ECO);
}

pub fn normal() {
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_END);
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_NORMAL);
    }
    throttle(None);
    MODE.set(NORMAL);
}

pub fn mode() -> u8 {
    MODE.get()
}

pub fn resume(mode: u8) {
    if MODE.get() == mode {
        return;
    }
    normal();
    match mode {
        URGENT => urgent(),
        ECO => eco(),
        _ => {}
    }
}

fn throttle(on: Option<bool>) {
    let state = THREAD_POWER_THROTTLING_STATE {
        Version: THREAD_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: if on.is_some() { THREAD_POWER_THROTTLING_EXECUTION_SPEED } else { 0 },
        StateMask: if on == Some(true) { THREAD_POWER_THROTTLING_EXECUTION_SPEED } else { 0 },
    };
    unsafe {
        let _ = SetThreadInformation(GetCurrentThread(), ThreadPowerThrottling, &state as *const _ as *const _, std::mem::size_of::<THREAD_POWER_THROTTLING_STATE>() as u32);
    }
}

fn records(kind: LOGICAL_PROCESSOR_RELATIONSHIP) -> Vec<u64> {
    let mut len = 0u32;
    unsafe {
        let _ = GetLogicalProcessorInformationEx(kind, None, &mut len);
    }
    if len == 0 {
        return Vec::new();
    }
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    let ok = unsafe { GetLogicalProcessorInformationEx(kind, Some(buf.as_mut_ptr() as *mut SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX), &mut len) }.is_ok();
    if !ok {
        return Vec::new();
    }
    buf.truncate((len as usize).div_ceil(8));
    buf
}

fn walk(kind: LOGICAL_PROCESSOR_RELATIONSHIP, mut each: impl FnMut(&SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX)) {
    let buf = records(kind);
    let bytes = buf.len() * 8;
    let base = buf.as_ptr() as *const u8;
    let head = std::mem::offset_of!(SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX, Anonymous);
    let full = std::mem::size_of::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>();
    let mut at = 0usize;
    while at + head <= bytes {
        let mut rec = SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX::default();
        let size = unsafe {
            std::ptr::copy_nonoverlapping(base.add(at), &mut rec as *mut _ as *mut u8, head);
            rec.Size as usize
        };
        if size < head || at + size > bytes {
            break;
        }
        unsafe { std::ptr::copy_nonoverlapping(base.add(at), &mut rec as *mut _ as *mut u8, size.min(full)) };
        if rec.Relationship == kind {
            each(&rec);
        }
        at += size;
    }
}

fn cores() -> Vec<(u8, usize)> {
    let mut out = Vec::new();
    walk(RelationProcessorCore, |rec| {
        let p = unsafe { &rec.Anonymous.Processor };
        let threads = p.GroupMask[0].Mask.count_ones() as usize;
        out.push((p.EfficiencyClass, threads.max(1)));
    });
    out
}
