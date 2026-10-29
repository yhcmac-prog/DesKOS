//! Kernel shell with basic commands.

use crate::{println, keyboard, e1fss, sched, pit};
use crate::keyboard::KeyEvent;
use alloc::vec::Vec;
use alloc::string::String;

const MAX_LINE: usize = 256;

pub fn run() -> ! {
    println!("");
    println!("  {} 0.1.0 - kernel shell", crate::BRAND_NAME);
    println!("  Type 'help' for commands.");
    println!("");

    let mut line = String::new();
    loop {
        print!("{}> ", crate::BRAND_NAME);
        line.clear();
        loop {
            let ev = keyboard::getkey();
            if !ev.pressed {
                continue;
            }
            match ev.code {
                b'\n' | b'\r' => {
                    println!("");
                    break;
                }
                8 => {
                    if !line.is_empty() {
                        line.pop();
                        print!("\x08 \x08");
                    }
                }
                c if c >= 32 && c < 127 => {
                    if line.len() < MAX_LINE {
                        line.push(c as char);
                        print!("{}", c as char);
                    }
                }
                _ => {}
            }
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }
        let cmd = parts[0];
        match cmd {
            "help" => cmd_help(),
            "ls" => cmd_ls(),
            "cat" => cmd_cat(&parts),
            "echo" => cmd_echo(&parts),
            "memmap" => cmd_memmap(),
            "ps" => cmd_ps(),
            "run" => cmd_run(&parts),
            "rit" => cmd_rit(&parts),
            "gui" => cmd_gui(),
            "reboot" => cmd_reboot(),
            "ver" => cmd_ver(),
            _ => {
                println!("unknown command: {}", cmd);
            }
        }
    }
}

fn cmd_help() {
    println!("  help      - show this help");
    println!("  ls        - list files");
    println!("  cat <f>   - print file");
    println!("  echo <s>  - print string");
    println!("  memmap    - show memory map");
    println!("  ps        - list processes");
    println!("  run <elf> - run ELF binary");
    println!("  rit <cmd> - package manager (install/list)");
    println!("  gui       - start graphical desktop");
    println!("  ver       - show version");
    println!("  reboot    - restart system");
}

fn cmd_ls() {
    match e1fss::list_dir("/") {
        Ok(entries) => {
            if entries.is_empty() {
                println!("  (empty)");
                return;
            }
            for e in entries {
                let nlen = e.name.iter().position(|&c| c == 0).unwrap_or(59);
                let name = core::str::from_utf8(&e.name[..nlen]).unwrap_or("?");
                let kind = if e.is_dir { "<dir>" } else { "     " };
                println!("  {} {}", kind, name);
            }
        }
        Err(e) => println!("ls: {}", e),
    }
}

fn cmd_cat(parts: &[&str]) {
    if parts.len() < 2 {
        println!("usage: cat <file>");
        return;
    }
    match e1fss::read_file(parts[1]) {
        Ok(data) => {
            let s = core::str::from_utf8(&data).unwrap_or("?");
            println!("{}", s);
        }
        Err(e) => println!("cat: {}", e),
    }
}

fn cmd_echo(parts: &[&str]) {
    for (i, p) in parts[1..].iter().enumerate() {
        if i > 0 {
            print!(" ");
        }
        print!("{}", p);
    }
    println!("");
}

fn cmd_memmap() {
    let bi = crate::bootinfo::get();
    println!("E820 memory map ({} entries):", bi.e820_count);
    for e in bi.e820_entries() {
        let typ = match e.typ {
            1 => "free",
            2 => "reserved",
            3 => "ACPI",
            4 => "ACPI NVS",
            5 => "bad",
            _ => "?",
        };
        println!("  {:#x} - {:#x} ({})", e.base, e.base + e.len, typ);
    }
    println!("");
    println!("  free pages: {} MB / {} MB", crate::pmm::free_mb(), crate::pmm::total_mb());
}

fn cmd_ps() {
    let mut buf = [(0usize, [0u8; 32], sched::State::Ready); 32];
    let n = sched::list(&mut buf);
    println!("  PID  STATE     NAME");
    for (pid, name, state) in &buf[..n] {
        let s = match state {
            sched::State::Ready => "ready",
            sched::State::Running => "running",
            sched::State::Zombie => "zombie",
        };
        let nlen = name.iter().position(|&c| c == 0).unwrap_or(32);
        let n = core::str::from_utf8(&name[..nlen]).unwrap_or("?");
        println!("  {:3}  {:8}  {}", pid, s, n);
    }
}

fn cmd_run(parts: &[&str]) {
    if parts.len() < 2 {
        println!("usage: run <elf>");
        return;
    }
    match e1fss::read_file(parts[1]) {
        Ok(data) => {
            match sched::spawn_user(parts[1].as_bytes(), &data, 0x400000) {
                Some(pid) => println!("spawned pid {}", pid),
                None => println!("failed to spawn"),
            }
        }
        Err(e) => println!("run: {}", e),
    }
}

fn cmd_rit(parts: &[&str]) {
    if parts.len() < 2 {
        println!("usage: rit <install|list>");
        return;
    }
    match parts[1] {
        "install" => {
            if parts.len() < 3 {
                println!("usage: rit install <file.rit>");
                return;
            }
            match e1fss::read_file(parts[2]) {
                Ok(data) => {
                    match crate::rit::install(&data) {
                        Ok(_) => println!("rit: installed"),
                        Err(e) => println!("rit: {}", e),
                    }
                }
                Err(e) => println!("rit: {}", e),
            }
        }
        "list" => {
            match crate::rit::list() {
                Ok(entries) => {
                    if entries.is_empty() {
                        println!("  (no packages installed)");
                        return;
                    }
                    for (name, _) in entries {
                        println!("  {}", name);
                    }
                }
                Err(e) => println!("rit: {}", e),
            }
        }
        _ => println!("usage: rit <install|list>"),
    }
}

fn cmd_gui() {
    println!("gui: not yet implemented (build with e1wm app)");
}

fn cmd_reboot() {
    println!("rebooting...");
    unsafe {
        // keyboard controller reset
        crate::console::outb(0x64, 0xFE);
        loop {}
    }
}

fn cmd_ver() {
    println!("{} kernel {}", crate::BRAND_NAME, crate::VERSION);
}
