#![forbid(unsafe_code)]

use protocol::Mode;

pub const RESET: &str = "\x1b[0m";

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub primary: &'static str,
    pub accent: &'static str,
    pub warn: &'static str,
    pub flash: &'static str,
}

impl Theme {
    pub fn for_mode(mode: Mode) -> Self {
        match mode {
            Mode::Training => Self {
                primary: "\x1b[38;5;46m",
                accent: "\x1b[38;5;120m",
                warn: "\x1b[38;5;154m",
                flash: "\x1b[48;5;22m\x1b[38;5;46m",
            },
            Mode::NetCity => Self {
                primary: "\x1b[38;5;141m",
                accent: "\x1b[38;5;213m",
                warn: "\x1b[38;5;177m",
                flash: "\x1b[48;5;53m\x1b[38;5;213m",
            },
            Mode::Redline => Self {
                primary: "\x1b[38;5;196m",
                accent: "\x1b[38;5;203m",
                warn: "\x1b[38;5;210m",
                flash: "\x1b[5m\x1b[38;5;15m\x1b[48;5;160m",
            },
        }
    }
}

pub fn mode_banner(mode: Mode, flash_enabled: bool) -> String {
    let theme = Theme::for_mode(mode.clone());
    let header = match mode {
        Mode::Training => "SOLO TRAINING SIM",
        Mode::NetCity => "MULTIPLAYER NETCITY MMO",
        Mode::Redline => "REDLINE // 5:00 COUNTDOWN",
    };

    let prefix = if mode == Mode::Redline && flash_enabled {
        theme.flash
    } else {
        theme.primary
    };

    format!(
        "{prefix}\n╔══════════════════════════════════════╗\n║ {:<36} ║\n╚══════════════════════════════════════╝\n{RESET}",
        header
    )
}

pub fn mode_switch_banner(from: Mode, to: Mode) -> String {
    let text = format!("MODE SWITCH: {} → {}", from.as_label(), to.as_label());
    let theme = Theme::for_mode(to);
    format!("{}{}{}\n", theme.accent, text, RESET)
}

pub fn lore_message(mode: Mode) -> &'static str {
    match mode {
        Mode::Training => "Welcome to CorpSim Onboarding. This simulation recruits real hunters.",
        Mode::NetCity => "NetCity is live. Megacorps are watching. Keep your traces cold.",
        Mode::Redline => "REDLINE active. Five minutes. One breach window. No hesitation.",
    }
}
