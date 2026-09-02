/// Save the WFM access token to Windows Credential Manager (encrypted by the OS).
/// Stored under "FrameForge_WFM" — username field = the email, blob = the token.
#[tauri::command]
#[cfg(target_os = "windows")]
pub(crate) fn wfm_save_credentials(email: String, token: String) -> Result<(), String> {
    use windows_sys::Win32::Security::Credentials::{
        CredWriteW, CREDENTIALW, CRED_TYPE_GENERIC, CRED_PERSIST_LOCAL_MACHINE,
    };
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    let target: Vec<u16> = OsStr::new("FrameForge_WFM").encode_wide().chain(Some(0)).collect();
    let user:   Vec<u16> = OsStr::new(&email).encode_wide().chain(Some(0)).collect();
    let token_bytes = token.as_bytes();

    let cred = CREDENTIALW {
        Flags: 0,
        Type: CRED_TYPE_GENERIC,
        TargetName: target.as_ptr() as *mut _,
        Comment: std::ptr::null_mut(),
        LastWritten: unsafe { std::mem::zeroed() },
        CredentialBlobSize: token_bytes.len() as u32,
        CredentialBlob: token_bytes.as_ptr() as *mut _,
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        AttributeCount: 0,
        Attributes: std::ptr::null_mut(),
        TargetAlias: std::ptr::null_mut(),
        UserName: user.as_ptr() as *mut _,
    };
    let ok = unsafe { CredWriteW(&cred, 0) };
    if ok == 0 { Err("Failed to save to Windows Credential Manager".into()) } else { Ok(()) }
}

/// Load WFM credentials from Windows Credential Manager.
#[tauri::command]
#[cfg(target_os = "windows")]
pub(crate) fn wfm_load_credentials() -> Result<Option<(String, String)>, String> {
    use windows_sys::Win32::Security::Credentials::{
        CredReadW, CredFree, CREDENTIALW, CRED_TYPE_GENERIC,
    };
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::slice;

    let target: Vec<u16> = OsStr::new("FrameForge_WFM").encode_wide().chain(Some(0)).collect();
    let mut cred_ptr: *mut CREDENTIALW = std::ptr::null_mut();
    let ok = unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut cred_ptr) };
    if ok == 0 || cred_ptr.is_null() { return Ok(None); }

    let cred = unsafe { &*cred_ptr };
    let email = unsafe {
        let ptr = cred.UserName;
        if ptr.is_null() { String::new() } else {
            let len = (0..).take_while(|&i| *ptr.offset(i) != 0).count();
            String::from_utf16_lossy(slice::from_raw_parts(ptr, len))
        }
    };
    let token = unsafe {
        if cred.CredentialBlob.is_null() || cred.CredentialBlobSize == 0 { String::new() } else {
            String::from_utf8_lossy(slice::from_raw_parts(cred.CredentialBlob, cred.CredentialBlobSize as usize)).to_string()
        }
    };
    unsafe { CredFree(cred_ptr as *mut _); }
    Ok(Some((email, token)))
}

/// Delete saved WFM credentials from Windows Credential Manager.
///
/// Async only so that every platform's delete has one shape for `wfm_logout` to
/// await; `CredDeleteW` itself returns immediately and never prompts.
#[tauri::command]
#[cfg(target_os = "windows")]
pub(crate) async fn wfm_delete_credentials() -> Result<(), String> {
    use windows_sys::Win32::Security::Credentials::{CredDeleteW, CRED_TYPE_GENERIC};
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    let target: Vec<u16> = OsStr::new("FrameForge_WFM").encode_wide().chain(Some(0)).collect();
    unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0); }
    Ok(())
}

// ==============================================================================
// Linux credential storage
// ==============================================================================
//
// gnome-keyring, KWallet and KeePassXC all implement this interface, so one
// client reaches every mainstream desktop. Why this client and not libsecret or
// the `keyring` crate: docs/adr/0001.
//
// The commands are async and hand their D-Bus calls to spawn_blocking, matching
// scan_warframe_credentials above. Unlocking a keyring can put a dialog in front
// of the user for as long as they take to answer, and Tauri runs a sync command
// on the main thread, so a sync version would freeze the window behind the very
// prompt it raised.

#[cfg(target_os = "linux")]
const WFM_SECRET_SERVICE: &str = "FrameForge_WFM";
#[cfg(target_os = "linux")]
const WFM_SECRET_ACCOUNT: &str = "wfm-session";
#[cfg(target_os = "linux")]
const WFM_SECRET_LABEL: &str = "FrameForge: warframe.market session";

/// Deliberately excludes the email. A record is addressed by where it came from
/// and what it is, so changing the account you log in with replaces the entry
/// rather than orphaning one under an address nothing will search for again.
/// `service` is a parameter only so the test can write somewhere the real
/// session cannot be clobbered from.
#[cfg(target_os = "linux")]
fn wfm_secret_key(service: &str) -> std::collections::HashMap<&str, &str> {
    std::collections::HashMap::from([("service", service), ("account", WFM_SECRET_ACCOUNT)])
}

/// `Error::NoResult` is the crate's generic "nothing matched". Only at the
/// default-collection lookup does it pin down something worth telling the user
/// about, so it becomes a distinct case there, while the call that produced it
/// is still in sight.
#[cfg(target_os = "linux")]
#[derive(Debug)]
enum SecretStoreError {
    NoDefaultCollection,
    Service(secret_service::Error),
}

#[cfg(target_os = "linux")]
impl From<secret_service::Error> for SecretStoreError {
    fn from(error: secret_service::Error) -> Self {
        Self::Service(error)
    }
}

#[cfg(target_os = "linux")]
fn wfm_secret_save(service: &str, email: &str, token: &str) -> Result<(), SecretStoreError> {
    let service_connection = secret_service::blocking::SecretService::connect(
        secret_service::EncryptionType::Dh,
    )?;
    let collection = service_connection.get_default_collection().map_err(|error| match error {
        secret_service::Error::NoResult => SecretStoreError::NoDefaultCollection,
        other => SecretStoreError::Service(other),
    })?;
    collection.ensure_unlocked()?;

    let mut attributes = wfm_secret_key(service);
    // Carried purely so the entry is identifiable in Seahorse or KeePassXC,
    // where "wfm-session" alone tells the user nothing about which account it
    // belongs to.
    attributes.insert("email", email);

    collection.create_item(
        WFM_SECRET_LABEL,
        attributes,
        token.as_bytes(),
        true, // replace: one session per install, so a re-save overwrites
        "text/plain",
    )?;
    Ok(())
}

/// The email in the returned pair is not needed to use the session. It is
/// handed back so a re-save can write the same label attribute again instead of
/// replacing it with whatever the caller happens to have to hand.
#[cfg(target_os = "linux")]
fn wfm_secret_load(service: &str) -> Result<Option<(String, String)>, SecretStoreError> {
    let service_connection = secret_service::blocking::SecretService::connect(
        secret_service::EncryptionType::Dh,
    )?;

    // Searching reports locked hits without opening them, which is what keeps
    // startup quiet: a user who never saved a session matches nothing and is
    // never asked for a keyring password.
    let found = service_connection.search_items(wfm_secret_key(service))?;
    let Some(item) = found.unlocked.first().or_else(|| found.locked.first()) else {
        return Ok(None);
    };

    item.ensure_unlocked()?;
    let email = item.get_attributes()?.get("email").cloned().unwrap_or_default();
    let token = String::from_utf8_lossy(&item.get_secret()?).into_owned();
    Ok(Some((email, token)))
}

#[cfg(target_os = "linux")]
fn wfm_secret_delete(service: &str) -> Result<(), SecretStoreError> {
    let service_connection = secret_service::blocking::SecretService::connect(
        secret_service::EncryptionType::Dh,
    )?;
    let found = service_connection.search_items(wfm_secret_key(service))?;
    for item in found.unlocked.iter().chain(found.locked.iter()) {
        item.ensure_unlocked()?;
        item.delete()?;
    }
    Ok(())
}

/// Only the variants a user can act on are worth rewording. The crate's own
/// Display text for them names D-Bus rather than the thing they would recognise
/// as their password manager.
#[cfg(target_os = "linux")]
fn wfm_secret_error(error: SecretStoreError) -> String {
    match error {
        // Creating the keyring is a password-manager decision with its own
        // setup prompt, not something a companion app should do behind the
        // user's back.
        SecretStoreError::NoDefaultCollection =>
            "No default keyring exists. Create one in your password manager first.".into(),
        SecretStoreError::Service(secret_service::Error::Unavailable) =>
            "No password manager is running to keep the session in.".into(),
        SecretStoreError::Service(secret_service::Error::Prompt) =>
            "The keyring unlock prompt was dismissed.".into(),
        SecretStoreError::Service(secret_service::Error::Locked) =>
            "The keyring is locked.".into(),
        SecretStoreError::Service(other) => other.to_string(),
    }
}

#[cfg(target_os = "linux")]
#[tauri::command]
pub(crate) async fn wfm_save_credentials(email: String, token: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || wfm_secret_save(WFM_SECRET_SERVICE, &email, &token))
        .await
        .map_err(|e| e.to_string())?
        .map_err(wfm_secret_error)
}

#[cfg(target_os = "linux")]
#[tauri::command]
pub(crate) async fn wfm_load_credentials() -> Result<Option<(String, String)>, String> {
    tauri::async_runtime::spawn_blocking(move || wfm_secret_load(WFM_SECRET_SERVICE))
        .await
        .map_err(|e| e.to_string())?
        .map_err(wfm_secret_error)
}

#[cfg(target_os = "linux")]
#[tauri::command]
pub(crate) async fn wfm_delete_credentials() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || wfm_secret_delete(WFM_SECRET_SERVICE))
        .await
        .map_err(|e| e.to_string())?
        .map_err(wfm_secret_error)
}

// ==============================================================================
// Credential storage elsewhere
// ==============================================================================
//
// macOS has a Keychain that would serve, but nothing here targets it. Rather
// than fall back to a plaintext file, the save refuses and the load reports no
// saved session.

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
#[tauri::command]
pub(crate) async fn wfm_save_credentials(email: String, token: String) -> Result<(), String> {
    let _ = (email, token);
    Err("Saving the session is not supported on this platform".into())
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
#[tauri::command]
pub(crate) async fn wfm_load_credentials() -> Result<Option<(String, String)>, String> {
    Ok(None)
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
#[tauri::command]
pub(crate) async fn wfm_delete_credentials() -> Result<(), String> {
    Ok(())
}

/// Whether this machine can persist a session at all.
///
/// Unlike the other two capabilities this is not a build-time fact. A Linux
/// desktop only has somewhere to put the session if it runs a Secret Service
/// provider, and a minimal window manager may run none, so the answer has to be
/// asked of the running system rather than the target triple.
///
/// Connecting negotiates a session with the provider without opening a
/// collection, so asking cannot raise an unlock prompt at startup.
#[cfg(target_os = "linux")]
pub(crate) async fn credential_store_available() -> bool {
    tauri::async_runtime::spawn_blocking(|| {
        secret_service::blocking::SecretService::connect(secret_service::EncryptionType::Dh).is_ok()
    })
    .await
    .unwrap_or(false)
}

#[cfg(not(target_os = "linux"))]
pub(crate) async fn credential_store_available() -> bool {
    cfg!(target_os = "windows")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exercises the Secret Service store against a real provider.
    ///
    /// Ignored by default because it needs a running secret service and an
    /// unlockable keyring, neither of which CI has. Run it on a desktop with
    /// `cargo test -- --ignored`.
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore]
    fn secret_store_round_trip() {
        // Its own service name, so a bug here can never overwrite or delete the
        // session the developer is actually logged in with.
        const TEST_SERVICE: &str = "FrameForge_WFM_test";

        // Runs even when an assert unwinds, so a failure cannot leave a stray
        // entry sitting in a real keyring.
        struct Cleanup;
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = wfm_secret_delete(TEST_SERVICE);
            }
        }
        let _cleanup = Cleanup;

        wfm_secret_save(TEST_SERVICE, "player@example.com", "{\"accessToken\":\"abc\"}")
            .expect("saving to an unlocked keyring succeeds");
        assert_eq!(
            wfm_secret_load(TEST_SERVICE).expect("loading a just-saved session succeeds"),
            Some(("player@example.com".to_string(), "{\"accessToken\":\"abc\"}".to_string())),
        );

        // A second save must replace rather than accumulate, since a re-login
        // writes over the same key.
        wfm_secret_save(TEST_SERVICE, "player@example.com", "{\"accessToken\":\"def\"}")
            .expect("re-saving over an existing session succeeds");
        let (_, token) = wfm_secret_load(TEST_SERVICE)
            .expect("loading after a re-save succeeds")
            .expect("the re-saved session is still there");
        assert_eq!(token, "{\"accessToken\":\"def\"}");

        wfm_secret_delete(TEST_SERVICE).expect("deleting an existing session succeeds");
        assert_eq!(
            wfm_secret_load(TEST_SERVICE).expect("loading after a delete succeeds"),
            None,
        );
    }
}
