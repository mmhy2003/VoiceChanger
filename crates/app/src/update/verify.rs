//! Checking a downloaded installer: its SHA-256 and its Authenticode signature.

use std::io::{self, Read};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use sha2::{Digest, Sha256};
use windows_sys::Win32::Foundation::{
    TRUST_E_NOSIGNATURE, TRUST_E_PROVIDER_UNKNOWN, TRUST_E_SUBJECT_FORM_UNKNOWN,
};
use windows_sys::Win32::Security::WinTrust::{
    WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO,
    WTD_CHOICE_FILE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE,
    WinVerifyTrust,
};

use super::client::hex;

/// Lowercase hex SHA-256 of the file, read in chunks.
pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

/// The result of checking a file's Authenticode signature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signature {
    Valid,
    Unsigned,
    /// The signature is present but fails, with the WinVerifyTrust status.
    Invalid(i32),
}

/// 0 → Valid; TRUST_E_NOSIGNATURE / TRUST_E_SUBJECT_FORM_UNKNOWN / TRUST_E_PROVIDER_UNKNOWN → Unsigned; else Invalid.
/// The PE subject-interface package recognises the file by its content, so a `.part` name is fine.
pub fn signature(path: &Path) -> Signature {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut file = WINTRUST_FILE_INFO {
        cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: wide.as_ptr(),
        hFile: std::ptr::null_mut(),
        pgKnownSubject: std::ptr::null_mut(),
    };
    // SAFETY: WINTRUST_DATA is a plain C struct; all-zero is a valid starting value.
    let mut data: WINTRUST_DATA = unsafe { std::mem::zeroed() };
    data.cbStruct = size_of::<WINTRUST_DATA>() as u32;
    data.dwUIChoice = WTD_UI_NONE;
    data.fdwRevocationChecks = WTD_REVOKE_NONE;
    data.dwUnionChoice = WTD_CHOICE_FILE;
    data.Anonymous = WINTRUST_DATA_0 { pFile: &mut file };
    data.dwStateAction = WTD_STATEACTION_VERIFY;
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    // SAFETY: data, file and wide outlive both calls; CLOSE frees the state VERIFY allocated.
    let status =
        unsafe { WinVerifyTrust(std::ptr::null_mut(), &mut action, (&raw mut data).cast()) };
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    // SAFETY: as above.
    unsafe { WinVerifyTrust(std::ptr::null_mut(), &mut action, (&raw mut data).cast()) };
    match status {
        0 => Signature::Valid,
        TRUST_E_NOSIGNATURE | TRUST_E_SUBJECT_FORM_UNKNOWN | TRUST_E_PROVIDER_UNKNOWN => {
            Signature::Unsigned
        }
        other => Signature::Invalid(other),
    }
}

/// A validly signed running app only accepts a validly signed installer. Otherwise only the SHA-256 is checked.
pub fn signature_ok(running: Signature, installer: Signature) -> Result<(), String> {
    match (running, installer) {
        (Signature::Valid, Signature::Valid) => Ok(()),
        (Signature::Valid, Signature::Unsigned) => {
            Err("is not validly signed (unsigned)".to_string())
        }
        (Signature::Valid, Signature::Invalid(code)) => Err(format!(
            "is not validly signed (error 0x{:08X})",
            code as u32
        )),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn sha256_file_matches_known() {
        let dir = TempDir::new("verify-sha");
        let path = dir.path().join("abc.txt");
        std::fs::write(&path, b"abc").unwrap();
        assert_eq!(
            sha256_file(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(sha256_file(&dir.path().join("missing.txt")).is_err());
    }

    #[test]
    fn text_file_is_unsigned() {
        let dir = TempDir::new("verify-txt");
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, b"just some text").unwrap();
        assert_eq!(signature(&path), Signature::Unsigned);
    }

    #[test]
    fn unsigned_exe_is_unsigned() {
        let exe = std::env::current_exe().unwrap();
        assert_eq!(signature(&exe), Signature::Unsigned);
    }

    #[test]
    fn missing_file_not_valid() {
        let dir = TempDir::new("verify-missing");
        assert_ne!(signature(&dir.path().join("nope.exe")), Signature::Valid);
    }

    #[test]
    fn rule_signed_app_needs_signed_installer() {
        assert_eq!(signature_ok(Signature::Valid, Signature::Valid), Ok(()));
        assert_eq!(
            signature_ok(Signature::Valid, Signature::Unsigned),
            Err("is not validly signed (unsigned)".to_string())
        );
        assert_eq!(
            signature_ok(Signature::Valid, Signature::Invalid(0x800B0109_u32 as i32)),
            Err("is not validly signed (error 0x800B0109)".to_string())
        );
    }

    #[test]
    fn rule_unsigned_app_checks_hash_only() {
        assert_eq!(
            signature_ok(Signature::Unsigned, Signature::Unsigned),
            Ok(())
        );
        assert_eq!(signature_ok(Signature::Unsigned, Signature::Valid), Ok(()));
        assert_eq!(
            signature_ok(Signature::Invalid(1), Signature::Unsigned),
            Ok(())
        );
    }
}
