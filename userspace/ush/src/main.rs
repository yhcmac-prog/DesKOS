//! ush: user shell shared by DesKOS (open-source) and tAbLeOS (closed-source).
//!
//! DesKOS build (`--cfg brand="desk"`):
//!   - always shows the DesKOS MIT banner
//!   - no activation gate — `need_activation()` is stubbed to false
//!   - `activate` command prints "no activation needed" and exits
//!
//! tAbLeOS build (`--cfg brand="table"`):
//!   - activation gate is real; premium commands need `try_activate()`
//!   - banner switches per activation state

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libe1::{write, read, ls, read_file, create_file, ps, exec, exec_arg, gettimeofday, exit};
use libe1::{wine_execv, rit_install, rit_list};
use libe1::{try_activate, set_activated, brand_status};

fn puts(s: &[u8]) {
    write(1, s);
}

/// Banners chosen at compile time based on brand cfg.
/// DesKOS build always shows the MIT-licensed banner; tAbLeOS build
/// shows the closed-source banner (switched to BANNER_DESK for unlicensed fallback).
#[cfg(brand = "desk")]
const BANNER_DESK: &str = "\nDesKOS user shell (e1sh)\nOpen source under the MIT License\ntype 'help' for commands\n";

#[cfg(not(brand = "desk"))]
const BANNER_TABLE: &str = "\ntAbLeOS user shell (e1sh)\nClosed source, all rights reserved\ntype 'help' for commands\n";
#[cfg(not(brand = "desk"))]
const BANNER_DESK: &str = "\nDesKOS user shell (e1sh, unlicensed)\nOpen source under the MIT License\ntype 'help' for commands\ntype 'activate' to unlock premium tAbLeOS features\n";

/// strip trailing CR/LF/space from a raw console line
fn line_of(b: &[u8]) -> &str {
    let s = core::str::from_utf8(b).unwrap_or("");
    s.trim_end_matches(|c: char| c == '\r' || c == '\n' || c == ' ' || c == '\0')
}

/// prompt + read one line, returned as an owned copy
fn ask(prompt: &[u8], ibuf: &mut [u8]) -> ([u8; 32], usize) {
    puts(prompt);
    let n = read(0, ibuf);
    let s = line_of(&ibuf[..n.max(0) as usize]);
    let mut out = [0u8; 32];
    let m = s.as_bytes().len().min(31);
    out[..m].copy_from_slice(&s.as_bytes()[..m]);
    (out, m)
}

/// gate: print a message if not activated and return true (caller should
/// skip the premium command). Always false for DesKOS builds.
fn need_activation() -> bool {
    #[cfg(brand = "desk")]
    { return false; }
    #[cfg(not(brand = "desk"))]
    {
        if brand_status() > 0 { false }
        else {
            puts(b"this feature requires tAbLeOS activation -- run 'activate'\n");
            true
        }
    }
}

fn setup_tabletop() {
    let mut ibuf = [0u8; 64];
    puts(b"setup-tabletop: configure the system\n");
    puts(b"press enter to accept the [default]\n\n");
    let (host, hlen) = ask(b"hostname [tabletop]: ", &mut ibuf);
    let host = if hlen == 0 { (&b"tabletop"[..]) } else { &host[..hlen] };

    let (kbd, klen) = ask(b"keyboard layout [us]: ", &mut ibuf);
    let kbd = if klen == 0 { (&b"us"[..]) } else { &kbd[..klen] };

    let desktop: &[u8] = {
        #[cfg(edition = "server")]
        {
            b"0" // server edition: no GUI available
        }
        #[cfg(not(edition = "server"))]
        {
            let (ans, _) = ask(b"start graphical desktop on boot? [y/n] (y): ", &mut ibuf);
            if &ans[..1] == b"n" || &ans[..1] == b"N" { b"0" } else { b"1" }
        }
    };

    // desktop environment (session used by the e1dm display manager)
    let session: &[u8] = {
        #[cfg(edition = "server")]
        {
            b"none"
        }
        #[cfg(not(edition = "server"))]
        {
            let (ans, _) = ask(b"desktop environment [e1wm/exaltop] (e1wm): ", &mut ibuf);
            if &ans[..7] == b"exaltop" { b"exaltop" } else { b"e1wm" }
        }
    };

    // build config (e1fss is a flat root fs: desktop.cfg == /etc/desktop.cfg)
    let mut out = [0u8; 192];
    let mut q = 0usize;
    for part in [
        &b"hostname="[..],
        host,
        &b"\nkeyboard="[..],
        kbd,
        &b"\ndesktop="[..],
        desktop,
        &b"\nsession="[..],
        session,
        &b"\n"[..],
    ] {
        out[q..q + part.len()].copy_from_slice(part);
        q += part.len();
    }
    if create_file("desktop.cfg", &out[..q]) == 0 {
        puts(b"config written to desktop.cfg\n");
    } else {
        puts(b"setup: write failed\n");
    }
}

/// parse a license.cfg file into its fields.
/// returns tuple of (activated, serial_str)
fn parse_license(cfg: &str) -> (bool, &str) {
    let mut activated = false;
    let mut serial: &str = "";
    for line in cfg.lines() {
        let l = line.trim();
        if let Some(v) = l.strip_prefix("activated=") {
            activated = v.trim() == "1";
        }
        if let Some(v) = l.strip_prefix("serial=") {
            serial = v.trim();
        }
    }
    (activated, serial)
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // boot-time activation restore: read license.cfg, call sys_set_activated
    // if activated=1, and (for one-time keys) remember the consumed serial
    #[cfg(not(brand = "desk"))]
    {
        let mut lbuf = [0u8; 256];
        let n = read_file("license.cfg", &mut lbuf);
        if n > 0 {
            let cfg = core::str::from_utf8(&lbuf[..n as usize]).unwrap_or("");
            let (activated, _serial) = parse_license(cfg);
            if activated {
                set_activated();
            }
        }
    }

    // banner
    #[cfg(brand = "desk")]
    { puts(BANNER_DESK.as_bytes()); }
    #[cfg(not(brand = "desk"))]
    {
        if brand_status() > 0 {
            puts(BANNER_TABLE.as_bytes());
        } else {
            puts(BANNER_DESK.as_bytes());
        }
    }

    // desktop autostart: desktop.cfg "desktop=1" (written by setup-tabletop)
    // gated on workstation edition; e1dm is the display manager and starts
    // the configured session (e1wm / exaltop)
    #[cfg(not(edition = "server"))]
    {
        let mut cbuf = [0u8; 256];
        let n = read_file("desktop.cfg", &mut cbuf);
        if n > 0 {
            let cfg = core::str::from_utf8(&cbuf[..n as usize]).unwrap_or("");
            if cfg.lines().any(|l| l.trim() == "desktop=1") {
                if need_activation() {
                    // don't autostart the display manager when unlicensed
                } else {
                    puts(b"desktop.cfg: desktop=1, starting display manager...\n");
                    exec("e1dm");
                }
            }
        }
    }

    let mut buf = [0u8; 128];
    let mut obuf = [0u8; 4096];
    loop {
        puts(b"e1sh> ");
        let n = read(0, &mut buf);
        if n <= 0 {
            continue;
        }
        let line = core::str::from_utf8(&buf[..n as usize]).unwrap_or("");
        let mut it = line.split_whitespace();
        match it.next() {
            None => {}
            Some("help") => {
                #[cfg(edition = "server")]
                puts(b"commands: help echo ls cat write ps uptime hello rit wine setup-tabletop activate status\n");
                #[cfg(not(edition = "server"))]
                puts(b"commands: help echo ls cat write ps uptime hello rit wine gui vfsx setup-tabletop e1apfs activate status\n");
            }
            Some("echo") => {
                for w in it {
                    puts(w.as_bytes());
                    puts(b" ");
                }
                puts(b"\n");
            }
            Some("ls") => {
                let n = ls(&mut obuf);
                if n < 0 {
                    puts(b"ls: fs error\n");
                } else {
                    let m = (n as usize).min(obuf.len());
                    write(1, &obuf[..m]);
                }
            }
            Some("cat") => match it.next() {
                Some(f) => match read_file(f, &mut obuf) {
                    n if n >= 0 => {
                        let m = (n as usize).min(obuf.len());
                        write(1, &obuf[..m]);
                        if m > 0 && obuf[m - 1] != b'\n' {
                            puts(b"\n");
                        }
                    }
                    _ => puts(b"cat: not found\n"),
                },
                None => puts(b"usage: cat <file>\n"),
            },
            Some("write") => {
                let f = it.next();
                let rest = line
                    .splitn(3, ' ')
                    .nth(2)
                    .unwrap_or("")
                    .as_bytes();
                match f {
                    Some(f) if !rest.is_empty() => {
                        let r = create_file(f, rest);
                        if r == 0 {
                            puts(b"ok\n");
                        } else {
                            puts(b"write: failed\n");
                        }
                    }
                    _ => puts(b"usage: write <file> <text>\n"),
                }
            }
            Some("ps") => {
                let n = ps(&mut obuf);
                if n >= 0 {
                    let m = (n as usize).min(obuf.len());
                    write(1, &obuf[..m]);
                }
            }
            Some("uptime") => {
                let ms = gettimeofday();
                puts(b"uptime ms: ");
                let mut digits = [0u8; 20];
                let mut i = digits.len();
                let mut v = ms;
                if v == 0 {
                    digits[i - 1] = b'0';
                    i -= 1;
                }
                while v > 0 {
                    i -= 1;
                    digits[i] = b'0' + (v % 10) as u8;
                    v /= 10;
                }
                write(1, &digits[i..]);
                puts(b"\n");
            }
            Some("hello") => {
                let pid = exec("hello");
                if pid < 0 {
                    puts(b"exec: failed\n");
                } else {
                    puts(b"[hello exited, pid ");
                    let mut digits = [0u8; 8];
                    let mut i = digits.len();
                    let mut v = pid as u64;
                    if v == 0 {
                        digits[i - 1] = b'0';
                        i -= 1;
                    }
                    while v > 0 {
                        i -= 1;
                        digits[i] = b'0' + (v % 10) as u8;
                        v /= 10;
                    }
                    write(1, &digits[i..]);
                    puts(b"]\n");
                }
            }
            #[cfg(not(edition = "server"))]
            Some("gui") => {
                if need_activation() { continue; }
                let pid = exec("e1wm");
                if pid < 0 {
                    puts(b"gui: failed (is e1wm on the fs?)\n");
                }
            }
            #[cfg(not(edition = "server"))]
            Some("vfsx") => {
                if need_activation() { continue; }
                // e1vfsx <file.com>: pass the file name via sys_exec_arg
                match it.next() {
                    Some(f) => {
                        let pid = exec_arg("e1vfsx", f);
                        if pid < 0 {
                            puts(b"vfsx: failed (is e1vfsx on the fs?)\n");
                        }
                    }
                    None => {
                        let pid = exec("e1vfsx");
                        if pid < 0 {
                            puts(b"vfsx: failed\n");
                        }
                    }
                }
            }
            Some("setup-tabletop") => {
                if need_activation() { continue; }
                setup_tabletop();
            }
            #[cfg(not(edition = "server"))]
            Some("e1apfs") => {
                if need_activation() { continue; }
                let pid = exec("e1apfs");
                if pid < 0 {
                    puts(b"e1apfs: failed (is e1apfs on the fs?)\n");
                }
            }
            Some("status") => {
                #[cfg(brand = "desk")]
                { puts(b"DesKOS (open source MIT) -- all features available\n"); }
                #[cfg(not(brand = "desk"))]
                {
                    if brand_status() > 0 {
                        puts(b"tAbLeOS -- ACTIVATED (premium features unlocked)\n");
                    } else {
                        puts(b"tAbLeOS -- UNLICENSED (DesKOS fallback)\nrun 'activate' to unlock\n");
                    }
                }
            }
            Some("activate") => {
                #[cfg(brand = "desk")]
                {
                    puts(b"DesKOS is open source -- no activation needed\n");
                }
                #[cfg(not(brand = "desk"))]
                {
                    if brand_status() > 0 {
                        puts(b"system is already activated\n");
                        continue;
                    }
                    puts(b"enter activation key (format: XXXX-XXXX-XXXX-XXXX): \n");
                    let mut kbuf = [0u8; 64];
                    let n = read(0, &mut kbuf);
                    let key = line_of(&kbuf[..n.max(0) as usize]);
                    let mut serial = [0u8; 4];
                    let rc = try_activate(key, &mut serial);
                    if rc != 0 {
                        puts(b"activation failed: invalid key\n");
                        continue;
                    }
                    let serial_str = if &serial[..4] == b"PERM" {
                        &b"PERM"[..]
                    } else {
                        // 2-digit serial
                        &serial[..2]
                    };
                    // write license.cfg -- this both marks activated and
                    // records the consumed serial to prevent one-time reuse
                    let mut out = [0u8; 64];
                    let mut q = 0usize;
                    for part in [
                        &b"activated=1\n"[..],
                        &b"serial="[..],
                        serial_str,
                        &b"\n"[..],
                    ] {
                        out[q..q + part.len()].copy_from_slice(part);
                        q += part.len();
                    }
                    if create_file("license.cfg", &out[..q]) == 0 {
                        puts(b"activation successful! license.cfg written\n");
                        puts(b"tAbLeOS premium features are now unlocked\n");
                    } else {
                        puts(b"activation: fs write failed\n");
                    }
                }
            }
            Some("rit") => match it.next() {
                Some(pkg) => {
                    if rit_install(pkg) == 0 {
                        puts(b"rit: installed\n");
                    } else {
                        puts(b"rit: install failed\n");
                    }
                }
                None => puts(b"usage: rit <file.rit>\n"),
            },
            Some("wine") | Some("e1wine") => {
                let mut argv: [&str; 8] = [""; 8];
                let mut n = 0usize;
                for (i, w) in line.split_whitespace().enumerate() {
                    if i == 0 {
                        continue;
                    }
                    if n < 8 {
                        argv[n] = w;
                        n += 1;
                    }
                }
                if n == 0 {
                    puts(b"usage: wine <linux-program> [args...]\n");
                } else {
                    let pid = wine_execv(&argv[..n]);
                    if pid < 0 {
                        puts(b"e1wine: failed\n");
                    } else {
                        puts(b"[wine exited, pid ");
                        let mut digits = [0u8; 8];
                        let mut i = digits.len();
                        let mut v = pid as u64;
                        if v == 0 {
                            digits[i - 1] = b'0';
                            i -= 1;
                        }
                        while v > 0 {
                            i -= 1;
                            digits[i] = b'0' + (v % 10) as u8;
                            v /= 10;
                        }
                        write(1, &digits[i..]);
                        puts(b"]\n");
                    }
                }
            }
            Some(cmd) => {
                puts(b"unknown command: ");
                puts(cmd.as_bytes());
                puts(b"\n");
            }
        }
    }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    exit(1);
}
