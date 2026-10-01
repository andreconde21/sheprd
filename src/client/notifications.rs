use std::io;

use tracing::{debug, warn};

use crate::protocol::NotifyKind;

use super::shell;

pub(super) fn handle_shell_notification_effects(
    effects: Vec<shell::ClientShellNotificationEffect>,
    sound_config: &crate::config::SoundConfig,
) {
    for effect in effects {
        match effect {
            shell::ClientShellNotificationEffect::Sound { sound, agent } => {
                let agent = agent.as_deref().and_then(crate::detect::parse_agent_label);
                if sound_config.allows(agent) {
                    crate::sound::play(sound, sound_config);
                }
            }
            shell::ClientShellNotificationEffect::Terminal { title, body } => {
                if let Err(err) = crate::terminal_notify::show_notification(&title, body.as_deref())
                {
                    warn!(err = %err, "failed to emit terminal notification");
                }
            }
            shell::ClientShellNotificationEffect::System {
                title,
                body,
                click: Some((endpoint_id, pane_id)),
            } => sheprd_clickable_notification(title, body, endpoint_id, pane_id),
            shell::ClientShellNotificationEffect::System { title, body, .. } => {
                if let Err(err) =
                    crate::platform::show_desktop_notification(&title, body.as_deref())
                {
                    warn!(err = %err, "failed to emit system notification");
                }
            }
        }
    }
}

pub(super) fn handle_notify(
    kind: NotifyKind,
    message: &str,
    body: Option<&str>,
    sound_config: &crate::config::SoundConfig,
) {
    handle_notify_with_notifiers(
        kind,
        message,
        body,
        sound_config,
        crate::terminal_notify::show_notification,
        crate::platform::show_desktop_notification,
    );
}

pub(super) fn handle_notify_with_notifiers(
    kind: NotifyKind,
    message: &str,
    body: Option<&str>,
    sound_config: &crate::config::SoundConfig,
    mut show_terminal_notification: impl FnMut(&str, Option<&str>) -> io::Result<bool>,
    mut show_system_notification: impl FnMut(&str, Option<&str>) -> io::Result<bool>,
) {
    match kind {
        NotifyKind::Sound => {
            let Some(sound) = sound_from_notify_message(message) else {
                warn!(
                    message = message,
                    "received unknown sound notification from server"
                );
                return;
            };
            if sound_config.enabled {
                crate::sound::play(sound, sound_config);
            }
        }
        NotifyKind::Toast => {
            debug!(
                message = message,
                "received terminal toast notification from server"
            );
            if let Err(err) = show_terminal_notification(message, body) {
                warn!(err = %err, "failed to emit terminal notification");
            }
        }
        NotifyKind::SystemToast => {
            debug!(
                message = message,
                "received system toast notification from server"
            );
            if let Err(err) = show_system_notification(message, body) {
                warn!(err = %err, "failed to emit system notification");
            }
        }
    }
}

pub(super) fn sound_from_notify_message(message: &str) -> Option<crate::sound::Sound> {
    match message {
        "agent done" => Some(crate::sound::Sound::Done),
        "agent attention" => Some(crate::sound::Sound::Request),
        _ => None,
    }
}

/// andreconde fork (sheprd): a desktop notification you can click. notify-send
/// waits (on its own thread) for the click; clicking raises the terminal running
/// sheprd and focuses the agent. Falls back to a plain notification.
fn sheprd_clickable_notification(
    title: String,
    body: Option<String>,
    endpoint_id: super::endpoint::ClientEndpointId,
    pane_id: String,
) {
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return;
    }
    std::thread::spawn(move || {
        let mut command = std::process::Command::new("notify-send");
        command
            .arg("--app-name")
            .arg("sheprd")
            .arg("--action=default=Open")
            .arg("--wait")
            .arg("--")
            .arg(&title);
        if let Some(body) = body.as_deref().filter(|body| !body.is_empty()) {
            command.arg(body);
        }
        let Ok(output) = command
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
        else {
            return;
        };
        if String::from_utf8_lossy(&output.stdout).trim() == "default" {
            raise_own_terminal();
            super::shell::request_agent_focus(endpoint_id, pane_id);
        }
    });
}

/// Focus the terminal window this client runs in (Hyprland): walk up our
/// process tree until a pid matches a Hyprland client window.
fn raise_own_terminal() {
    let Ok(output) = std::process::Command::new("hyprctl")
        .args(["clients", "-j"])
        .output()
    else {
        return;
    };
    let Ok(clients) = serde_json::from_slice::<Vec<serde_json::Value>>(&output.stdout) else {
        return;
    };
    let window_pids = clients
        .iter()
        .filter_map(|client| client.get("pid").and_then(serde_json::Value::as_u64))
        .collect::<std::collections::HashSet<_>>();
    let mut pid = u64::from(std::process::id());
    for _ in 0..32 {
        if window_pids.contains(&pid) {
            let _ = std::process::Command::new("hyprctl")
                .args(["dispatch", "focuswindow", &format!("pid:{pid}")])
                .output();
            return;
        }
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            return;
        };
        // ppid is the 2nd field after the parenthesised command name.
        let Some(ppid) = stat
            .rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().nth(1))
            .and_then(|ppid| ppid.parse::<u64>().ok())
        else {
            return;
        };
        if ppid <= 1 {
            return;
        }
        pid = ppid;
    }
}
