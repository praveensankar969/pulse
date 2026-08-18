use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::ValidationError;

pub const DEFAULT_INTERVAL_SEC: u32 = 60;
pub const DEFAULT_TIMEOUT_MS: u32 = 10_000;
pub const DEFAULT_FAIL_THRESHOLD: u32 = 3;
pub const DEFAULT_HOTKEY: &str = "CommandOrControl+Shift+U";

/// Settings help, verbatim. A still-up homelab host keeps Pulse online.
pub const MIXED_REACHABILITY_HELP: &str = "If any check succeeds, Pulse assumes the network is up. A homelab box that still answers will keep Pulse online even if the public internet is gone.";

/// Settings help, verbatim. A still-up homelab host keeps Pulse online.
pub const MIXED_REACHABILITY_HELP: &str = "If any check succeeds, Pulse assumes the network is up. A homelab box that still answers will keep Pulse online even if the public internet is gone.";

use super::{MAX_INTERVAL_SEC, MIN_INTERVAL_SEC};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Dark,
    Light,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuietHours {
    pub start: String,
    pub end: String,
    pub days: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppSettings {
    pub launch_at_login: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hotkey: Option<String>,
    pub theme: Theme,
    pub default_interval: u32,
    pub default_timeout_ms: u32,
    pub fail_threshold: u32,
    pub notifications: bool,
    pub sound: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quiet_hours: Option<QuietHours>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_export_at: Option<DateTime<Utc>>,
    pub asked_launch_at_login: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            launch_at_login: false,
            hotkey: None,
            theme: Theme::System,
            default_interval: DEFAULT_INTERVAL_SEC,
            default_timeout_ms: DEFAULT_TIMEOUT_MS,
            fail_threshold: DEFAULT_FAIL_THRESHOLD,
            notifications: true,
            sound: true,
            quiet_hours: None,
            last_export_at: None,
            asked_launch_at_login: false,
        }
    }
}

impl QuietHours {
    /// Weekdays 22:00–08:00. Overnight Friday continues into Saturday morning.
    pub fn weekdays_overnight() -> Self {
        Self {
            start: "22:00".into(),
            end: "08:00".into(),
            days: vec![1, 2, 3, 4, 5],
        }
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        if !valid_hhmm(&self.start) || !valid_hhmm(&self.end) {
            return Err(ValidationError::QuietHours);
        }
        if self.days.len() > 7 {
            return Err(ValidationError::QuietHours);
        }
        let mut seen = [false; 7];
        for day in &self.days {
            if *day > 6 {
                return Err(ValidationError::QuietHours);
            }
            if seen[*day as usize] {
                return Err(ValidationError::QuietHours);
            }
            seen[*day as usize] = true;
        }
        Ok(())
    }
}

impl AppSettings {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.default_interval < MIN_INTERVAL_SEC {
            return Err(ValidationError::IntervalTooSmall {
                min: MIN_INTERVAL_SEC,
                got: self.default_interval,
            });
        }
        if self.default_interval > MAX_INTERVAL_SEC {
            return Err(ValidationError::IntervalTooLarge {
                max: MAX_INTERVAL_SEC,
                got: self.default_interval,
            });
        }
        if !(500..=60_000).contains(&self.default_timeout_ms) {
            return Err(ValidationError::TimeoutMs {
                got: self.default_timeout_ms,
            });
        }
        if !(1..=10).contains(&self.fail_threshold) {
            return Err(ValidationError::FailThreshold {
                got: self.fail_threshold,
            });
        }
        if let Some(hotkey) = &self.hotkey {
            if hotkey.len() > 64 {
                return Err(ValidationError::Hotkey);
            }
        }
        if let Some(quiet_hours) = &self.quiet_hours {
            quiet_hours.validate()?;
        }
        Ok(())
    }
}

/// None / blank means the default global hotkey.
pub fn resolved_hotkey(settings: &AppSettings) -> String {
    match settings.hotkey.as_deref().map(str::trim) {
        None | Some("") => DEFAULT_HOTKEY.to_string(),
        Some(value) => value.to_string(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchPromptAction {
    Skip,
    MarkAsked,
    Ask,
}

/// One prompt after the first service is saved. Checking the box yourself counts as the answer.
pub fn launch_prompt_action(settings: &AppSettings) -> LaunchPromptAction {
    if settings.asked_launch_at_login {
        LaunchPromptAction::Skip
    } else if settings.launch_at_login {
        LaunchPromptAction::MarkAsked
    } else {
        LaunchPromptAction::Ask
    }
}

pub fn apply_launch_prompt(settings: &mut AppSettings, enable: bool) {
    settings.asked_launch_at_login = true;
    if enable {
        settings.launch_at_login = true;
    }
}

fn valid_hhmm(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 5 || bytes[2] != b':' {
        return false;
    }
    let Ok(hour) = value[0..2].parse::<u8>() else {
        return false;
    };
    let Ok(minute) = value[3..5].parse::<u8>() else {
        return false;
    };
    hour <= 23 && minute <= 59
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_reachability_help_is_verbatim() {
        assert_eq!(
            MIXED_REACHABILITY_HELP,
            "If any check succeeds, Pulse assumes the network is up. A homelab box that still answers will keep Pulse online even if the public internet is gone."
        );
    }

    #[test]
    fn default_hotkey_is_command_or_control_shift_u() {
        assert_eq!(DEFAULT_HOTKEY, "CommandOrControl+Shift+U");
        assert_eq!(resolved_hotkey(&AppSettings::default()), DEFAULT_HOTKEY);
        let custom = AppSettings {
            hotkey: Some("CommandOrControl+Shift+P".into()),
            ..AppSettings::default()
        };
        assert_eq!(resolved_hotkey(&custom), "CommandOrControl+Shift+P");
        let blank = AppSettings {
            hotkey: Some("  ".into()),
            ..AppSettings::default()
        };
        assert_eq!(resolved_hotkey(&blank), DEFAULT_HOTKEY);
    }

    #[test]
    fn launch_prompt_fires_once_after_first_save() {
        let mut settings = AppSettings::default();
        assert!(!settings.launch_at_login);
        assert_eq!(launch_prompt_action(&settings), LaunchPromptAction::Ask);
        apply_launch_prompt(&mut settings, false);
        assert!(settings.asked_launch_at_login);
        assert!(!settings.launch_at_login);
        assert_eq!(launch_prompt_action(&settings), LaunchPromptAction::Skip);
    }

    #[test]
    fn enabling_launch_at_login_marks_asked() {
        let settings = AppSettings {
            launch_at_login: true,
            ..AppSettings::default()
        };
        assert_eq!(
            launch_prompt_action(&settings),
            LaunchPromptAction::MarkAsked
        );
    }
}
