//! Remembered server certificates (trust on first use). Saved between runs, so a server that
//! later presents a different certificate is refused instead of silently trusted.

use std::collections::BTreeMap;
use std::sync::Mutex;

/// Key under which the remembered certificates are stored by eframe.
pub const PINS_KEY: &str = "disscord_pins";

/// Certificate fingerprints by address, for example "server:127.0.0.1:7878" or "auth:127.0.0.1:7900".
pub static PINS: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());

/// Forgets the certificate remembered for `key`, so the next connection remembers the new one.
pub fn forget(key: &str) {
    PINS.lock().unwrap().remove(key);
}

/// Checks the certificate fingerprint seen for `key`. The first one seen is remembered.
pub fn check(key: &str, fingerprint: &str) -> Result<(), String> {
    let mut pins = PINS.lock().unwrap();
    let first = common::tls::check_pin(pins.get(key).map(String::as_str), fingerprint)?;
    if first {
        pins.insert(key.to_string(), fingerprint.to_string());
    }
    Ok(())
}
