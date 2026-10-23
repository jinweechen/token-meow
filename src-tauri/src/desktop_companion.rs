//! Ephemeral Windows context and user-selected application launchers.
//! No Codex facts, input contents, document reads, hooks or native movement.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

const IDLE_MS: u64 = 120_000;
const PLAY_MS: u64 = 2_400;
const PLAY_GAP_MS: u64 = 18_000;
const MAX_LAUNCHERS: usize = 16;
const MAX_REGISTRY_BYTES: u64 = 128 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum MotionDirection {
    Left,
    Right,
}

/// The desktop owner supplies actual gait availability and current direction.
/// This policy never grants permission to perform a native window move.
#[derive(Clone, Copy, Debug)]
pub(super) struct RoamingContext {
    pub available: bool,
    pub direction: MotionDirection,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Motion {
    direction: MotionDirection,
    remaining_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Movement {
    pub generation: u64,
    pub remaining: Duration,
    pub direction: MotionDirection,
    /// The native marker whose unchanged interval admitted this burst.
    pub input_marker: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Capabilities {
    system_idle: bool,
    roaming: bool,
    work_apps: bool,
    launchers: bool,
    file_drop: bool,
    window_edges: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ActiveApp {
    id: String,
    label: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DesktopCompanionState {
    runtime_epoch: String,
    revision: String,
    phase: String,
    pause_reason: Option<String>,
    /// Continuous observed inactivity since this owner started or resumed.
    /// It does not claim knowledge of idle time before the first native sample.
    input_idle_ms: Option<u64>,
    active_app: Option<ActiveApp>,
    motion_generation: String,
    motion: Option<Motion>,
    capabilities: Capabilities,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Launcher {
    id: String,
    label: String,
    executable: PathBuf,
    work_app_enabled: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct LauncherSummary {
    id: String,
    label: String,
    work_app_enabled: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct LauncherUpdateOptions {
    launcher_id: String,
    #[serde(default, deserialize_with = "present_launcher_option")]
    label: Option<String>,
    #[serde(default, deserialize_with = "present_launcher_option")]
    work_app_enabled: Option<bool>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum LauncherMoveDirection {
    Up,
    Down,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct LauncherMoveOptions {
    launcher_id: String,
    direction: LauncherMoveDirection,
}

// 省略表示不修改；显式 null 和错误类型不能变成一个被忽略的补丁。
fn present_launcher_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

fn normalized_launcher_label(value: &str) -> Result<String, String> {
    if value.chars().any(|character| character.is_control()
        || matches!(character, '\u{061c}' | '\u{200e}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')) {
        return Err("应用名称不能包含控制字符或双向文字控制字符。".into());
    }
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 80 {
        return Err("应用名称去除首尾空格后须为1至80个字符。".into());
    }
    Ok(trimmed.to_owned())
}

impl Launcher {
    fn summary(&self) -> LauncherSummary {
        LauncherSummary {
            id: self.id.clone(),
            label: self.label.clone(),
            work_app_enabled: self.work_app_enabled,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Registry {
    version: u8,
    entries: Vec<Launcher>,
}

struct IdlePolicy {
    last_input: Option<u32>,
    changed_at: Option<u64>,
    observed_at: Option<u64>,
    play_until: Option<u64>,
    next_play: Option<u64>,
    generation: u64,
    moving_direction: Option<MotionDirection>,
    next_burst_moves: bool,
}

impl Default for IdlePolicy {
    fn default() -> Self {
        Self {
            last_input: None,
            changed_at: None,
            observed_at: None,
            play_until: None,
            next_play: None,
            generation: 0,
            moving_direction: None,
            next_burst_moves: true,
        }
    }
}

struct Observation {
    phase: &'static str,
    reason: Option<&'static str>,
    idle_ms: Option<u64>,
    motion: Option<Motion>,
}
impl IdlePolicy {
    fn reset(&mut self) {
        if self.play_until.take().is_some() {
            self.generation = self.generation.saturating_add(1);
        }
        self.last_input = None;
        self.changed_at = None;
        self.observed_at = None;
        self.next_play = None;
        self.moving_direction = None;
        self.next_burst_moves = true;
    }

    fn movement(&self, now: u64) -> Option<Movement> {
        let remaining = self.play_until?.checked_sub(now)?;
        if remaining == 0 || remaining > PLAY_MS || self.observed_at.is_none_or(|at| now < at) {
            return None;
        }
        Some(Movement {
            generation: self.generation,
            remaining: Duration::from_millis(remaining),
            direction: self.moving_direction?,
            input_marker: self.last_input?,
        })
    }

    fn active_observation(&self, elapsed: u64, now: u64) -> Observation {
        let motion = self.movement(now).map(|movement| Motion {
            direction: movement.direction,
            remaining_ms: movement.remaining.as_millis() as u64,
        });
        Observation {
            phase: if motion.is_some() { "moving" } else { "playing" },
            reason: None,
            idle_ms: Some(elapsed),
            motion,
        }
    }

    fn observe(
        &mut self,
        input: Option<u32>,
        now: u64,
        blocked: Option<&'static str>,
        enabled: bool,
        roaming: RoamingContext,
    ) -> Observation {
        if let Some(reason) = blocked {
            self.reset();
            return Observation {
                phase: "paused",
                reason: Some(reason),
                idle_ms: None,
                motion: None,
            };
        }
        let Some(input) = input else {
            self.reset();
            return Observation {
                phase: "paused",
                reason: Some("inputUnavailable"),
                idle_ms: None,
                motion: None,
            };
        };
        if self.last_input != Some(input)
            || self.changed_at.is_none()
            || self.observed_at.is_some_and(|at| now < at)
            || (self.moving_direction.is_some() && !roaming.available)
        {
            self.reset();
            self.last_input = Some(input);
            self.changed_at = Some(now);
        }
        self.observed_at = Some(now);
        let elapsed = now.saturating_sub(self.changed_at.unwrap_or(now));
        if !roaming.available {
            self.next_burst_moves = true;
        }
        if !enabled {
            if self.play_until.take().is_some() {
                self.generation = self.generation.saturating_add(1);
            }
            self.next_play = None;
            self.moving_direction = None;
            self.next_burst_moves = true;
            return Observation {
                phase: "idle",
                reason: Some("disabled"),
                idle_ms: Some(elapsed),
                motion: None,
            };
        }
        if elapsed < IDLE_MS {
            return Observation {
                phase: "idle",
                reason: Some("userActive"),
                idle_ms: Some(elapsed),
                motion: None,
            };
        }
        if let Some(until) = self.play_until {
            if now < until {
                if self.moving_direction.is_some() {
                    self.moving_direction = Some(roaming.direction);
                }
                return self.active_observation(elapsed, now);
            }
            self.play_until = None;
            self.moving_direction = None;
            self.next_play = Some(now.saturating_add(PLAY_GAP_MS));
        }
        if self.next_play.is_none_or(|at| now >= at) {
            let Some(until) = now.checked_add(PLAY_MS) else {
                self.reset();
                return Observation {
                    phase: "paused",
                    reason: Some("clockUnavailable"),
                    idle_ms: None,
                    motion: None,
                };
            };
            let Some(generation) = self.generation.checked_add(1) else {
                self.reset();
                return Observation {
                    phase: "paused",
                    reason: Some("motionUnavailable"),
                    idle_ms: None,
                    motion: None,
                };
            };
            self.play_until = Some(until);
            self.generation = generation;
            self.moving_direction = (roaming.available && self.next_burst_moves)
                .then_some(roaming.direction);
            if roaming.available {
                self.next_burst_moves = !self.next_burst_moves;
            }
            return self.active_observation(elapsed, now);
        }
        Observation {
            phase: "idle",
            reason: None,
            idle_ms: Some(elapsed),
            motion: None,
        }
    }
}

pub(super) struct PickerPermit(Arc<AtomicBool>);
impl Drop for PickerPermit {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

pub(super) struct Companion {
    state: DesktopCompanionState,
    policy: IdlePolicy,
    clock: Instant,
    revision: u64,
    app_sample_at: Option<u64>,
    active_app: Option<ActiveApp>,
    notification_sample_at: Option<u64>,
    notifications_allowed: bool,
    registry_path: PathBuf,
    registry_error: Option<String>,
    entries: Vec<Launcher>,
    picker_active: Arc<AtomicBool>,
}

fn path_key(path: &Path) -> Option<String> {
    Some(
        path.to_str()?
            .trim_start_matches(r"\\?\")
            .replace('/', "\\")
            .to_lowercase(),
    )
}
fn executable_path(path: &Path) -> bool {
    path.is_absolute()
        && path.to_str().is_some_and(|s| !s.contains('\0'))
        && path
            .extension()
            .and_then(|v| v.to_str())
            .is_some_and(|v| v.eq_ignore_ascii_case("exe"))
}
fn notifications_allow_idle_play(state: Option<i32>) -> bool {
    state == Some(5)
}
pub(super) fn settings_pause(settings: &Value) -> Option<&'static str> {
    if settings["reducedMotion"].as_bool().unwrap_or(false) {
        return Some("reducedMotion");
    }
    let parse = |key: &str| {
        settings[key]
            .as_str()
            .and_then(|value| chrono::NaiveTime::parse_from_str(value, "%H:%M").ok())
    };
    let reading = crate::budget_clock::now();
    let now = reading.utc
        .with_timezone(&chrono::FixedOffset::east_opt(8 * 3600).unwrap())
        .time();
    let scheduled = match (parse("dndStart"), parse("dndEnd")) {
        (Some(start), Some(end)) if start < end => now >= start && now < end,
        (Some(start), Some(end)) if start > end => now >= start || now < end,
        _ => false,
    };
    let temporary = &settings["temporaryDoNotDisturb"];
    // 投影不会逐帧重写；用单调截止时间判断到期，非法活动许可保持暂停。
    let temporary_active = temporary["active"].as_bool() == Some(true)
        && temporary["deadlineMonoMs"].as_u64().is_none_or(|deadline| reading.mono_ms < deadline);
    if settings["doNotDisturb"].as_bool().unwrap_or(false) || scheduled || temporary_active {
        Some("doNotDisturb")
    } else {
        None
    }
}
fn read_registry(path: &Path) -> Result<Vec<Launcher>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err("无法读取快捷启动登记文件。".into()),
    };
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_REGISTRY_BYTES
    {
        return Err("快捷启动登记文件无效；原文件已保留。".into());
    }
    let file = fs::File::open(path).map_err(|_| "无法读取快捷启动登记文件。")?;
    let opened = file.metadata().map_err(|_| "无法读取快捷启动登记文件。")?;
    if !opened.is_file() || opened.len() > MAX_REGISTRY_BYTES {
        return Err("快捷启动登记文件无效；原文件已保留。".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_REGISTRY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "无法读取快捷启动登记文件。")?;
    if bytes.len() as u64 > MAX_REGISTRY_BYTES {
        return Err("快捷启动登记文件过大；原文件已保留。".into());
    }
    let registry: Registry =
        serde_json::from_slice(&bytes).map_err(|_| "快捷启动登记格式无效；原文件已保留。")?;
    if registry.version != 1 || registry.entries.len() > MAX_LAUNCHERS {
        return Err("快捷启动登记版本或数量无效；原文件已保留。".into());
    }
    let mut ids = std::collections::HashSet::new();
    let mut paths = std::collections::HashSet::new();
    for entry in &registry.entries {
        if uuid::Uuid::parse_str(&entry.id).is_err()
            || !ids.insert(entry.id.clone())
            || entry.label.trim().is_empty()
            || entry.label.chars().count() > 80
            || !executable_path(&entry.executable)
            || !paths.insert(path_key(&entry.executable))
        {
            return Err("快捷启动登记内容无效；原文件已保留。".into());
        }
    }
    Ok(registry.entries)
}

impl Companion {
    pub(super) fn new(data_dir: &Path) -> Self {
        let registry_path = data_dir.join("companion-launchers.json");
        let (entries, registry_error) = match read_registry(&registry_path) {
            Ok(entries) => (entries, None),
            Err(error) => (Vec::new(), Some(error)),
        };
        let registry_ok = registry_error.is_none();
        Self {
            state: DesktopCompanionState {
                runtime_epoch: uuid::Uuid::new_v4().to_string(),
                revision: "0".into(),
                phase: "paused".into(),
                pause_reason: Some("inputUnavailable".into()),
                input_idle_ms: None,
                active_app: None,
                motion_generation: "0".into(),
                motion: None,
                capabilities: Capabilities {
                    system_idle: cfg!(windows),
                    roaming: false,
                    work_apps: cfg!(windows) && registry_ok,
                    launchers: cfg!(windows) && registry_ok,
                    file_drop: cfg!(windows) && registry_ok,
                    window_edges: false,
                },
            },
            policy: IdlePolicy::default(),
            clock: Instant::now(),
            revision: 0,
            app_sample_at: None,
            active_app: None,
            notification_sample_at: None,
            notifications_allowed: false,
            registry_path,
            registry_error,
            entries,
            picker_active: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(super) fn state(&self) -> DesktopCompanionState {
        self.state.clone()
    }

    /// Only a remaining lease for the current generation, never a fresh duration.
    /// The native owner must independently validate all movement gates.
    pub(super) fn movement(&self) -> Option<Movement> {
        if self.state.phase != "moving" || !self.state.capabilities.roaming ||
            self.picker_active.load(Ordering::Acquire) {
            return None;
        }
        let now = self.clock.elapsed().as_millis().min(u64::MAX as u128) as u64;
        self.policy.movement(now)
    }

    pub(super) fn interrupt(
        &mut self,
        reason: &'static str,
    ) -> Option<DesktopCompanionState> {
        self.policy.reset();
        self.notification_sample_at = None;
        self.notifications_allowed = false;
        self.app_sample_at = None;
        self.active_app = None;
        self.publish(
            Observation {
                phase: "paused",
                reason: Some(reason),
                idle_ms: None,
                motion: None,
            },
            self.state.capabilities.roaming,
        )
    }

    pub(super) fn tick(
        &mut self,
        settings: &Value,
        blocked: Option<&'static str>,
        roaming: RoamingContext,
    ) -> Option<DesktopCompanionState> {
        let now = self.clock.elapsed().as_millis().min(u64::MAX as u128) as u64;
        let blocked = if !cfg!(windows) {
            Some("unsupported")
        } else if self.picker_active.load(Ordering::Acquire) {
            Some("interaction")
        } else {
            blocked
        };
        if blocked.is_some() {
            self.notification_sample_at = None;
            self.notifications_allowed = false;
        } else if self
            .notification_sample_at
            .is_none_or(|at| now.saturating_sub(at) >= 1000)
        {
            self.notification_sample_at = Some(now);
            self.notifications_allowed =
                notifications_allow_idle_play(native::notification_state());
        }
        let idle_blocked =
            blocked.or_else(|| (!self.notifications_allowed).then_some("systemBusy"));
        let input = if idle_blocked.is_none() {
            native::last_input()
        } else {
            None
        };
        let observed = self.policy.observe(
            input,
            now,
            idle_blocked,
            settings["idlePlayEnabled"].as_bool().unwrap_or(true),
            RoamingContext {
                available: cfg!(windows)
                    && roaming.available
                    && settings["idleRoamingEnabled"].as_bool().unwrap_or(false),
                direction: roaming.direction,
            },
        );
        let link = settings["workAppLinkEnabled"].as_bool().unwrap_or(true)
            && self.registry_error.is_none()
            && self.entries.iter().any(|entry| entry.work_app_enabled);
        if blocked.is_some() || !link || settings["privacyMode"].as_bool().unwrap_or(false) {
            self.active_app = None;
            self.app_sample_at = None;
        } else if self
            .app_sample_at
            .is_none_or(|at| now.saturating_sub(at) >= 500)
        {
            self.app_sample_at = Some(now);
            self.active_app = native::foreground_executable().and_then(|path| {
                let key = path_key(&path)?;
                self.entries
                    .iter()
                    .find(|entry| {
                        entry.work_app_enabled && path_key(&entry.executable).as_ref() == Some(&key)
                    })
                    .map(|entry| ActiveApp {
                        id: entry.id.clone(),
                        label: entry.label.clone(),
                    })
            });
        }
        self.publish(observed, cfg!(windows) && roaming.available)
    }

    fn publish(
        &mut self,
        observed: Observation,
        roaming_available: bool,
    ) -> Option<DesktopCompanionState> {
        let old = self.state.clone();
        self.state.phase = observed.phase.into();
        self.state.pause_reason = observed.reason.map(str::to_owned);
        // Informational sample only; time passage alone does not emit an event.
        self.state.input_idle_ms = observed.idle_ms.map(|ms| ms / 1000 * 1000);
        self.state.motion_generation = self.policy.generation.to_string();
        self.state.motion = observed.motion;
        self.state.active_app = self.active_app.clone();
        self.state.capabilities.roaming = roaming_available;
        let changed = self.state.phase != old.phase
            || self.state.pause_reason != old.pause_reason
            || self.state.active_app != old.active_app
            || self.state.motion_generation != old.motion_generation
            || self.state.motion.as_ref().map(|motion| motion.direction)
                != old.motion.as_ref().map(|motion| motion.direction)
            || self.state.capabilities != old.capabilities;
        if changed {
            self.revision = self.revision.saturating_add(1);
            self.state.revision = self.revision.to_string();
            Some(self.state())
        } else {
            None
        }
    }

    fn registry_ready(&self) -> Result<(), String> {
        if !cfg!(windows) {
            return Err("快捷启动目前仅支持 Windows。".into());
        }
        if let Some(error) = &self.registry_error {
            return Err(error.clone());
        }
        Ok(())
    }
    pub(super) fn list(&self) -> Result<Vec<LauncherSummary>, String> {
        self.registry_ready()?;
        Ok(self.entries.iter().map(Launcher::summary).collect())
    }
    pub(super) fn begin_picker(&self) -> Result<PickerPermit, String> {
        self.registry_ready()?;
        self.picker_active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "应用选择窗口已打开。".to_owned())?;
        Ok(PickerPermit(self.picker_active.clone()))
    }
    fn persist(&self, entries: &[Launcher]) -> Result<(), String> {
        let temporary = self
            .registry_path
            .with_extension(format!("pending-{}.json", uuid::Uuid::new_v4()));
        let mut created = false;
        let result: Result<(), String> = (|| {
            let bytes = serde_json::to_vec_pretty(&Registry {
                version: 1,
                entries: entries.to_vec(),
            })
            .map_err(|e| e.to_string())?;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|_| "无法保存快捷启动登记。")?;
            created = true;
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .map_err(|_| "无法保存快捷启动登记。")?;
            drop(file);
            fs::rename(&temporary, &self.registry_path)
                .map_err(|_| "无法替换快捷启动登记；原登记仍保留。".to_owned())
        })();
        if result.is_err() && created {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
    pub(super) fn register(&mut self, selected: PathBuf) -> Result<LauncherSummary, String> {
        self.registry_ready()?;
        let executable = selected
            .canonicalize()
            .map_err(|_| "选中的应用已不存在或无法访问。")?;
        if !executable_path(&executable) || !executable.is_file() {
            return Err("请选择本机 .exe 应用。".into());
        }
        let key = path_key(&executable).ok_or("应用路径不是有效 Unicode。")?;
        if let Some(existing) = self
            .entries
            .iter()
            .find(|entry| path_key(&entry.executable).as_ref() == Some(&key))
        {
            return Ok(existing.summary());
        }
        if self.entries.len() >= MAX_LAUNCHERS {
            return Err("最多登记16个应用，请先移除不需要的项目。".into());
        }
        let label = executable
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("应用")
            .chars()
            .map(|character| if character.is_control() || matches!(character, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') { '\u{fffd}' } else { character })
            .take(80)
            .collect::<String>();
        let label = if label.trim().is_empty() { "应用".to_owned() } else { label.trim().to_owned() };
        let entry = Launcher {
            id: uuid::Uuid::new_v4().to_string(),
            label,
            executable,
            work_app_enabled: true,
        };
        let mut updated = self.entries.clone();
        updated.push(entry.clone());
        self.persist(&updated)?;
        self.entries = updated;
        self.app_sample_at = None;
        Ok(entry.summary())
    }
    pub(super) fn remove(&mut self, id: &str) -> Result<(), String> {
        self.registry_ready()?;
        if !self.entries.iter().any(|entry| entry.id == id) {
            return Err("启动项已不存在，请刷新列表。".into());
        }
        let updated: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| entry.id != id)
            .cloned()
            .collect();
        self.persist(&updated)?;
        self.entries = updated;
        self.app_sample_at = None;
        Ok(())
    }
    pub(super) fn update(&mut self, options: LauncherUpdateOptions) -> Result<LauncherSummary, String> {
        self.registry_ready()?;
        if options.label.is_none() && options.work_app_enabled.is_none() {
            return Err("请至少修改应用名称或工作陪伴设置。".into());
        }
        let index = self.entries.iter().position(|entry| entry.id == options.launcher_id)
            .ok_or("启动项已不存在，请刷新列表。")?;
        let label = options.label.as_deref().map(normalized_launcher_label).transpose()?;
        let mut updated = self.entries.clone();
        if let Some(label) = label { updated[index].label = label; }
        if let Some(enabled) = options.work_app_enabled { updated[index].work_app_enabled = enabled; }
        if updated[index].label == self.entries[index].label
            && updated[index].work_app_enabled == self.entries[index].work_app_enabled {
            return Ok(self.entries[index].summary());
        }
        // 保存完成后才发布内存数据；失败时保留旧列表和正在使用的采样。
        self.persist(&updated)?;
        self.entries = updated;
        self.app_sample_at = None;
        self.active_app = None;
        Ok(self.entries[index].summary())
    }
    pub(super) fn move_launcher(&mut self, options: LauncherMoveOptions) -> Result<(), String> {
        self.registry_ready()?;
        let index = self.entries.iter().position(|entry| entry.id == options.launcher_id)
            .ok_or("启动项已不存在，请刷新列表。")?;
        let adjacent = match options.direction {
            LauncherMoveDirection::Up => index.checked_sub(1),
            LauncherMoveDirection::Down => index.checked_add(1).filter(|next| *next < self.entries.len()),
        };
        let Some(adjacent) = adjacent else { return Ok(()); };
        // 调用方持有原生锁；只基于最新列表交换相邻项，不接收客户端完整列表。
        let mut updated = self.entries.clone();
        updated.swap(index, adjacent);
        self.persist(&updated)?;
        self.entries = updated;
        self.app_sample_at = None;
        self.active_app = None;
        Ok(())
    }
    pub(super) fn target(&self, id: &str) -> Result<PathBuf, String> {
        self.registry_ready()?;
        self.entries
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.executable.clone())
            .ok_or_else(|| "启动项已不存在，请刷新列表。".into())
    }
}

#[cfg(test)]
#[path = "launcher_management_tests.rs"]
mod launcher_management_tests;

pub(super) fn launch_selected(path: &Path) -> Result<(), String> {
    if !executable_path(path) || !path.is_file() {
        return Err("登记的应用已不存在，请重新选择。".into());
    }
    let mut command = Command::new(path);
    if let Some(parent) = path.parent() {
        command.current_dir(parent);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Normal OS/proxy/certificate settings remain; task/auth context must not leak to a user launcher.
    const PRIVATE_KEYS: &[&str] = &[
        "CODEX_HOME",
        "CODEX_ACCESS_TOKEN",
        "CODEX_API_KEY",
        "CODEX_APP_TOOLS_PIPE_PATH",
        "CODEX_INTERNAL_ORIGINATOR_OVERRIDE",
        "CODEX_THREAD_ID",
        "CODEX_SESSION_ID",
        "CODEX_TOOL_CALL_ID",
        "CODEX_TASK_WORKSPACE_VERIFYING_IDENTITY",
    ];
    for (key, _) in std::env::vars_os() {
        if PRIVATE_KEYS
            .iter()
            .any(|blocked| key.to_string_lossy().eq_ignore_ascii_case(blocked))
        {
            command.env_remove(key);
        }
    }
    // Explicit user action; no Job breakaway is requested. Ambient Job ownership still applies.
    command
        .spawn()
        .map(|child| drop(child))
        .map_err(|_| "未能启动登记的应用，请检查路径与系统权限。".into())
}

mod native {
    #[cfg(windows)]
    pub(super) fn notification_state() -> Option<i32> {
        use windows_sys::Win32::UI::Shell::SHQueryUserNotificationState;
        let mut state = 0;
        (unsafe { SHQueryUserNotificationState(&mut state) } == 0).then_some(state)
    }
    #[cfg(not(windows))]
    pub(super) fn notification_state() -> Option<i32> {
        None
    }

    #[cfg(windows)]
    pub(super) fn last_input() -> Option<u32> {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
        let mut info = LASTINPUTINFO {
            cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        (unsafe { GetLastInputInfo(&mut info) } != 0).then_some(info.dwTime)
    }
    #[cfg(not(windows))]
    pub(super) fn last_input() -> Option<u32> {
        None
    }

    #[cfg(windows)]
    pub(super) fn foreground_executable() -> Option<std::path::PathBuf> {
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            System::Threading::{
                OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
            },
            UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId},
        };
        unsafe {
            let window = GetForegroundWindow();
            if window.is_null() {
                return None;
            }
            let mut pid = 0;
            GetWindowThreadProcessId(window, &mut pid);
            if pid == 0 || pid == std::process::id() {
                return None;
            }
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                return None;
            }
            let mut image = vec![0u16; 32768];
            let mut size = image.len() as u32;
            let ok = QueryFullProcessImageNameW(process, 0, image.as_mut_ptr(), &mut size) != 0;
            CloseHandle(process);
            if !ok || size == 0 || size as usize > image.len() {
                return None;
            }
            String::from_utf16(&image[..size as usize])
                .ok()
                .map(std::path::PathBuf::from)
        }
    }
    #[cfg(not(windows))]
    pub(super) fn foreground_executable() -> Option<std::path::PathBuf> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NO_ROAMING: RoamingContext = RoamingContext {
        available: false,
        direction: MotionDirection::Right,
    };
    const ROAMING: RoamingContext = RoamingContext {
        available: true,
        direction: MotionDirection::Right,
    };

    #[cfg(windows)]
    fn stub_exe(root: &Path, name: &str) -> PathBuf {
        let path = root.join(name);
        fs::write(&path, b"test fixture only: registration, never launch").unwrap();
        path
    }

    #[cfg(windows)]
    #[test]
    fn launcher_registration_deduplicates_and_restart_keeps_opaque_summary() {
        let temp = tempfile::tempdir().unwrap();
        let executable = stub_exe(temp.path(), "example app.exe");
        let mut owner = Companion::new(temp.path());
        let first = owner.register(executable.clone()).unwrap();
        let registry_before = fs::read(&owner.registry_path).unwrap();
        let duplicate = owner.register(executable).unwrap();
        assert_eq!(first.id, duplicate.id);
        assert_eq!(owner.list().unwrap().len(), 1);
        assert_eq!(fs::read(&owner.registry_path).unwrap(), registry_before);
        let reopened = Companion::new(temp.path());
        let summaries = reopened.list().unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, first.id);
        let summary = serde_json::to_value(&summaries[0]).unwrap();
        let mut keys: Vec<_> = summary
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["id", "label", "workAppEnabled"]);
        assert_eq!(summary["label"], "example app");
        assert_eq!(summary["workAppEnabled"], true);
        assert_eq!(
            read_registry(&reopened.registry_path).unwrap()[0].id,
            first.id
        );
    }

    #[cfg(windows)]
    #[test]
    fn launcher_second_registration_remove_and_unknown_id_preserve_registry_contract() {
        let temp = tempfile::tempdir().unwrap();
        let first_exe = stub_exe(temp.path(), "first.exe");
        let second_exe = stub_exe(temp.path(), "second.exe");
        let mut owner = Companion::new(temp.path());
        let first = owner.register(first_exe).unwrap();
        let first_bytes = fs::read(&owner.registry_path).unwrap();
        let second = owner.register(second_exe).unwrap();
        assert_ne!(first.id, second.id);
        assert_ne!(fs::read(&owner.registry_path).unwrap(), first_bytes);
        assert_eq!(read_registry(&owner.registry_path).unwrap().len(), 2);
        owner.remove(&first.id).unwrap();
        let remaining = read_registry(&owner.registry_path).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, second.id);
        let saved = fs::read(&owner.registry_path).unwrap();
        assert!(owner.remove("unknown-launcher-id").is_err());
        assert!(owner.target("unknown-launcher-id").is_err());
        assert_eq!(fs::read(&owner.registry_path).unwrap(), saved);
        assert_eq!(Companion::new(temp.path()).list().unwrap()[0].id, second.id);
        assert!(fs::read_dir(temp.path()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("pending-")));
    }

    #[cfg(windows)]
    #[test]
    fn invalid_launcher_registry_is_retained_and_refuses_mutations() {
        let temp = tempfile::tempdir().unwrap();
        let executable = stub_exe(temp.path(), "valid.exe");
        let registry = temp.path().join("companion-launchers.json");
        for bytes in [
            b"{ invalid JSON".to_vec(),
            vec![b' '; MAX_REGISTRY_BYTES as usize + 1],
        ] {
            fs::write(&registry, &bytes).unwrap();
            assert!(read_registry(&registry).is_err());
            let mut owner = Companion::new(temp.path());
            assert!(!owner.state().capabilities.launchers);
            assert!(owner.list().is_err());
            assert!(owner.register(executable.clone()).is_err());
            assert!(owner.remove("unknown").is_err());
            assert_eq!(fs::read(&registry).unwrap(), bytes);
            assert!(fs::read_dir(temp.path()).unwrap().all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("pending-")));
        }
    }

    #[cfg(windows)]
    #[test]
    fn failed_launcher_replace_keeps_memory_and_cleans_only_created_pending_file() {
        let temp = tempfile::tempdir().unwrap();
        let executable = stub_exe(temp.path(), "valid.exe");
        let mut owner = Companion::new(temp.path());
        // Create a deterministic rename failure after startup without touching permissions.
        fs::create_dir(&owner.registry_path).unwrap();
        let marker = owner.registry_path.join("preserve.txt");
        fs::write(&marker, b"preserve existing destination").unwrap();
        assert!(owner.register(executable).is_err());
        assert!(owner.entries.is_empty());
        assert_eq!(fs::read(&marker).unwrap(), b"preserve existing destination");
        assert!(fs::read_dir(temp.path()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("pending-")));
    }

    #[test]
    fn system_notification_state_allows_only_accepts_and_failure_stays_quiet() {
        assert!(!notifications_allow_idle_play(None));
        for state in 0..=8 {
            assert_eq!(notifications_allow_idle_play(Some(state)), state == 5);
        }
        let mut policy = IdlePolicy::default();
        policy.observe(Some(3), 0, None, true, NO_ROAMING);
        assert_eq!(
            policy.observe(Some(3), 120_000, None, true, NO_ROAMING).phase,
            "playing"
        );
        assert_eq!(
            policy
                .observe(Some(3), 120_100, Some("systemBusy"), true, NO_ROAMING)
                .phase,
            "paused"
        );
        assert_eq!(
            policy.observe(Some(3), 999_000, None, true, NO_ROAMING).idle_ms,
            Some(0)
        );
    }

    #[test]
    fn observed_idle_requires_full_threshold_then_plays_finite_bursts() {
        let mut policy = IdlePolicy::default();
        assert_eq!(policy.observe(Some(7), 0, None, true, NO_ROAMING).idle_ms, Some(0));
        assert_eq!(policy.observe(Some(7), 119_999, None, true, NO_ROAMING).phase, "idle");
        assert_eq!(
            policy.observe(Some(7), 120_000, None, true, NO_ROAMING).phase,
            "playing"
        );
        assert_eq!(
            policy.observe(Some(7), 122_399, None, true, NO_ROAMING).phase,
            "playing"
        );
        assert_eq!(policy.observe(Some(7), 122_400, None, true, NO_ROAMING).phase, "idle");
        assert_eq!(policy.observe(Some(7), 140_399, None, true, NO_ROAMING).phase, "idle");
        assert_eq!(
            policy.observe(Some(7), 140_400, None, true, NO_ROAMING).phase,
            "playing"
        );
    }

    #[test]
    fn changed_input_including_backward_tick_cancels_play_without_backlog() {
        let mut policy = IdlePolicy::default();
        policy.observe(Some(u32::MAX), 0, None, true, NO_ROAMING);
        assert_eq!(
            policy.observe(Some(u32::MAX), 120_000, None, true, NO_ROAMING).phase,
            "playing"
        );
        let generation = policy.generation;
        let fresh = policy.observe(Some(1), 120_010, None, true, NO_ROAMING);
        assert_eq!((fresh.phase, fresh.idle_ms), ("idle", Some(0)));
        assert!(policy.generation > generation);
        assert_eq!(
            policy.observe(Some(0), 240_009, None, true, NO_ROAMING).idle_ms,
            Some(0)
        );
        assert_eq!(policy.observe(Some(0), 360_008, None, true, NO_ROAMING).phase, "idle");
        assert_eq!(
            policy.observe(Some(0), 360_009, None, true, NO_ROAMING).phase,
            "playing"
        );
    }

    #[test]
    fn suppression_and_input_failure_require_fresh_observation_after_resume() {
        for reason in [
            "hidden",
            "interaction",
            "desktopPaused",
            "doNotDisturb",
            "reducedMotion",
        ] {
            let mut policy = IdlePolicy::default();
            policy.observe(Some(4), 0, None, true, NO_ROAMING);
            policy.observe(Some(4), 120_000, None, true, NO_ROAMING);
            assert_eq!(
                policy.observe(Some(4), 120_100, Some(reason), true, NO_ROAMING).phase,
                "paused"
            );
            assert_eq!(
                policy.observe(Some(4), 999_000, None, true, NO_ROAMING).idle_ms,
                Some(0)
            );
            assert_eq!(
                policy.observe(None, 1_000_000, None, true, NO_ROAMING).reason,
                Some("inputUnavailable")
            );
            assert_eq!(
                policy.observe(Some(4), 2_000_000, None, true, NO_ROAMING).idle_ms,
                Some(0)
            );
        }
    }

    #[test]
    fn disabled_idle_play_cancels_burst_and_clock_reset_never_creates_old_play() {
        let mut policy = IdlePolicy::default();
        policy.observe(Some(2), 0, None, true, NO_ROAMING);
        policy.observe(Some(2), 120_000, None, true, NO_ROAMING);
        assert_eq!(
            policy.observe(Some(2), 120_100, None, false, NO_ROAMING).reason,
            Some("disabled")
        );
        assert!(policy.play_until.is_none());
        assert_eq!(policy.observe(Some(2), 10, None, true, NO_ROAMING).idle_ms, Some(0));
        assert_eq!(policy.observe(Some(2), 120_009, None, true, NO_ROAMING).phase, "idle");
    }

    #[test]
    fn available_gait_alternates_moving_and_playing_without_changing_idle_cadence() {
        let mut policy = IdlePolicy::default();
        policy.observe(Some(7), 0, None, true, ROAMING);
        assert_eq!(policy.observe(Some(7), 119_999, None, true, ROAMING).phase, "idle");
        let first = policy.observe(Some(7), 120_000, None, true, ROAMING);
        assert_eq!(first.phase, "moving");
        assert_eq!(first.motion.unwrap().remaining_ms, PLAY_MS);
        assert_eq!(policy.generation, 1);
        assert_eq!(policy.observe(Some(7), 122_399, None, true, ROAMING).phase, "moving");
        assert_eq!(policy.observe(Some(7), 122_400, None, true, ROAMING).phase, "idle");
        assert!(policy.movement(122_400).is_none());
        assert_eq!(policy.observe(Some(7), 140_399, None, true, ROAMING).phase, "idle");
        let second = policy.observe(Some(7), 140_400, None, true, ROAMING);
        assert_eq!(second.phase, "playing");
        assert!(second.motion.is_none());
        assert_eq!(policy.generation, 2);
        assert_eq!(policy.observe(Some(7), 142_800, None, true, ROAMING).phase, "idle");
        let third = policy.observe(Some(7), 160_800, None, true, RoamingContext {
            direction: MotionDirection::Left,
            ..ROAMING
        });
        assert_eq!(third.phase, "moving");
        assert_eq!(third.motion.unwrap().direction, MotionDirection::Left);
        assert_eq!(policy.generation, 3);
    }

    #[test]
    fn unavailable_gait_falls_back_and_loss_cancels_a_current_movement() {
        let mut policy = IdlePolicy::default();
        policy.observe(Some(9), 0, None, true, NO_ROAMING);
        let fallback = policy.observe(Some(9), 120_000, None, true, NO_ROAMING);
        assert_eq!(fallback.phase, "playing");
        assert!(fallback.motion.is_none());
        assert!(policy.movement(120_100).is_none());
        policy.observe(Some(9), 122_400, None, true, ROAMING);
        assert_eq!(policy.observe(Some(9), 140_400, None, true, ROAMING).phase, "moving");
        let generation = policy.generation;
        let unavailable = policy.observe(Some(9), 140_450, None, true, NO_ROAMING);
        assert_eq!((unavailable.phase, unavailable.idle_ms), ("idle", Some(0)));
        assert!(unavailable.motion.is_none());
        assert!(policy.generation > generation);
        assert!(policy.movement(140_450).is_none());
        assert_eq!(policy.observe(Some(9), 260_449, None, true, ROAMING).phase, "idle");
        assert_eq!(policy.observe(Some(9), 260_450, None, true, ROAMING).phase, "moving");
    }

    #[test]
    fn informational_revisions_and_direction_changes_do_not_renew_a_movement() {
        let temp = tempfile::tempdir().unwrap();
        let mut owner = Companion::new(temp.path());
        let observed = owner.policy.observe(Some(3), 0, None, true, ROAMING);
        owner.publish(observed, true);
        let observed = owner.policy.observe(Some(3), 120_000, None, true, ROAMING);
        let started = owner.publish(observed, true).unwrap();
        let generation = started.motion_generation.clone();
        let json = serde_json::to_value(&started).unwrap();
        assert_eq!(json["motion"], serde_json::json!({"direction": "right", "remainingMs": 2400}));
        assert_eq!(json["capabilities"]["roaming"], true);
        let observed = owner.policy.observe(Some(3), 120_500, None, true, ROAMING);
        assert!(owner.publish(observed, true).is_none());
        assert_eq!(owner.state.revision, started.revision);
        assert_eq!(owner.state.motion.as_ref().unwrap().remaining_ms, 1900);
        owner.active_app = Some(ActiveApp { id: "selected-app".into(), label: "Editor".into() });
        let observed = owner.policy.observe(Some(3), 120_550, None, true, ROAMING);
        let informational = owner.publish(observed, true).unwrap();
        assert_ne!(informational.revision, started.revision);
        assert_eq!(informational.motion_generation, generation);
        assert_eq!(informational.motion.unwrap().remaining_ms, 1850);
        let observed = owner.policy.observe(Some(3), 120_600, None, true, RoamingContext {
            direction: MotionDirection::Left,
            ..ROAMING
        });
        let reversed = owner.publish(observed, true).unwrap();
        let motion = reversed.motion.unwrap();
        assert_eq!(motion.direction, MotionDirection::Left);
        assert_eq!(motion.remaining_ms, 1800);
        assert_eq!(reversed.motion_generation, generation);
        assert_eq!(owner.policy.play_until, Some(122_400));
        assert_eq!(owner.policy.movement(122_399).unwrap().remaining, Duration::from_millis(1));
        assert!(owner.policy.movement(122_400).is_none());
        let observed = owner.policy.observe(Some(3), 122_400, None, true, ROAMING);
        let ended = owner.publish(observed, true).unwrap();
        assert_eq!(ended.phase, "idle");
        assert!(serde_json::to_value(&ended).unwrap()["motion"].is_null());
        let observed = owner.policy.observe(Some(3), 122_450, None, true, NO_ROAMING);
        let unavailable = owner.publish(observed, false).unwrap();
        assert!(!unavailable.capabilities.roaming);
        assert!(unavailable.motion.is_none());
    }

    #[test]
    fn input_missing_input_block_and_clock_rollback_cancel_movement_before_resume() {
        for (input, now, blocked) in [
            (Some(2), 120_100, None),
            (None, 120_100, None),
            (Some(1), 120_100, Some("workActive")),
            (Some(1), 119_900, None),
        ] {
            let mut policy = IdlePolicy::default();
            policy.observe(Some(1), 0, None, true, ROAMING);
            policy.observe(Some(1), 120_000, None, true, ROAMING);
            let generation = policy.generation;
            let cancelled = policy.observe(input, now, blocked, true, ROAMING);
            assert_ne!(cancelled.phase, "moving");
            assert!(cancelled.motion.is_none());
            assert!(policy.movement(now).is_none());
            assert!(policy.generation > generation);
            let resumed_input = input.unwrap_or(1);
            let fresh_at = if cancelled.phase == "idle" {
                assert_eq!(cancelled.idle_ms, Some(0));
                now
            } else {
                assert_eq!(policy.observe(Some(resumed_input), now + 1, None, true, ROAMING).idle_ms, Some(0));
                now + 1
            };
            assert_eq!(policy.observe(Some(resumed_input), fresh_at + IDLE_MS - 1, None, true, ROAMING).phase, "idle");
            assert_eq!(policy.observe(Some(resumed_input), fresh_at + IDLE_MS, None, true, ROAMING).phase, "moving");
        }
    }

    #[test]
    fn explicit_interrupt_publishes_paused_once_and_requires_a_fresh_idle_threshold() {
        let temp = tempfile::tempdir().unwrap();
        let mut owner = Companion::new(temp.path());
        owner.policy.observe(Some(1), 0, None, true, ROAMING);
        let observed = owner.policy.observe(Some(1), 120_000, None, true, ROAMING);
        let started = owner.publish(observed, true).unwrap();
        let interrupted = owner.interrupt("geometryUnavailable").unwrap();
        assert_eq!(interrupted.phase, "paused");
        assert_eq!(interrupted.pause_reason.as_deref(), Some("geometryUnavailable"));
        assert!(interrupted.input_idle_ms.is_none());
        assert!(interrupted.motion.is_none());
        assert_ne!(interrupted.motion_generation, started.motion_generation);
        assert!(owner.movement().is_none());
        assert!(owner.interrupt("geometryUnavailable").is_none());
        assert_eq!(owner.policy.observe(Some(1), 500_000, None, true, ROAMING).idle_ms, Some(0));
        assert_eq!(owner.policy.observe(Some(1), 619_999, None, true, ROAMING).phase, "idle");
        assert_eq!(owner.policy.observe(Some(1), 620_000, None, true, ROAMING).phase, "moving");
    }

    #[test]
    fn scheduler_pause_discards_expired_motion_instead_of_replaying_bursts() {
        let mut policy = IdlePolicy::default();
        policy.observe(Some(1), 0, None, true, ROAMING);
        policy.observe(Some(1), 120_000, None, true, ROAMING);
        let generation = policy.generation;
        assert_eq!(policy.observe(Some(1), 999_000, None, true, ROAMING).phase, "idle");
        assert!(policy.movement(999_000).is_none());
        assert_eq!(policy.generation, generation);
        assert_eq!(policy.observe(Some(1), 1_016_999, None, true, ROAMING).phase, "idle");
        let next = policy.observe(Some(1), 1_017_000, None, true, ROAMING);
        assert_eq!(next.phase, "playing");
        assert!(next.motion.is_none());
        assert!(policy.generation > generation);
    }

    #[cfg(windows)]
    #[test]
    fn picker_blocks_current_movement_before_any_policy_tick() {
        let temp = tempfile::tempdir().unwrap();
        let mut owner = Companion::new(temp.path());
        owner.policy.last_input = Some(1);
        owner.policy.changed_at = Some(0);
        owner.policy.observed_at = Some(0);
        owner.policy.play_until = Some(PLAY_MS);
        owner.policy.moving_direction = Some(MotionDirection::Right);
        owner.policy.generation = 1;
        owner.state.phase = "moving".into();
        owner.state.capabilities.roaming = true;
        assert_eq!(owner.movement().unwrap().input_marker, 1);
        let permit = owner.begin_picker().unwrap();
        assert!(owner.movement().is_none());
        assert_eq!(owner.policy.play_until, Some(PLAY_MS));
        assert_eq!(owner.state.phase, "moving");
        drop(permit);
    }
}
