use windows::Win32::System::Restore::{
    BEGIN_SYSTEM_CHANGE, END_SYSTEM_CHANGE, MODIFY_SETTINGS, RESTOREPOINTINFOW, SRSetRestorePointW, STATEMGRSTATUS,
};

use super::{data, sys};

const MAX_DESC: usize = 63;

pub fn description(text: &str) -> [u16; 256] {
    let mut out = [0u16; 256];
    for (slot, unit) in out.iter_mut().zip(text.encode_utf16().take(MAX_DESC)) {
        *slot = unit;
    }
    out
}

pub fn create(text: &str) -> bool {
    if !sys::is_admin() {
        return false;
    }
    let mut info = RESTOREPOINTINFOW { dwEventType: BEGIN_SYSTEM_CHANGE, dwRestorePtType: MODIFY_SETTINGS, ..Default::default() };
    info.szDescription = description(text);
    let mut status = STATEMGRSTATUS::default();
    let begun = unsafe { SRSetRestorePointW(&info, &mut status) }.as_bool();
    let (code, seq) = unsafe { (std::ptr::addr_of!(status.nStatus).read_unaligned().0, std::ptr::addr_of!(status.llSequenceNumber).read_unaligned()) };
    if !begun || code != 0 {
        data::log(&format!("restore point not created (status {})", code));
        return false;
    }
    let mut end = RESTOREPOINTINFOW { dwEventType: END_SYSTEM_CHANGE, dwRestorePtType: MODIFY_SETTINGS, llSequenceNumber: seq, ..Default::default() };
    end.szDescription = description(text);
    let closed = unsafe { SRSetRestorePointW(&end, &mut status) }.as_bool();
    data::log(if closed { "restore point created" } else { "restore point could not be closed" });
    closed
}
