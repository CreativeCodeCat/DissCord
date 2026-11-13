//! The saved login token, kept in the system keychain (Keychain on macOS, Credential Manager on
//! Windows, Secret Service on Linux) rather than in the client's data file.

const SERVICE: &str = "DissCord";

/// Saves the token for an account, replacing any earlier one.
pub fn store(name: &str, token: &str) -> Result<(), String> {
    keyring::Entry::new(SERVICE, name)
        .and_then(|entry| entry.set_password(token))
        .map_err(|e| e.to_string())
}

/// The saved token for an account, if the keychain has one.
pub fn load(name: &str) -> Option<String> {
    keyring::Entry::new(SERVICE, name).ok()?.get_password().ok()
}

/// Removes the saved token for an account. Does nothing if there is none.
pub fn forget(name: &str) {
    if let Ok(entry) = keyring::Entry::new(SERVICE, name) {
        let _ = entry.delete_credential();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn stores_loads_and_forgets() {
        let name = "disscord-test-account";
        super::forget(name);
        assert_eq!(super::load(name), None);
        super::store(name, "secret-token").expect("keychain available");
        assert_eq!(super::load(name).as_deref(), Some("secret-token"));
        super::forget(name);
        assert_eq!(super::load(name), None);
    }
}
