//! cpal keeps one WASAPI device enumerator for the whole process, created on the first thread that
//! lists devices. COM must stay up after that thread exits, or later cpal calls touch freed COM
//! state (an access violation on machines with no audio endpoints, such as CI runners).
#![cfg(windows)]

use std::thread;

use windows::Win32::System::Com::{
    APTTYPE, APTTYPE_MTA, APTTYPEQUALIFIER, APTTYPEQUALIFIER_IMPLICIT_MTA, CoGetApartmentType,
};

/// The apartment a thread that never initialised COM is in, if COM is up in the process.
fn implicit_apartment() -> Option<(APTTYPE, APTTYPEQUALIFIER)> {
    thread::spawn(|| {
        let mut kind = APTTYPE::default();
        let mut qualifier = APTTYPEQUALIFIER::default();
        // SAFETY: both pointers are valid for the call.
        unsafe { CoGetApartmentType(&mut kind, &mut qualifier) }
            .ok()
            .map(|()| (kind, qualifier))
    })
    .join()
    .unwrap()
}

#[test]
fn com_outlives_the_first_thread_that_lists_devices() {
    thread::spawn(vc_audio::list_devices).join().unwrap();
    assert_eq!(
        implicit_apartment(),
        Some((APTTYPE_MTA, APTTYPEQUALIFIER_IMPLICIT_MTA))
    );
    // and cpal still works from another thread
    thread::spawn(vc_audio::list_devices).join().unwrap();
}
