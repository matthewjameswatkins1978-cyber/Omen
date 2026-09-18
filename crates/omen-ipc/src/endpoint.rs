#[cfg(not(windows))]
use std::path::PathBuf;

pub const DEFAULT_SOCKET_NAME: &str = "omend.sock";

/// Returns the canonical endpoint address for this user / machine.
/// If `OMEN_IPC_ENDPOINT` is set in the environment, its value is preferred.
pub fn default_endpoint_address() -> String {
    if let Ok(override_addr) = std::env::var("OMEN_IPC_ENDPOINT") {
        let trimmed = override_addr.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    platform_default_endpoint()
}

#[cfg(windows)]
fn platform_default_endpoint() -> String {
    let username = std::env::var("USERNAME")
        .unwrap_or_else(|_| "default".to_string())
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect::<String>();
    format!(r"\\.\pipe\omen-{username}-1")
}

#[cfg(not(windows))]
fn platform_default_endpoint() -> String {
    if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR") {
        let path = PathBuf::from(xdg)
            .join("omen")
            .join("1")
            .join(DEFAULT_SOCKET_NAME);
        return path.to_string_lossy().to_string();
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".omen")
        .join("run")
        .join("1")
        .join(DEFAULT_SOCKET_NAME)
        .to_string_lossy()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_endpoint_address_not_empty() {
        let addr = default_endpoint_address();
        assert!(!addr.is_empty());
    }
}
