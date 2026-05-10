/// Default JSON database path.
pub const DATABASE_FILE: &str = "database.json";

/// Encrypted JSON database path.
pub const DATABASE_ENCRYPTED_FILE: &str = "database.json.enc";

/// Default Telegram session path.
pub const DEFAULT_SESSION_FILE: &str = "fly-telegram.session";

/// Encrypted default Telegram session path.
pub const DEFAULT_SESSION_ENCRYPTED_FILE: &str = "fly-telegram.session.enc";

/// Directory containing hot-reloaded Lua modules.
pub const MODULES_DIR: &str = "modules";

/// Directory containing additional Telegram session files.
pub const SESSIONS_DIR: &str = "sessions";

/// Required URL scheme for SOCKS5 proxies.
pub const SOCKS5_SCHEME: &str = "socks5://";

/// Database keys used by Rust and Lua code.
pub mod db_key {
    pub const API_HASH: &str = "api_hash";
    pub const API_ID: &str = "api_id";
    pub const PHONE: &str = "phone";
    pub const PROXY_URL: &str = "proxy_url";
    pub const SESSION_FILE: &str = "session_file";
    pub const SESSION_FILES: &str = "session_files";
}

/// Environment variables used by the userbot.
pub mod env_key {
    pub const FLY_MASTER_PASSWORD: &str = "FLY_MASTER_PASSWORD";
    pub const TELOXIDE_TOKEN: &str = "TELOXIDE_TOKEN";
}
