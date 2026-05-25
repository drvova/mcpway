pub fn should_attempt_browser_launch() -> bool {
    let display = std::env::var("DISPLAY").ok();
    let wayland_display = std::env::var("WAYLAND_DISPLAY").ok();
    should_attempt_browser_launch_with_values(display.as_deref(), wayland_display.as_deref())
}

fn should_attempt_browser_launch_with_values(
    display: Option<&str>,
    wayland_display: Option<&str>,
) -> bool {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        is_non_empty(display) || is_non_empty(wayland_display)
    }

    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        let _ = (display, wayland_display);
        true
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn is_non_empty(value: Option<&str>) -> bool {
    value.is_some_and(|inner| !inner.trim().is_empty())
}

pub fn try_open_browser(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .status()
            .map_err(|err| format!("Failed to launch browser via open: {err}"))?;
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .status()
            .map_err(|err| format!("Failed to launch browser via cmd /C start: {err}"))?;
        return Ok(());
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .status()
            .map_err(|err| format!("Failed to launch browser via xdg-open: {err}"))?;
        return Ok(());
    }

    #[allow(unreachable_code)]
    Err("Automatic browser open is unsupported on this platform".to_string())
}

#[cfg(test)]
mod tests {
    #[cfg(all(unix, not(target_os = "macos")))]
    use super::*;

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn launch_policy_accepts_display_session() {
        assert!(should_attempt_browser_launch_with_values(Some(":0"), None));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn launch_policy_accepts_wayland_session() {
        assert!(should_attempt_browser_launch_with_values(
            None,
            Some("wayland-0")
        ));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn launch_policy_rejects_headless_session() {
        assert!(!should_attempt_browser_launch_with_values(None, None));
        assert!(!should_attempt_browser_launch_with_values(
            Some("  "),
            Some("")
        ));
    }
}
