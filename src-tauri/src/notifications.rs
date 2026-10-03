use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

pub const MAX_BUFFERED_NOTIFICATIONS: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationCategory {
    Security,
    Automation,
    Guard,
    Preset,
    Macro,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationSeverity {
    Info,
    Success,
    Warning,
    Error,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationChannel {
    Toast,
    Modal,
    Balloon,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SoundCue {
    None,
    Start,
    Stop,
    Success,
    Warning,
    Emergency,
    Tick,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationCode {
    // 1. Security (3)
    UipiBlocked,
    AdminElevationRequired,
    AdminRestartInitiated,

    // 2. Automation (9)
    ClickerStarted,
    ClickerStopped,
    EmergencyStopped,
    AutoStopDuration,
    AutoStopWallclock,
    ClickLimitReached,
    StartDelayCountdown,
    SpeedAdjusted,
    ImageTriggerMatched,

    // 3. Guard (5)
    FocusLossPaused,
    TypingGuardBlocked,
    AppFilterBlocked,
    WorkModeBlocked,
    ModeSwitched,

    // 4. Preset (6)
    PresetActivated,
    PresetPreempted,
    PresetSaved,
    PresetDeleted,
    PresetsExported,
    PresetsImported,

    // 5. Macro (6)
    MacroRecordingStarted,
    MacroRecordingSaved,
    MacroPlaybackStarted,
    MacroPlaybackFinished,
    MacroOptimized,
    MacroTemplateAdded,

    // 6. System (7)
    ConfigSaved,
    ConfigSaveFailed,
    ConfigBackupCreated,
    ConfigResetSuccess,
    ExternalFileChanged,
    InputDiagDumped,
    HotkeyClashWarning,
}

impl NotificationCode {
    pub fn default_category(&self) -> NotificationCategory {
        match self {
            Self::UipiBlocked | Self::AdminElevationRequired | Self::AdminRestartInitiated => {
                NotificationCategory::Security
            }
            Self::ClickerStarted
            | Self::ClickerStopped
            | Self::EmergencyStopped
            | Self::AutoStopDuration
            | Self::AutoStopWallclock
            | Self::ClickLimitReached
            | Self::StartDelayCountdown
            | Self::SpeedAdjusted
            | Self::ImageTriggerMatched => NotificationCategory::Automation,
            Self::FocusLossPaused
            | Self::TypingGuardBlocked
            | Self::AppFilterBlocked
            | Self::WorkModeBlocked
            | Self::ModeSwitched => NotificationCategory::Guard,
            Self::PresetActivated
            | Self::PresetPreempted
            | Self::PresetSaved
            | Self::PresetDeleted
            | Self::PresetsExported
            | Self::PresetsImported => NotificationCategory::Preset,
            Self::MacroRecordingStarted
            | Self::MacroRecordingSaved
            | Self::MacroPlaybackStarted
            | Self::MacroPlaybackFinished
            | Self::MacroOptimized
            | Self::MacroTemplateAdded => NotificationCategory::Macro,
            Self::ConfigSaved
            | Self::ConfigSaveFailed
            | Self::ConfigBackupCreated
            | Self::ConfigResetSuccess
            | Self::ExternalFileChanged
            | Self::InputDiagDumped
            | Self::HotkeyClashWarning => NotificationCategory::System,
        }
    }

    pub fn default_severity(&self) -> NotificationSeverity {
        match self {
            Self::ClickerStarted
            | Self::ImageTriggerMatched
            | Self::PresetActivated
            | Self::PresetSaved
            | Self::PresetsExported
            | Self::PresetsImported
            | Self::MacroRecordingSaved
            | Self::MacroPlaybackFinished
            | Self::MacroTemplateAdded
            | Self::ConfigSaved
            | Self::ConfigResetSuccess => NotificationSeverity::Success,

            Self::UipiBlocked
            | Self::EmergencyStopped
            | Self::FocusLossPaused
            | Self::TypingGuardBlocked
            | Self::AppFilterBlocked
            | Self::WorkModeBlocked
            | Self::ExternalFileChanged
            | Self::HotkeyClashWarning => NotificationSeverity::Warning,

            Self::ConfigSaveFailed => NotificationSeverity::Error,

            _ => NotificationSeverity::Info,
        }
    }

    pub fn default_sound(&self) -> SoundCue {
        match self {
            Self::ClickerStarted
            | Self::PresetActivated
            | Self::MacroRecordingStarted
            | Self::MacroPlaybackStarted
            | Self::AdminRestartInitiated => SoundCue::Start,

            Self::ClickerStopped
            | Self::AutoStopDuration
            | Self::AutoStopWallclock
            | Self::ClickLimitReached
            | Self::MacroPlaybackFinished => SoundCue::Stop,

            Self::EmergencyStopped | Self::ConfigSaveFailed => SoundCue::Emergency,

            Self::UipiBlocked
            | Self::FocusLossPaused
            | Self::WorkModeBlocked
            | Self::AppFilterBlocked
            | Self::ExternalFileChanged
            | Self::HotkeyClashWarning => SoundCue::Warning,

            Self::ImageTriggerMatched
            | Self::PresetSaved
            | Self::PresetsExported
            | Self::PresetsImported
            | Self::MacroRecordingSaved
            | Self::ConfigResetSuccess => SoundCue::Success,

            Self::StartDelayCountdown | Self::SpeedAdjusted | Self::ModeSwitched | Self::PresetPreempted => {
                SoundCue::Tick
            }

            _ => SoundCue::None,
        }
    }

    pub fn default_channel(&self) -> NotificationChannel {
        match self {
            Self::UipiBlocked => NotificationChannel::Modal,
            Self::EmergencyStopped
            | Self::FocusLossPaused
            | Self::WorkModeBlocked
            | Self::AutoStopDuration
            | Self::AutoStopWallclock
            | Self::ClickLimitReached
            | Self::ExternalFileChanged
            | Self::ConfigSaveFailed => NotificationChannel::All,
            _ => NotificationChannel::Toast,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppNotification {
    pub id: String,
    pub code: NotificationCode,
    pub category: NotificationCategory,
    pub severity: NotificationSeverity,
    pub channel: NotificationChannel,
    pub sound: SoundCue,
    pub title_key: String,
    pub message_key: String,
    pub default_title: String,
    pub default_message: String,
    pub params: Option<serde_json::Value>,
    pub timestamp_ms: u64,
    pub unread: bool,
}

pub struct NotificationStore {
    buffer: Mutex<VecDeque<AppNotification>>,
}

impl NotificationStore {
    pub fn new() -> Self {
        Self {
            buffer: Mutex::new(VecDeque::with_capacity(MAX_BUFFERED_NOTIFICATIONS)),
        }
    }

    pub fn push(
        &self,
        mut notification: AppNotification,
        app: Option<&AppHandle>,
        show_windows_notifications: bool,
    ) {
        notification.unread = true;

        if let Ok(mut buf) = self.buffer.lock() {
            if buf.len() >= MAX_BUFFERED_NOTIFICATIONS {
                buf.pop_front();
            }
            buf.push_back(notification.clone());
        }

        let is_window_visible = app.map(crate::main_window_visible).unwrap_or(false);

        // Native Windows Tray Balloon & Sound when in tray or critical
        if show_windows_notifications {
            let is_critical = matches!(
                notification.code,
                NotificationCode::UipiBlocked
                    | NotificationCode::EmergencyStopped
                    | NotificationCode::FocusLossPaused
                    | NotificationCode::AutoStopDuration
                    | NotificationCode::AutoStopWallclock
                    | NotificationCode::WorkModeBlocked
                    | NotificationCode::ConfigSaveFailed
            );

            if !is_window_visible || is_critical {
                crate::tray::notify_balloon(&notification.default_title, &notification.default_message);
                play_system_sound(notification.sound);
            }
        }

        // Emit to active WebViews if app is running
        if let Some(app) = app {
            let _ = app.emit("notification-created", &notification);
        }
    }

    pub fn get_all(&self, limit: Option<usize>) -> Vec<AppNotification> {
        let buf = self.buffer.lock().unwrap_or_else(|p| p.into_inner());
        let count = limit.unwrap_or(buf.len()).min(buf.len());
        buf.iter().rev().take(count).cloned().collect()
    }

    pub fn get_unread(&self) -> Vec<AppNotification> {
        let buf = self.buffer.lock().unwrap_or_else(|p| p.into_inner());
        buf.iter().filter(|n| n.unread).cloned().collect()
    }

    pub fn mark_read(&self, ids: &[String]) {
        if ids.is_empty() {
            return;
        }
        if let Ok(mut buf) = self.buffer.lock() {
            for item in buf.iter_mut() {
                if ids.iter().any(|id| id == &item.id) {
                    item.unread = false;
                }
            }
        }
    }

    pub fn clear(&self) {
        if let Ok(mut buf) = self.buffer.lock() {
            buf.clear();
        }
    }
}

impl Default for NotificationStore {
    fn default() -> Self {
        Self::new()
    }
}

static GLOBAL_NOTIFICATION_STORE: std::sync::OnceLock<Arc<NotificationStore>> =
    std::sync::OnceLock::new();

pub fn set_global_notification_store(store: Arc<NotificationStore>) {
    let _ = GLOBAL_NOTIFICATION_STORE.set(store);
}

pub fn get_global_notification_store() -> Option<&'static Arc<NotificationStore>> {
    GLOBAL_NOTIFICATION_STORE.get()
}

/// Helper to push a standardized notification into the global store
pub fn push_notification(
    code: NotificationCode,
    default_title: &str,
    default_message: &str,
    params: Option<serde_json::Value>,
    show_windows_notifications: bool,
) {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let id = format!("notif_{}_{}", now_ms, rand::random::<u16>());

    let notification = AppNotification {
        id,
        code,
        category: code.default_category(),
        severity: code.default_severity(),
        channel: code.default_channel(),
        sound: code.default_sound(),
        title_key: format!("notif_title_{:?}", code).to_lowercase(),
        message_key: format!("notif_msg_{:?}", code).to_lowercase(),
        default_title: default_title.to_string(),
        default_message: default_message.to_string(),
        params,
        timestamp_ms: now_ms,
        unread: true,
    };

    if let Some(store) = get_global_notification_store() {
        store.push(
            notification,
            crate::platform::windows::get_global_app_handle(),
            show_windows_notifications,
        );
    }
}

#[cfg(target_os = "windows")]
extern "system" {
    fn MessageBeep(uType: u32) -> i32;
}

#[cfg(target_os = "windows")]
pub fn play_system_sound(sound: SoundCue) {
    const MB_ICONHAND: u32 = 0x00000010;
    const MB_ICONWARNING: u32 = 0x00000030;
    const MB_ICONASTERISK: u32 = 0x00000040;

    unsafe {
        match sound {
            SoundCue::Warning => {
                let _ = MessageBeep(MB_ICONWARNING);
            }
            SoundCue::Emergency => {
                let _ = MessageBeep(MB_ICONHAND);
            }
            SoundCue::Start | SoundCue::Stop | SoundCue::Success => {
                let _ = MessageBeep(MB_ICONASTERISK);
            }
            _ => {}
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn play_system_sound(_sound: SoundCue) {}
