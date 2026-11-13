#![no_main]

use libfuzzer_sys::fuzz_target;
use vc_convert::checkpoint::Checkpoint;
use vc_convert::pth::PthArchive;

// Opens, classifies and reads every tensor, like a real conversion does.
fuzz_target!(|d: &[u8]| {
    let Ok(mut a) = PthArchive::open(d.to_vec(), "fuzz.pth") else {
        return;
    };
    let Ok(ckpt) = Checkpoint::from_value(&a.root, &a.name) else {
        return;
    };
    for t in ckpt.weights.values() {
        let _ = a.tensor_f32(t);
    }
});
