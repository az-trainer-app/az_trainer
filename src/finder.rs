//! "Find out what accesses this address" via hardware breakpoints.
//!
//! Attaches as a debugger, arms DR0 on every thread to trap reads/writes of a
//! target address, collects the instruction pointers that hit, then detaches.
//!
//! Notes and hazards:
//!   * Only one debugger may be attached to a process at a time.
//!   * DebugSetProcessKillOnExit(false) is essential -- otherwise the game dies
//!     with us.
//!   * A data breakpoint traps AFTER the accessing instruction retires, so the
//!     reported RIP is the NEXT instruction. The bytes just before it are the
//!     access itself, which is why preceding bytes are captured.
//!   * The game runs slowly while this is attached. Keep windows short.

use std::collections::HashMap;

use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Diagnostics::Debug::{
    ContinueDebugEvent, DebugActiveProcess, DebugActiveProcessStop, DebugSetProcessKillOnExit,
    GetThreadContext, SetThreadContext, WaitForDebugEvent, CONTEXT, CONTEXT_FLAGS,
    DEBUG_EVENT, EXCEPTION_DEBUG_EVENT, CREATE_THREAD_DEBUG_EVENT,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Threading::{
    GetThreadTimes, OpenThread, ResumeThread, SuspendThread, THREAD_ALL_ACCESS,
    THREAD_QUERY_LIMITED_INFORMATION,
};

use crate::mem::Proc;

const CONTEXT_DEBUG_REGISTERS_AMD64: u32 = 0x0010_0010;
const EXCEPTION_SINGLE_STEP: u32 = 0x8000_0004;
const DBG_CONTINUE: u32 = 0x0001_0002;
const DBG_EXCEPTION_NOT_HANDLED: u32 = 0x8001_0001;

#[derive(Debug, Clone)]
pub struct Hit {
    /// instruction pointer reported by the trap (the instruction AFTER the access)
    pub rip: u64,
    pub hits: u32,
    /// module-relative offset when the RIP falls inside the main module
    pub module_offset: Option<u64>,
    /// 24 bytes ending at rip, then 16 from rip
    pub before: Vec<u8>,
    pub at: Vec<u8>,
}

/// 16-byte aligned CONTEXT, as the CPU requires.
#[repr(C, align(16))]
struct AlignedContext(CONTEXT);

pub(crate) fn thread_ids(pid: u32) -> Vec<u32> {
    let mut out = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) else {
            return out;
        };
        let mut te = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        if Thread32First(snap, &mut te).is_ok() {
            loop {
                if te.th32OwnerProcessID == pid {
                    out.push(te.th32ThreadID);
                }
                if Thread32Next(snap, &mut te).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    out
}

/// The process's main thread: the one created first.
///
/// Toolhelp returns threads in whatever order its snapshot holds them, not in
/// creation order, and that order drifts as a game creates and retires
/// workers. So taking the first entry only agrees with the game thread while
/// the process is young - attach to a game that has been running a while and
/// it can name a worker instead. A cave that guards on the result would then
/// run its body off the game thread, and anything touching UObjects there
/// deadlocks against the game thread on the next load.
///
/// Creation time does not drift, so ask for that instead.
pub(crate) fn main_thread_id(pid: u32) -> u32 {
    let mut best: Option<(u64, u32)> = None;
    for id in thread_ids(pid) {
        let created = unsafe {
            let Ok(h) = OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, id) else {
                continue;
            };
            let mut create = FILETIME::default();
            let mut exit = FILETIME::default();
            let mut kernel = FILETIME::default();
            let mut user = FILETIME::default();
            let ok =
                GetThreadTimes(h, &mut create, &mut exit, &mut kernel, &mut user).is_ok();
            let _ = CloseHandle(h);
            if !ok {
                continue;
            }
            ((create.dwHighDateTime as u64) << 32) | create.dwLowDateTime as u64
        };
        if best.is_none_or(|(t, _)| created < t) {
            best = Some((created, id));
        }
    }
    // Nothing queryable: fall back to the old guess rather than returning 0,
    // which would disarm every cave that checks the thread.
    best.map(|(_, id)| id)
        .unwrap_or_else(|| thread_ids(pid).first().copied().unwrap_or(0))
}

/// DR7 for one active breakpoint in slot 0.
/// rw: 1 = write, 3 = read/write.  len: 0=1, 1=2, 3=4, 2=8 bytes.
fn dr7_for(size: usize, rw: u64) -> u64 {
    let len_bits: u64 = match size {
        1 => 0,
        2 => 1,
        8 => 2,
        _ => 3, // 4 bytes
    };
    // L0 enabled, plus RW0/LEN0 fields
    1 | (rw << 16) | (len_bits << 18)
}

/// Edit a thread's debug registers.
///
/// The thread MUST be suspended first: Get/SetThreadContext on a running
/// thread is unreliable, and a partially-applied write leaves the thread with
/// live debug registers. That is how the game gets an unhandled
/// EXCEPTION_SINGLE_STEP after we detach.
fn edit_debug_regs(tid: u32, apply: impl Fn(&mut CONTEXT)) -> bool {
    unsafe {
        let Ok(th) = OpenThread(THREAD_ALL_ACCESS, false, tid) else {
            return false;
        };
        if SuspendThread(th) == u32::MAX {
            let _ = CloseHandle(th);
            return false;
        }
        let mut ctx = AlignedContext(CONTEXT::default());
        ctx.0.ContextFlags = CONTEXT_FLAGS(CONTEXT_DEBUG_REGISTERS_AMD64);
        let mut ok = false;
        if GetThreadContext(th, &mut ctx.0).is_ok() {
            apply(&mut ctx.0);
            ctx.0.ContextFlags = CONTEXT_FLAGS(CONTEXT_DEBUG_REGISTERS_AMD64);
            ok = SetThreadContext(th, &ctx.0).is_ok();
        }
        ResumeThread(th);
        let _ = CloseHandle(th);
        ok
    }
}

fn arm_thread(tid: u32, addr: u64, dr7: u64) -> bool {
    edit_debug_regs(tid, |c| {
        c.Dr0 = addr;
        c.Dr6 = 0;
        c.Dr7 = dr7;
    })
}

fn disarm_thread(tid: u32) -> bool {
    edit_debug_regs(tid, |c| {
        c.Dr0 = 0;
        c.Dr6 = 0;
        c.Dr7 = 0;
    })
}

/// Read back Dr7 to confirm a thread really is disarmed.
fn dr7_of(tid: u32) -> Option<u64> {
    unsafe {
        let th = OpenThread(THREAD_ALL_ACCESS, false, tid).ok()?;
        if SuspendThread(th) == u32::MAX {
            let _ = CloseHandle(th);
            return None;
        }
        let mut ctx = AlignedContext(CONTEXT::default());
        ctx.0.ContextFlags = CONTEXT_FLAGS(CONTEXT_DEBUG_REGISTERS_AMD64);
        let got = GetThreadContext(th, &mut ctx.0).is_ok();
        let dr7 = ctx.0.Dr7;
        ResumeThread(th);
        let _ = CloseHandle(th);
        got.then_some(dr7)
    }
}

/// Clear debug registers everywhere, retrying until nothing is armed.
/// Runs BEFORE detaching - a thread left armed with no debugger crashes the game.
fn disarm_all(pid: u32) -> usize {
    let mut still_armed = 0;
    for _ in 0..5 {
        still_armed = 0;
        for tid in thread_ids(pid) {
            disarm_thread(tid);
            if dr7_of(tid).map(|d| d & 0xFF) != Some(0) {
                still_armed += 1;
            }
        }
        if still_armed == 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    still_armed
}

fn rip_of(tid: u32) -> Option<u64> {
    unsafe {
        let th = OpenThread(THREAD_ALL_ACCESS, false, tid).ok()?;
        let mut ctx = AlignedContext(CONTEXT::default());
        // CONTEXT_CONTROL | CONTEXT_INTEGER for AMD64
        ctx.0.ContextFlags = CONTEXT_FLAGS(0x0010_0001 | 0x0010_0002);
        let got = GetThreadContext(th, &mut ctx.0).is_ok();
        let rip = ctx.0.Rip;
        let _ = CloseHandle(th);
        got.then_some(rip)
    }
}

/// Acknowledge any debug events still queued, so none is left pending when we
/// detach. Returns how many were drained.
fn drain_events(rounds: u32) -> u32 {
    let mut drained = 0;
    for _ in 0..rounds {
        let mut ev = DEBUG_EVENT::default();
        if unsafe { WaitForDebugEvent(&mut ev, 30) }.is_err() {
            break; // nothing queued
        }
        drained += 1;
        unsafe {
            let _ = ContinueDebugEvent(
                ev.dwProcessId,
                ev.dwThreadId,
                windows::Win32::Foundation::NTSTATUS(DBG_CONTINUE as i32),
            );
        }
    }
    drained
}

/// Watch `addr` for `ms` milliseconds; return the instruction pointers that hit.
///
/// `access`: 1 = writes only, 3 = reads and writes.
pub fn find_accessors(
    proc: &Proc,
    pid: u32,
    addr: u64,
    size: usize,
    access: u64,
    ms: u32,
    module: (u64, usize),
) -> Result<Vec<Hit>, String> {
    unsafe {
        DebugActiveProcess(pid).map_err(|e| format!("DebugActiveProcess: {e}"))?;
        let _ = DebugSetProcessKillOnExit(false);
    }

    let dr7 = dr7_for(size, access);
    let tids = thread_ids(pid);
    let armed = tids.iter().filter(|t| arm_thread(**t, addr, dr7)).count();
    println!(
        "[finder] addr=0x{addr:X} size={size} access={access} dr7=0x{dr7:X}           threads={} armed={}",
        tids.len(),
        armed
    );

    let mut counts: HashMap<u64, u32> = HashMap::new();
    let mut events = 0u32;
    let mut exceptions: HashMap<u32, u32> = HashMap::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms as u64);

    while std::time::Instant::now() < deadline {
        let mut ev = DEBUG_EVENT::default();
        let waited = unsafe { WaitForDebugEvent(&mut ev, 100) };
        if waited.is_err() {
            continue; // timeout; loop until the deadline
        }
        events += 1;
        let mut status = DBG_CONTINUE;

        match ev.dwDebugEventCode {
            EXCEPTION_DEBUG_EVENT => {
                let code = unsafe { ev.u.Exception.ExceptionRecord.ExceptionCode.0 as u32 };
                *exceptions.entry(code).or_insert(0) += 1;
                if code == EXCEPTION_SINGLE_STEP {
                    if let Some(rip) = rip_of(ev.dwThreadId) {
                        *counts.entry(rip).or_insert(0) += 1;
                    }
                    // re-arm: DR6 must be cleared or the trap will not fire again
                    arm_thread(ev.dwThreadId, addr, dr7);
                } else {
                    // not ours - let the game handle it
                    status = DBG_EXCEPTION_NOT_HANDLED;
                }
            }
            CREATE_THREAD_DEBUG_EVENT => {
                arm_thread(ev.dwThreadId, addr, dr7);
            }
            _ => {}
        }

        unsafe {
            let _ = ContinueDebugEvent(ev.dwProcessId, ev.dwThreadId, windows::Win32::Foundation::NTSTATUS(status as i32));
        }
    }

    println!("[finder] events={events} exceptions={exceptions:X?} hits={}", counts.len());

    // Teardown order matters, and getting it wrong kills the debuggee:
    //   1. disarm every thread (verified) so no NEW traps can be generated
    //   2. drain events already in flight - a trap left unacknowledged is
    //      re-raised to the process on detach, with no handler, and it dies
    //   3. only then detach
    let leftover = disarm_all(pid);
    let drained = drain_events(20);
    println!(
        "[finder] disarmed all threads, still_armed={leftover}, drained={drained}"
    );
    unsafe {
        let _ = DebugActiveProcessStop(pid);
    }
    if leftover > 0 {
        return Err(format!(
            "{leftover} threads could not be disarmed - detached anyway"
        ));
    }

    let (mbase, msize) = module;
    let mut hits: Vec<Hit> = counts
        .into_iter()
        .map(|(rip, n)| {
            let module_offset = (rip >= mbase && rip < mbase + msize as u64).then(|| rip - mbase);
            Hit {
                rip,
                hits: n,
                module_offset,
                before: proc.read_bytes(rip.saturating_sub(24), 24).unwrap_or_default(),
                at: proc.read_bytes(rip, 16).unwrap_or_default(),
            }
        })
        .collect();
    hits.sort_by(|a, b| b.hits.cmp(&a.hits));
    Ok(hits)
}
