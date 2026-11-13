#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|d: &[u8]| {
    let _ = vc_convert::pickle::load(d);
});
