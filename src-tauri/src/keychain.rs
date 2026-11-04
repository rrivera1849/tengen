//! Server sign-ins (OGS tokens, KGS and Pandanet passwords) and the Claude API
//! key, kept in the macOS Keychain and nowhere else.

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

pub fn load_claude_key() -> Option<String> {
    entry("claude-api-key")?
        .get_password()
        .ok()
        .filter(|k| !k.trim().is_empty())
}

pub fn save_claude_key(key: &str) -> Result<(), String> {
    entry("claude-api-key")
        .ok_or("Keychain isn't available")?
        .set_password(key.trim())
        .map_err(|e| format!("couldn't save the API key to Keychain: {e}"))
}

pub fn forget_claude_key() {
    if let Some(e) = entry("claude-api-key") {
        let _ = e.delete_credential();
    }
}

pub fn forget_ogs() {
    if let Some(e) = entry("ogs") {
        let _ = e.delete_credential();
    }
}

/// The Keychain account name for a password login (KGS, Pandanet).
fn login_key(server: tengen_core::adapter::Server) -> String {
    format!("login-{}", server.name().to_lowercase())
}

pub fn load_login(
    server: tengen_core::adapter::Server,
) -> Option<tengen_core::adapter::Credentials> {
    let json = entry(&login_key(server))?.get_password().ok()?;
    serde_json::from_str(&json).ok()
}

pub fn save_login(
    server: tengen_core::adapter::Server,
    credentials: &tengen_core::adapter::Credentials,
) -> Result<(), String> {
    let json = serde_json::to_string(credentials).map_err(|e| e.to_string())?;
    entry(&login_key(server))
        .ok_or("Keychain isn't available")?
        .set_password(&json)
        .map_err(|e| format!("couldn't save the sign-in to Keychain: {e}"))
}

pub fn forget_login(server: tengen_core::adapter::Server) {
    if let Some(e) = entry(&login_key(server)) {
        let _ = e.delete_credential();
    }
}
