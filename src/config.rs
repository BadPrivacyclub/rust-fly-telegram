pub const DATABASE_FILE: &str = "database.json";

pub const DATABASE_ENCRYPTED_FILE: &str = "database.json.enc";

pub const DEFAULT_SESSION_FILE: &str = "fly-telegram.session";

pub const DEFAULT_SESSION_ENCRYPTED_FILE: &str = "fly-telegram.session.enc";

pub const SIGNING_PUB_KEY_FILE: &str = "keys/signing.pub";

pub const SIGNING_KEY_ENC_FILE: &str = "keys/signing.key.enc";

pub const MODULES_DIR: &str = "modules";

pub const SESSIONS_DIR: &str = "sessions";

pub const SOCKS5_SCHEME: &str = "socks5://";

pub mod db_key {
    pub const API_HASH: &str = "api_hash";
    pub const API_ID: &str = "api_id";
    pub const PHONE: &str = "phone";
    pub const PROXY_URL: &str = "proxy_url";
    pub const SESSION_FILE: &str = "session_file";
    pub const SESSION_FILES: &str = "session_files";
}

pub mod env_key {
    pub const FLY_MASTER_PASSWORD: &str = "FLY_MASTER_PASSWORD";
    pub const TELOXIDE_TOKEN: &str = "TELOXIDE_TOKEN";
}
