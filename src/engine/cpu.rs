use std::sync::OnceLock;

use windows::Win32::System::SystemInformation::{GetLogicalProcessorInformationEx, LOGICAL_PROCESSOR_RELATIONSHIP, RelationCache, RelationProcessorCore, SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX};
use windows::Win32::System::Threading::{
    GetCurrentThread, SetThreadInformation, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN, THREAD_POWER_THROTTLING_CURRENT_VERSION, THREAD_POWER_THROTTLING_EXECUTION_SPEED,
    THREAD_POWER_THROTTLING_STATE, THREAD_PRIORITY_BELOW_NORMAL, ThreadPowerThrottling,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Topology {
    pub logical: usize,
    pub cores: usize,
    pub fast_cores: usize,
    pub hybrid: bool,
    pub smt: bool,
    pub l3_groups: usize,
}

pub fn topology() -> &'static Topology {
    static TOPO: OnceLock<Topology> = OnceLock::new();
    TOPO.get_or_init(|| shape(std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4), &cores(), l3_groups()))
}

pub fn shape(allowed: usize, cores: &[(u8, usize)], l3_groups: usize) -> Topology {
    let logical = allowed.max(1);
    if cores.is_empty() {
        return Topology { logical, cores: logical, fast_cores: logical, hybrid: false, smt: false, l3_groups: 1 };
    }
    let total: usize = cores.iter().map(|c| c.1).sum::<usize>().max(1);
    let top = cores.iter().map(|c| c.0).max().unwrap_or(0);
    let hybrid = cores.iter().any(|c| c.0 != top);
    let scale = |n: usize| (n * logical).div_ceil(total).clamp(1, logical);
    let all = scale(cores.len());
    let fast = scale(cores.iter().filter(|c| c.0 == top).count()).min(all);
    Topology { logical, cores: all, fast_cores: fast, hybrid, smt: cores.iter().any(|c| c.1 > 1), l3_groups: l3_groups.max(1) }
}

pub fn io_threads() -> usize {
    topology().logical.clamp(2, 16)
}

pub fn compute_threads() -> usize {
    topology().fast_cores.clamp(2, 16)
}

pub fn urgent() {
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
    throttle(false);
}

pub fn eco() {
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN);
    }
    throttle(true);
}

fn throttle(on: bool) {
    let state = THREAD_POWER_THROTTLING_STATE {
        Version: THREAD_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: THREAD_POWER_THROTTLING_EXECUTION_SPEED,
        StateMask: if on { THREAD_POWER_THROTTLING_EXECUTION_SPEED } else { 0 },
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

fn l3_groups() -> usize {
    let mut n = 0;
    walk(RelationCache, |rec| {
        if unsafe { rec.Anonymous.Cache.Level } == 3 {
            n += 1;
        }
    });
    n
}
