//! Standalone harness for the hardware-breakpoint finder.
//!
//! Run against the sacrificial target, never a game you care about:
//!     target.exe            -> prints pid= and addr=
//!     finder_test <pid> <addr-hex> [ms]
//!
//! Expected: the finder reports the instruction(s) in target.exe touching the
//! float, and the target keeps running afterwards.

#![allow(dead_code)]  // each bin pulls in whole modules

#[path = "../mem.rs"]
mod mem;

#[path = "../finder.rs"]
mod finder;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: finder_test <pid> <addr-hex> [ms]");
        std::process::exit(2);
    }
    let pid: u32 = args[1].parse().expect("pid");
    let addr = u64::from_str_radix(args[2].trim_start_matches("0x"), 16).expect("addr");
    let ms: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(4000);
    // 1 = writes only (what computes the value), 3 = reads and writes
    let access: u64 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(3);
    let module_name = args.get(5).cloned().unwrap_or_else(|| "target.exe".to_string());

    let Some(proc) = mem::Proc::open(pid) else {
        eprintln!("cannot open pid {pid}");
        std::process::exit(1);
    };
    let module = proc.module(&module_name).unwrap_or((0, 0));
    println!("target module base=0x{:X} size=0x{:X}", module.0, module.1);
    println!("watching 0x{addr:X} for {ms}ms\n");

    match finder::find_accessors(&proc, pid, addr, 4, access, ms, module) {
        Ok(hits) => {
            println!("\n{} distinct accessors", hits.len());
            for h in hits.iter().take(10) {
                let off = match h.module_offset {
                    Some(o) => format!("{module_name}+0x{o:X}"),
                    None => "outside module".into(),
                };
                let before: Vec<String> =
                    h.before.iter().rev().take(10).rev().map(|b| format!("{b:02X}")).collect();
                let at: Vec<String> = h.at.iter().take(8).map(|b| format!("{b:02X}")).collect();
                println!(
                    "  hits={:<6} rip=0x{:X} ({})\n      before=[{}]  at=[{}]",
                    h.hits, h.rip, off, before.join(" "), at.join(" ")
                );
            }
        }
        Err(e) => println!("finder error: {e}"),
    }

    // did the target survive?
    std::thread::sleep(std::time::Duration::from_millis(500));
    let alive = mem::find_pid(&module_name) == Some(pid);
    println!("\ntarget still alive: {alive}");
}
