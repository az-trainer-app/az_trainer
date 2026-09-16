//! Process attachment, AOB scanning, and pointer-chain resolution.

use std::ffi::c_void;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Module32FirstW, Module32NextW, Process32FirstW, Process32NextW,
    MODULEENTRY32W, PROCESSENTRY32W, TH32CS_SNAPMODULE, TH32CS_SNAPMODULE32, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Memory::{
    VirtualAllocEx, VirtualFreeEx, VirtualProtectEx, VirtualQueryEx,
    MEMORY_BASIC_INFORMATION, MEM_COMMIT, MEM_FREE, MEM_RELEASE, MEM_RESERVE,
    PAGE_EXECUTE_READWRITE, PAGE_GUARD, PAGE_NOACCESS, PAGE_PROTECTION_FLAGS,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_OPERATION, PROCESS_VM_READ,
    PROCESS_VM_WRITE,
};

const SCAN_CHUNK: usize = 16 * 1024 * 1024;

/// A byte pattern with per-nibble wildcards (`4?` and `??` both work).
pub struct Pattern {
    val: Vec<u8>,
    mask: Vec<u8>,
}

impl Pattern {
    pub fn parse(sig: &str) -> Option<Pattern> {
        let mut val = Vec::new();
        let mut mask = Vec::new();
        for tok in sig.split_whitespace() {
            if tok.len() != 2 {
                return None;
            }
            let (mut v, mut m) = (0u8, 0u8);
            for ch in tok.chars() {
                v <<= 4;
                m <<= 4;
                if ch != '?' {
                    m |= 0xF;
                    v |= ch.to_digit(16)? as u8;
                }
            }
            val.push(v & m);
            mask.push(m);
        }
        (!val.is_empty()).then_some(Pattern { val, mask })
    }

    pub fn len(&self) -> usize {
        self.val.len()
    }

    fn matches(&self, buf: &[u8]) -> bool {
        self.val
            .iter()
            .zip(&self.mask)
            .zip(buf)
            .all(|((v, m), b)| b & m == *v)
    }
}

pub fn find_pid(name: &str) -> Option<u32> {
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut pe = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut found = None;
        if Process32FirstW(snap, &mut pe).is_ok() {
            loop {
                let exe = String::from_utf16_lossy(&pe.szExeFile);
                let exe = exe.trim_end_matches('\0');
                if exe.eq_ignore_ascii_case(name) {
                    found = Some(pe.th32ProcessID);
                    break;
                }
                if Process32NextW(snap, &mut pe).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
        found
    }
}

/// Candidates from a value scan in one region: the offsets still in the
/// running, and the 4 bytes each held when last read.
///
/// Indices plus raw bytes rather than a flat list of addresses: a first scan
/// can keep tens of millions of candidates, and at that size the
/// representation is the difference between fitting in memory and not. The
/// bytes are kept raw so an int survives exactly, where a float copy of it
/// would not past 2^24.
pub struct ValueScan {
    pub base: u64,
    pub size: usize,
    /// the 4 bytes are an `i32`, not an `f32`
    pub int: bool,
    pub idx: Vec<u32>,
    pub raw: Vec<u32>,
}

impl ValueScan {
    pub fn decode(int: bool, raw: u32) -> f64 {
        if int {
            raw as i32 as f64
        } else {
            f32::from_bits(raw) as f64
        }
    }

    pub fn value(&self, raw: u32) -> f64 {
        Self::decode(self.int, raw)
    }
}

pub struct Proc {
    pub handle: HANDLE,
    pub pid: u32,
}

impl Drop for Proc {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

impl Proc {
    pub fn open(pid: u32) -> Option<Proc> {
        unsafe {
            let handle = OpenProcess(
                PROCESS_QUERY_INFORMATION | PROCESS_VM_READ | PROCESS_VM_WRITE
                    | PROCESS_VM_OPERATION,
                false,
                pid,
            )
            .ok()?;
            Some(Proc { handle, pid })
        }
    }

    /// Base address and size of a loaded module. Retries: snapshotting a busy
    /// 64-bit process transiently fails.
    pub fn module(&self, name: &str) -> Option<(u64, usize)> {
        for _ in 0..20 {
            unsafe {
                let Ok(snap) =
                    CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, self.pid)
                else {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    continue;
                };
                let mut me = MODULEENTRY32W {
                    dwSize: std::mem::size_of::<MODULEENTRY32W>() as u32,
                    ..Default::default()
                };
                let mut out = None;
                if Module32FirstW(snap, &mut me).is_ok() {
                    loop {
                        let m = String::from_utf16_lossy(&me.szModule);
                        let m = m.trim_end_matches('\0');
                        if m.eq_ignore_ascii_case(name) {
                            out = Some((me.modBaseAddr as u64, me.modBaseSize as usize));
                            break;
                        }
                        if Module32NextW(snap, &mut me).is_err() {
                            break;
                        }
                    }
                }
                let _ = CloseHandle(snap);
                if out.is_some() {
                    return out;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        None
    }

    /// Full path of the process's executable.
    pub fn image_path(&self) -> Option<std::path::PathBuf> {
        use windows::core::PWSTR;
        use windows::Win32::System::Threading::{QueryFullProcessImageNameW, PROCESS_NAME_WIN32};
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        unsafe {
            QueryFullProcessImageNameW(self.handle, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len)
                .ok()?;
        }
        Some(String::from_utf16_lossy(&buf[..len as usize]).into())
    }

    pub fn read(&self, addr: u64, buf: &mut [u8]) -> bool {
        if addr == 0 {
            return false;
        }
        let mut got = 0usize;
        unsafe {
            windows::Win32::System::Diagnostics::Debug::ReadProcessMemory(
                self.handle,
                addr as *const c_void,
                buf.as_mut_ptr() as *mut c_void,
                buf.len(),
                Some(&mut got),
            )
            .is_ok()
                && got == buf.len()
        }
    }

    pub fn read_u64(&self, addr: u64) -> Option<u64> {
        let mut b = [0u8; 8];
        self.read(addr, &mut b).then(|| u64::from_le_bytes(b)).filter(|v| *v != 0)
    }

    pub fn read_f32(&self, addr: u64) -> Option<f32> {
        let mut b = [0u8; 4];
        self.read(addr, &mut b).then(|| f32::from_le_bytes(b))
    }

    pub fn read_i32(&self, addr: u64) -> Option<i32> {
        let mut b = [0u8; 4];
        self.read(addr, &mut b).then(|| i32::from_le_bytes(b))
    }

    pub fn write_f32(&self, addr: u64, v: f32) -> bool {
        if addr == 0 {
            return false;
        }
        let b = v.to_le_bytes();
        let mut put = 0usize;
        unsafe {
            windows::Win32::System::Diagnostics::Debug::WriteProcessMemory(
                self.handle,
                addr as *const c_void,
                b.as_ptr() as *const c_void,
                4,
                Some(&mut put),
            )
            .is_ok()
                && put == 4
        }
    }


    /// Every match of `pat`, up to `limit`.
    pub fn scan_all(&self, base: u64, size: usize, pat: &Pattern, limit: usize) -> Vec<u64> {
        let mut out = Vec::new();
        let mut from = base;
        let end = base + size as u64;
        while out.len() < limit && from < end {
            match self.scan(from, (end - from) as usize, pat) {
                Some(a) => {
                    out.push(a);
                    from = a + 1;
                }
                None => break,
            }
        }
        out
    }

    /// Scan `[base, base+size)` for `pat`, skipping unreadable regions.
    pub fn scan(&self, base: u64, size: usize, pat: &Pattern) -> Option<u64> {
        let end = base + size as u64;
        let mut addr = base;
        let mut buf = vec![0u8; SCAN_CHUNK];

        while addr < end {
            let mut mbi = MEMORY_BASIC_INFORMATION::default();
            let n = unsafe {
                VirtualQueryEx(
                    self.handle,
                    Some(addr as *const c_void),
                    &mut mbi,
                    std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
                )
            };
            if n == 0 {
                break;
            }
            let rbase = mbi.BaseAddress as u64;
            let rsize = mbi.RegionSize;
            let prot = mbi.Protect;
            let readable = mbi.State == MEM_COMMIT
                && (prot & PAGE_GUARD).0 == 0
                && (prot & PAGE_NOACCESS).0 == 0;

            if readable {
                let mut p = rbase.max(base);
                let rend = (rbase + rsize as u64).min(end);
                while p < rend {
                    let want = ((rend - p) as usize).min(SCAN_CHUNK);
                    if want < pat.len() {
                        break;
                    }
                    let slice = &mut buf[..want];
                    if self.read(p, slice) {
                        for i in 0..=(want - pat.len()) {
                            if pat.matches(&slice[i..i + pat.len()]) {
                                return Some(p + i as u64);
                            }
                        }
                    }
                    p += (want - (pat.len() - 1)) as u64;
                }
            }
            addr = rbase + rsize as u64;
        }
        None
    }
}

// ---- code injection primitives -----------------------------------------

impl Proc {
    /// Commit RWX memory in the target, preferring a page within +/-2GB of
    /// `near` so a 5-byte `E9 rel32` jump can reach it. Returns 0 on failure.
    pub fn alloc_near(&self, size: usize, near: u64) -> u64 {
        if near != 0 {
            // walk free regions outward from `near`, staying inside rel32 range
            const SPAN: u64 = 0x7FFF_0000;
            let low = near.saturating_sub(SPAN);
            let high = near.saturating_add(SPAN);
            let mut addr = low & !0xFFFF; // 64K granularity
            while addr < high {
                let mut mbi = MEMORY_BASIC_INFORMATION::default();
                let n = unsafe {
                    VirtualQueryEx(
                        self.handle,
                        Some(addr as *const c_void),
                        &mut mbi,
                        std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
                    )
                };
                if n == 0 {
                    break;
                }
                let rbase = mbi.BaseAddress as u64;
                let rsize = mbi.RegionSize as u64;
                if mbi.State == MEM_FREE && rsize as usize >= size {
                    let candidate = (rbase + 0xFFFF) & !0xFFFF;
                    if candidate >= low && candidate + size as u64 <= high {
                        let p = unsafe {
                            VirtualAllocEx(
                                self.handle,
                                Some(candidate as *const c_void),
                                size,
                                MEM_RESERVE | MEM_COMMIT,
                                PAGE_EXECUTE_READWRITE,
                            )
                        };
                        if !p.is_null() {
                            return p as u64;
                        }
                    }
                }
                addr = rbase + rsize;
            }
        }
        // fall back to anywhere
        let p = unsafe {
            VirtualAllocEx(
                self.handle,
                None,
                size,
                MEM_RESERVE | MEM_COMMIT,
                PAGE_EXECUTE_READWRITE,
            )
        };
        p as u64
    }

    pub fn free(&self, addr: u64) -> bool {
        if addr == 0 {
            return false;
        }
        unsafe { VirtualFreeEx(self.handle, addr as *mut c_void, 0, MEM_RELEASE).is_ok() }
    }

    /// Make a range writable+executable; returns the previous protection.
    pub fn protect_rwx(&self, addr: u64, size: usize) -> u32 {
        let mut old = PAGE_PROTECTION_FLAGS(0);
        let ok = unsafe {
            VirtualProtectEx(
                self.handle,
                addr as *const c_void,
                size,
                PAGE_EXECUTE_READWRITE,
                &mut old,
            )
        };
        if ok.is_ok() {
            old.0
        } else {
            0
        }
    }

    pub fn protect_set(&self, addr: u64, size: usize, prot: u32) -> bool {
        let mut old = PAGE_PROTECTION_FLAGS(0);
        unsafe {
            VirtualProtectEx(
                self.handle,
                addr as *const c_void,
                size,
                PAGE_PROTECTION_FLAGS(prot),
                &mut old,
            )
            .is_ok()
        }
    }

    /// The committed region `addr` falls in: base, size, protection, type.
    ///
    /// A signature can match bytes that are never executed - a packed game
    /// carries plenty of code-shaped data - and hooking those installs
    /// cleanly and then does nothing. Checking the page is executable first
    /// turns that silent failure into an answer.
    pub fn region_of(&self, addr: u64) -> Option<(u64, usize, u32, u32)> {
        let mut mbi = MEMORY_BASIC_INFORMATION::default();
        let n = unsafe {
            VirtualQueryEx(
                self.handle,
                Some(addr as *const c_void),
                &mut mbi,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if n == 0 {
            return None;
        }
        Some((mbi.BaseAddress as u64, mbi.RegionSize, mbi.Protect.0, mbi.Type.0))
    }

    /// Every float in writable private memory that falls by roughly the wall
    /// time between two reads - which is what a countdown does and almost
    /// nothing else does.
    ///
    /// Matching candidates are kept per region as indices plus values rather
    /// than as a flat list of pairs; at tens of millions of candidates that is
    /// the difference between fitting in memory and not.
    pub fn find_falling(&self, lo: f32, hi: f32, ms: u64, limit: usize) -> Vec<(u64, f32, f32)> {
        struct Snap {
            base: u64,
            size: usize,
            idx: Vec<u32>,
            val: Vec<f32>,
        }

        let mut snaps: Vec<Snap> = Vec::new();
        let mut total = 0usize;
        let mut buf: Vec<u8> = Vec::new();
        let mut addr: u64 = 0;

        while addr < (1u64 << 47) && total < limit {
            let mut mbi = MEMORY_BASIC_INFORMATION::default();
            let n = unsafe {
                VirtualQueryEx(
                    self.handle,
                    Some(addr as *const c_void),
                    &mut mbi,
                    std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
                )
            };
            if n == 0 {
                break;
            }
            let rbase = mbi.BaseAddress as u64;
            let rsize = mbi.RegionSize;
            let prot = mbi.Protect;
            let writable = (prot.0 & 0x04) != 0 || (prot.0 & 0x40) != 0;
            let usable = mbi.State == MEM_COMMIT
                && mbi.Type.0 == 0x20000 // MEM_PRIVATE
                && writable
                && (prot & PAGE_GUARD).0 == 0
                && rsize >= 0x1000
                && rsize <= 256 * 1024 * 1024;

            if usable {
                buf.resize(rsize, 0);
                if self.read(rbase, &mut buf[..rsize]) {
                    let mut idx = Vec::new();
                    let mut val = Vec::new();
                    for i in 0..rsize / 4 {
                        let o = i * 4;
                        let v = f32::from_le_bytes([buf[o], buf[o + 1], buf[o + 2], buf[o + 3]]);
                        if v >= lo && v <= hi {
                            idx.push(i as u32);
                            val.push(v);
                        }
                    }
                    if !idx.is_empty() {
                        total += idx.len();
                        snaps.push(Snap { base: rbase, size: rsize, idx, val });
                    }
                }
            }
            if rsize == 0 {
                break;
            }
            addr = rbase.wrapping_add(rsize as u64);
        }

        std::thread::sleep(std::time::Duration::from_millis(ms));
        let secs = ms as f32 / 1000.0;

        let mut out = Vec::new();
        for s in &snaps {
            buf.resize(s.size, 0);
            if !self.read(s.base, &mut buf[..s.size]) {
                continue;
            }
            for (k, &i) in s.idx.iter().enumerate() {
                let o = i as usize * 4;
                if o + 4 > s.size {
                    continue;
                }
                let now = f32::from_le_bytes([buf[o], buf[o + 1], buf[o + 2], buf[o + 3]]);
                let was = s.val[k];
                let drop = was - now;
                if now >= 0.0 && drop > secs * 0.6 && drop < secs * 1.6 {
                    out.push((s.base + o as u64, was, now));
                }
            }
        }
        out
    }

    /// Committed regions, in address order, whose protection `want` accepts.
    /// Guard pages and no-access pages never are.
    fn regions(&self, want: impl Fn(u32) -> bool) -> Vec<(u64, usize)> {
        let mut out = Vec::new();
        let mut addr: u64 = 0;
        while addr < (1u64 << 47) {
            let mut mbi = MEMORY_BASIC_INFORMATION::default();
            let n = unsafe {
                VirtualQueryEx(
                    self.handle,
                    Some(addr as *const c_void),
                    &mut mbi,
                    std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
                )
            };
            if n == 0 {
                break;
            }
            let rbase = mbi.BaseAddress as u64;
            let rsize = mbi.RegionSize;
            let prot = mbi.Protect;
            if mbi.State == MEM_COMMIT
                && (prot & PAGE_GUARD).0 == 0
                && (prot & PAGE_NOACCESS).0 == 0
                && want(prot.0)
            {
                out.push((rbase, rsize));
            }
            if rsize == 0 {
                break;
            }
            addr = rbase.wrapping_add(rsize as u64);
        }
        out
    }

    /// Committed, writable regions - where a game keeps the values it is
    /// changing: the heap, and the module's own data sections.
    fn writable_regions(&self) -> Vec<(u64, usize)> {
        // READWRITE, WRITECOPY, EXECUTE_READWRITE, EXECUTE_WRITECOPY
        self.regions(|p| p & (0x04 | 0x08 | 0x40 | 0x80) != 0)
    }

    /// Walk `[base, base + size)` in `SCAN_CHUNK` pieces, handing
    /// `f(offset, bytes)` each piece that could be read. A piece that fails as
    /// a whole - the process changing its memory mid-read - is retried page
    /// by page, and its readable pages are passed on one at a time, so one
    /// moving page does not hide the rest of a region.
    fn read_chunks(&self, base: u64, size: usize, mut f: impl FnMut(usize, &[u8])) {
        const PAGE: usize = 0x1000;
        let mut buf = vec![0u8; SCAN_CHUNK.min(size)];
        let mut off = 0usize;
        while off < size {
            let n = (size - off).min(SCAN_CHUNK);
            if self.read(base + off as u64, &mut buf[..n]) {
                f(off, &buf[..n]);
            } else {
                for p in (0..n).step_by(PAGE) {
                    let m = PAGE.min(n - p);
                    if self.read(base + (off + p) as u64, &mut buf[..m]) {
                        f(off + p, &buf[..m]);
                    }
                }
            }
            off += n;
        }
    }

    /// A first scan: every 4-byte value in writable memory within `[lo, hi]`,
    /// read as a float, an int, or both, up to `limit` candidates.
    pub fn snapshot(&self, lo: f64, hi: f64, floats: bool, ints: bool, limit: usize) -> Vec<ValueScan> {
        let mut out = Vec::new();
        let mut total = 0usize;
        for (base, size) in self.writable_regions() {
            if total >= limit {
                break;
            }
            let (mut fidx, mut fraw, mut iidx, mut iraw) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
            self.read_chunks(base, size, |off, bytes| {
                for (i, word) in bytes.chunks_exact(4).enumerate() {
                    let r = u32::from_le_bytes(word.try_into().unwrap());
                    let idx = (off / 4 + i) as u32;
                    // NaN compares false, so a float scan never keeps one
                    if floats && (lo..=hi).contains(&ValueScan::decode(false, r)) {
                        fidx.push(idx);
                        fraw.push(r);
                    }
                    if ints && (lo..=hi).contains(&ValueScan::decode(true, r)) {
                        iidx.push(idx);
                        iraw.push(r);
                    }
                }
            });
            for (int, idx, raw) in [(false, fidx, fraw), (true, iidx, iraw)] {
                if !idx.is_empty() {
                    total += idx.len();
                    out.push(ValueScan { base, size, int, idx, raw });
                }
            }
        }
        out
    }

    /// A next scan: re-read every candidate, keep those for which
    /// `keep(before, now)` holds, and remember `now` for the pass after.
    /// Candidates that can no longer be read are dropped. Returns the survivors.
    pub fn refine(&self, scan: &mut Vec<ValueScan>, keep: impl Fn(f64, f64) -> bool) -> usize {
        let mut total = 0usize;
        scan.retain_mut(|s| {
            let (mut idx, mut raw) = (Vec::new(), Vec::new());
            let mut k = 0usize; // candidates are in address order
            self.read_chunks(s.base, s.size, |off, bytes| {
                let end = off + bytes.len();
                while k < s.idx.len() && (s.idx[k] as usize) * 4 < off {
                    k += 1; // in a page that could not be read
                }
                while k < s.idx.len() && (s.idx[k] as usize) * 4 + 4 <= end {
                    let o = s.idx[k] as usize * 4 - off;
                    let r = u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
                    if keep(s.value(s.raw[k]), s.value(r)) {
                        idx.push(s.idx[k]);
                        raw.push(r);
                    }
                    k += 1;
                }
            });
            s.idx = idx;
            s.raw = raw;
            total += s.idx.len();
            !s.idx.is_empty()
        });
        total
    }

    /// Every 8-byte slot in readable committed memory holding a value in
    /// `[lo, hi)`, as `(address, value)`, up to `limit` - the step a reverse
    /// pointer search repeats.
    pub fn find_pointers(&self, lo: u64, hi: u64, limit: usize) -> Vec<(u64, u64)> {
        let mut out = Vec::new();
        // READONLY, READWRITE, WRITECOPY and their EXECUTE_ forms
        for (base, size) in self.regions(|p| p & (0x02 | 0x04 | 0x08 | 0x20 | 0x40 | 0x80) != 0) {
            self.read_chunks(base, size, |off, bytes| {
                for (i, slot) in bytes.chunks_exact(8).enumerate() {
                    if out.len() >= limit {
                        return;
                    }
                    let v = u64::from_le_bytes(slot.try_into().unwrap());
                    if v >= lo && v < hi {
                        out.push((base + (off + i * 8) as u64, v));
                    }
                }
            });
            if out.len() >= limit {
                break;
            }
        }
        out
    }

    pub fn read_bytes(&self, addr: u64, n: usize) -> Option<Vec<u8>> {
        let mut buf = vec![0u8; n];
        self.read(addr, &mut buf).then_some(buf)
    }

    /// Write bytes, temporarily lifting page protection so code can be patched.
    pub fn write_bytes(&self, addr: u64, data: &[u8]) -> bool {
        if addr == 0 || data.is_empty() {
            return false;
        }
        let old = self.protect_rwx(addr, data.len());
        let mut put = 0usize;
        let ok = unsafe {
            windows::Win32::System::Diagnostics::Debug::WriteProcessMemory(
                self.handle,
                addr as *const c_void,
                data.as_ptr() as *const c_void,
                data.len(),
                Some(&mut put),
            )
            .is_ok()
        };
        if old != 0 {
            self.protect_set(addr, data.len(), old);
        }
        ok && put == data.len()
    }
}

#[cfg(test)]
mod tests {
    use super::Pattern;

    #[test]
    fn matches_exact_and_wildcard_bytes() {
        let p = Pattern::parse("48 8B ?? 4?").unwrap();
        assert_eq!(p.len(), 4);
        assert!(p.matches(&[0x48, 0x8B, 0x00, 0x40]));
        assert!(p.matches(&[0x48, 0x8B, 0xFF, 0x4F]));
        assert!(!p.matches(&[0x48, 0x8B, 0x00, 0x50]), "high nibble must match");
        assert!(!p.matches(&[0x49, 0x8B, 0x00, 0x40]));
    }

    #[test]
    fn low_nibble_wildcard() {
        let p = Pattern::parse("?F").unwrap();
        assert!(p.matches(&[0x0F]));
        assert!(p.matches(&[0xFF]));
        assert!(!p.matches(&[0xFE]));
    }

    #[test]
    fn rejects_malformed_signatures() {
        assert!(Pattern::parse("").is_none());
        assert!(Pattern::parse("4").is_none());
        assert!(Pattern::parse("488B").is_none());
        assert!(Pattern::parse("48 8G").is_none());
    }
}

#[cfg(test)]
mod research_tests {
    use super::*;

    fn me() -> Proc {
        Proc::open(std::process::id()).expect("open own process")
    }

    #[test]
    fn an_int_scan_narrows_to_the_value_that_changed() {
        // odd values no other memory is likely to hold, on the heap
        let mut cells: Box<[i32; 3]> = Box::new([731_902_417, 731_902_417, 5]);
        let at = cells.as_ptr() as u64;
        let proc = me();

        let mut scan = proc.snapshot(731_902_417.0, 731_902_417.0, false, true, 1_000_000);
        let found: Vec<u64> = scan.iter().flat_map(|s| s.idx.iter().map(move |&i| s.base + i as u64 * 4)).collect();
        assert!(found.contains(&at) && found.contains(&(at + 4)), "both copies found");
        assert!(scan.iter().all(|s| s.int), "ints only");

        cells[0] -= 7;
        proc.refine(&mut scan, |before, now| now < before);
        let left: Vec<u64> = scan.iter().flat_map(|s| s.idx.iter().map(move |&i| s.base + i as u64 * 4)).collect();
        assert!(left.contains(&at), "the lowered one stays");
        assert!(!left.contains(&(at + 4)), "the unchanged one goes");
        std::hint::black_box(&cells);
    }

    #[test]
    fn a_float_scan_keeps_values_within_a_band() {
        let cells: Box<[f32; 2]> = Box::new([4_812.625, 9.5]);
        let at = cells.as_ptr() as u64;
        let scan = me().snapshot(4_812.5, 4_812.75, true, false, 1_000_000);
        let hit = scan.iter().any(|s| !s.int && s.idx.iter().any(|&i| s.base + i as u64 * 4 == at));
        assert!(hit, "the float in range is found");
        std::hint::black_box(&cells);
    }

    #[test]
    fn find_pointers_sees_a_slot_pointing_into_a_range() {
        let target: Box<[u8; 64]> = Box::new([7; 64]);
        let t = target.as_ptr() as u64;
        let holder: Box<u64> = Box::new(t + 16);
        let slot = &*holder as *const u64 as u64;

        let hits = me().find_pointers(t, t + 64, 100_000);
        assert!(hits.contains(&(slot, t + 16)), "the holder is found with what it points to");
        std::hint::black_box((&target, &holder));
    }
}
