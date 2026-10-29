//! ush: user-space shell for tAbLeOS.

use libe1::*;

#[no_mangle]
pub extern "C" fn main() -> i32 {
    println!("tAbLeOS ush v0.1");
    println!("Type 'exit' to quit.");
    let mut buf = [0u8; 256];
    loop {
        print!("ush$ ");
        let n = read(0, &mut buf);
        if n <= 0 {
            continue;
        }
        let line = core::str::from_utf8(&buf[..n as usize]).unwrap_or("");
        let line = line.trim();
        if line == "exit" {
            break;
        }
        if line.is_empty() {
            continue;
        }
        // just echo back for now
        println!("ush: {}", line);
    }
    println!("bye");
    0
}
