//! Server sign-in tokens, kept in the macOS Keychain and nowhere else.

use tengen_ogs::auth::Tokens;

const SERVICE: &str = "io.github.rrivera1849.tengen";

fn entry(account: &str) -> Option<keyring::Entry> {
    keyring::Entry::new(SERVICE, account).ok()
}

pub fn load_ogs() -> Option<Tokens> {
    let json = entry("ogs")?.get_password().ok()?;
    serde_json::from_str(&json).ok()
}

pub fn save_ogs(tokens: &Tokens) -> Result<(), String> {
    let json = serde_json::to_string(tokens).map_err(|e| e.to_string())?;
    entry("ogs")
        .ok_or("Keychain isn't available")?
        .set_password(&json)
        .map_err(|e| format!("couldn't save the sign-in to Keychain: {e}"))
}

pub fn forget_ogs() {
    if let Some(e) = entry("ogs") {
        let _ = e.delete_credential();
    }
}
