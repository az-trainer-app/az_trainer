//! Validate the detour design on a sacrificial process.
//!
//!   detour_test <pid> <setter-va-hex> <speed-va-hex> <mult>
//!
//! Hooks a 5-byte `movss [rcx+0Ch], xmm1` function entry and scales xmm1, but
//! ONLY when [rcx+20h] matches the guarded address - so one shared setter can
//! be used for many values and only the guarded one is affected.

#![allow(dead_code)]  // each bin pulls in whole modules

#[path = "../mem.rs"]
mod mem;

fn i32b(v: i64) -> [u8; 4] {
    (v as i32).to_le_bytes()
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 5 {
        eprintln!("usage: detour_test <pid> <setter-hex> <guard-addr-hex> <mult>");
        std::process::exit(2);
    }
    let pid: u32 = a[1].parse().unwrap();
    let setter = u64::from_str_radix(a[2].trim_start_matches("0x"), 16).unwrap();
    let guard = u64::from_str_radix(a[3].trim_start_matches("0x"), 16).unwrap();
    let mult: f32 = a[4].parse().unwrap();

    let Some(p) = mem::Proc::open(pid) else {
        eprintln!("cannot open pid");
        std::process::exit(1);
    };

    const STEAL: usize = 5; // movss [rcx+0Ch], xmm1
    let original = p.read_bytes(setter, STEAL).expect("read original");
    println!("original: {:02X?}", original);

    let cave = p.alloc_near(0x1000, setter);
    if cave == 0 {
        eprintln!("alloc failed");
        std::process::exit(1);
    }
    let delta = cave as i64 - setter as i64;
    println!("cave 0x{cave:X} (delta {delta}, rel32-ok {})", delta.abs() < 0x7fff_0000);

    // data slots
    let slot_addr = cave + 0x40; // guarded address  (qword)
    let slot_mult = cave + 0x48; // multiplier       (dword float)

    let mut code: Vec<u8> = Vec::new();
    // mov r10, [rcx+0x20]
    code.extend_from_slice(&[0x4C, 0x8B, 0x51, 0x20]);
    // cmp r10, [rip+disp]   -> slot_addr ; next instruction at cave+11
    code.extend_from_slice(&[0x4C, 0x3B, 0x15]);
    code.extend_from_slice(&i32b(slot_addr as i64 - (cave as i64 + 11)));
    // jne +8 (skip the mulss)
    code.extend_from_slice(&[0x75, 0x08]);
    // mulss xmm1, [rip+disp] -> slot_mult ; next instruction at cave+21
    code.extend_from_slice(&[0xF3, 0x0F, 0x59, 0x0D]);
    code.extend_from_slice(&i32b(slot_mult as i64 - (cave as i64 + 21)));
    // stolen bytes
    code.extend_from_slice(&original);
    // jmp [rip+0]; qword back
    code.extend_from_slice(&[0xFF, 0x25, 0x00, 0x00, 0x00, 0x00]);
    code.extend_from_slice(&(setter + STEAL as u64).to_le_bytes());
    assert!(code.len() <= 0x40, "code overruns the data slots");

    p.write_bytes(cave, &code);
    p.write_bytes(slot_addr, &guard.to_le_bytes());
    p.write_bytes(slot_mult, &mult.to_le_bytes());

    // patch the entry: E9 rel32 -> cave
    let rel = cave as i64 - (setter as i64 + 5);
    let mut patch = vec![0xE9u8];
    patch.extend_from_slice(&i32b(rel));
    println!("patching entry with {:02X?}", patch);
    if !p.write_bytes(setter, &patch) {
        eprintln!("patch failed");
        std::process::exit(1);
    }

    println!("\nhook installed; sampling for 3s...");
    std::thread::sleep(std::time::Duration::from_secs(3));

    let guarded = p.read_f32(guard).unwrap_or(0.0);
    let other = p.read_f32(guard + 4).unwrap_or(0.0);
    println!("  guarded value : {guarded}   (expected 100 * {mult} = {})", 100.0 * mult);
    println!("  other value   : {other}   (expected 100, must be untouched)");

    // restore
    p.write_bytes(setter, &original);
    std::thread::sleep(std::time::Duration::from_millis(300));
    p.free(cave);
    let after = p.read_f32(guard).unwrap_or(0.0);
    println!("\nrestored; guarded value now {after}");
    println!("process alive: {}", mem::find_pid("target2.exe") == Some(pid));
}
