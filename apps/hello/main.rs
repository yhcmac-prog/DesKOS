//! hello: simplest ring3 test program for tAbLeOS.

use libe1::*;

#[no_mangle]
pub extern "C" fn main() -> i32 {
    println!("hello from ring3! pid={}", getpid());
    0
}
