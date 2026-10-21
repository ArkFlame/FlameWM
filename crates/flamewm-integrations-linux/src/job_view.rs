//! KDE KIO JobViewServer adapter. Progress is keyed by the caller's unique bus owner and
//! server-issued object path; no progress is inferred from notification hints.

use std::collections::BTreeMap;
use std::ffi::CString;
use std::time::Duration;

use dbus::arg::messageitem::MessageItem;
use dbus::strings::ErrorName;
use dbus::{Message, MessageType};
use flamewm_api::panels::{PanelsSnapshot, TaskProgress, TaskProgressEntry};
use flamewm_api::{DesktopAppId, ErrorCode, FlameError, FlameResult};
use flamewm_dbus_reactor::{BusKind, BusPump, BusWatch, MessageDisposition};

pub const JOB_VIEW_SERVICE: &str = "org.kde.JobViewServer";
pub const KUISERVER_SERVICE: &str = "org.kde.kuiserver";
pub const JOB_VIEW_PATH: &str = "/JobViewServer";
pub const JOB_VIEW_SERVER_INTERFACE: &str = "org.kde.JobViewServer";
pub const JOB_VIEW_SERVER_V2_INTERFACE: &str = "org.kde.JobViewServerV2";
pub const JOB_VIEW_V2_INTERFACE: &str = "org.kde.JobViewV2";
pub const JOB_VIEW_V3_INTERFACE: &str = "org.kde.JobViewV3";

const DBUS_SERVICE: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";
const DBUS_INTERFACE: &str = "org.freedesktop.DBus";
const NAME_OWNER_CHANGED: &str = "NameOwnerChanged";
const MAX_JOBS: usize = 256;
const MAX_DESKTOP_ENTRY_BYTES: usize = 255;
const JOB_PATH_PREFIX: &str = "/org/kde/notificationmanager/jobs/JobView_";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobViewProviderState {
    Disconnected,
    Unavailable,
    Started,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct JobProgress {
    percent: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct JobView {
    owner: String,
    app_id: Option<DesktopAppId>,
    progress: JobProgress,
    suspended: bool,
}

/// Event-driven implementation of KDE's JobView server side. It declines ownership when another
/// desktop already provides either well-known name, and never replaces or queues behind it.
pub struct KioJobViewProvider {
    pump: BusPump,
    jobs: BTreeMap<String, JobView>,
    next_job_id: u64,
    state: JobViewProviderState,
    changed: bool,
}

impl KioJobViewProvider {
    pub fn connect() -> FlameResult<Self> {
        Ok(Self {
            pump: BusPump::connect(BusKind::Session)?,
            jobs: BTreeMap::new(),
            next_job_id: 1,
            state: JobViewProviderState::Disconnected,
            changed: false,
        })
    }

    /// Returns `false` when another desktop owns either KDE job-view name.
    pub fn start(&mut self) -> FlameResult<bool> {
        if self.state == JobViewProviderState::Started {
            return Ok(true);
        }
        self.add_owner_match()?;
        if !request_name(&self.pump, JOB_VIEW_SERVICE)? {
            self.state = JobViewProviderState::Unavailable;
            return Ok(false);
        }
        if !request_name(&self.pump, KUISERVER_SERVICE)? {
            release_name(&self.pump, JOB_VIEW_SERVICE)?;
            self.state = JobViewProviderState::Unavailable;
            return Ok(false);
        }
        self.state = JobViewProviderState::Started;
        Ok(true)
    }

    pub fn stop(&mut self) -> FlameResult<()> {
        let owned = self.state == JobViewProviderState::Started;
        self.state = JobViewProviderState::Disconnected;
        self.changed |= !self.jobs.is_empty();
        self.jobs.clear();
        if !owned {
            return Ok(());
        }
        let first = release_name(&self.pump, KUISERVER_SERVICE);
        let second = release_name(&self.pump, JOB_VIEW_SERVICE);
        first.and(second)
    }

    #[must_use]
    pub const fn state(&self) -> JobViewProviderState {
        self.state
    }

    #[must_use]
    pub fn watch(&self) -> BusWatch {
        self.pump.watch()
    }

    /// Processes one bounded reactor turn. Job updates are accepted only from the unique bus
    /// connection that requested the corresponding object path.
    pub fn on_ready(&mut self) -> FlameResult<usize> {
        let mut changed = false;
        let started = self.state == JobViewProviderState::Started;
        let pump = &self.pump;
        let jobs = &mut self.jobs;
        let next_job_id = &mut self.next_job_id;
        let count = pump.on_ready(|message| {
            handle_message(message, started, jobs, next_job_id, &mut changed)
        })?;
        self.changed |= changed;
        Ok(count)
    }

    /// Maps active jobs onto task entries only when desktop identity selects exactly one entry.
    /// Jobs without a valid percentage keep that app's aggregate indeterminate and hidden.
    #[must_use]
    pub fn task_progress(&self, panels: &PanelsSnapshot) -> Vec<TaskProgressEntry> {
        task_progress_for_jobs(&self.jobs, panels)
    }

    #[must_use]
    pub fn active_job_count(&self) -> usize {
        self.jobs.len()
    }

    #[must_use]
    pub fn job_is_suspended(&self, path: &str) -> Option<bool> {
        self.jobs.get(path).map(|job| job.suspended)
    }

    /// Whether the most recent reactor turn changed visible task progress.
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    fn add_owner_match(&self) -> FlameResult<()> {
        let rule = "type='signal',sender='org.freedesktop.DBus',path='/org/freedesktop/DBus',interface='org.freedesktop.DBus',member='NameOwnerChanged'";
        let call = Message::new_method_call(DBUS_SERVICE, DBUS_PATH, DBUS_INTERFACE, "AddMatch")
            .map_err(|error| FlameError::new(ErrorCode::InvalidArgument, error))?
            .append1(rule.to_owned());
        let reply = self.pump.send_with_reply(call, Duration::from_secs(2))?;
        if reply.msg_type() == MessageType::Error {
            return Err(FlameError::unavailable("cannot watch D-Bus owner changes"));
        }
        Ok(())
    }
}

fn task_progress_for_jobs(
    jobs: &BTreeMap<String, JobView>,
    panels: &PanelsSnapshot,
) -> Vec<TaskProgressEntry> {
    let mut grouped: BTreeMap<DesktopAppId, (u64, u64, bool)> = BTreeMap::new();
    for job in jobs.values() {
        let Some(app_id) = &job.app_id else {
            continue;
        };
        let aggregate = grouped.entry(app_id.clone()).or_default();
        match job.progress.percent {
            Some(percent) => {
                aggregate.0 += u64::from(percent);
                aggregate.1 += 1;
            }
            None => aggregate.2 = true,
        }
    }

    grouped
        .into_iter()
        .filter_map(|(app_id, (sum, count, indeterminate))| {
            if count == 0 || indeterminate {
                return None;
            }
            let mut matching = panels.tasks.iter().filter(|task| task.app_id == app_id);
            let task = matching.next()?;
            if matching.next().is_some() {
                return None;
            }
            Some(TaskProgressEntry {
                task_id: task.id.clone(),
                progress: TaskProgress::new((sum / count) as u8)?,
            })
        })
        .collect()
}

fn handle_message(
    message: &Message,
    started: bool,
    jobs: &mut BTreeMap<String, JobView>,
    next_job_id: &mut u64,
    changed: &mut bool,
) -> MessageDisposition {
    if message.msg_type() == MessageType::Signal
        && message.interface().is_some_and(|v| v == DBUS_INTERFACE)
        && message.member().is_some_and(|v| v == NAME_OWNER_CHANGED)
        && message.path().is_some_and(|v| v == DBUS_PATH)
    {
        if let [
            MessageItem::Str(name),
            MessageItem::Str(old_owner),
            MessageItem::Str(new_owner),
        ] = message.get_items().as_slice()
        {
            if new_owner.is_empty() {
                let before = jobs.len();
                jobs.retain(|_, job| job.owner != *old_owner);
                *changed |= jobs.len() != before;
            }
            if name == JOB_VIEW_SERVICE || name == KUISERVER_SERVICE {
                if new_owner.is_empty() {
                    let before = jobs.len();
                    jobs.clear();
                    *changed |= before != 0;
                }
            }
        }
        return MessageDisposition::Handled;
    }
    if message.msg_type() != MessageType::MethodCall {
        return MessageDisposition::Unhandled;
    }

    let path = message
        .path()
        .map(|value| value.to_string())
        .unwrap_or_default();
    let interface = message
        .interface()
        .map(|value| value.to_string())
        .unwrap_or_default();
    let member = message
        .member()
        .map(|value| value.to_string())
        .unwrap_or_default();

    if path == JOB_VIEW_PATH
        && (interface == JOB_VIEW_SERVER_INTERFACE || interface == JOB_VIEW_SERVER_V2_INTERFACE)
        && member == "requestView"
    {
        if message.get_no_reply() {
            return MessageDisposition::Handled;
        }
        if !started {
            return error_reply(
                message,
                "org.freedesktop.DBus.Error.ServiceUnknown",
                "KIO JobView is unavailable",
            );
        }
        return request_view(message, &interface, jobs, next_job_id, changed);
    }

    if path.starts_with(JOB_PATH_PREFIX) {
        return update_job(message, &path, &interface, &member, jobs, changed);
    }
    MessageDisposition::Unhandled
}

fn request_view(
    message: &Message,
    interface: &str,
    jobs: &mut BTreeMap<String, JobView>,
    next_job_id: &mut u64,
    changed: &mut bool,
) -> MessageDisposition {
    if jobs.len() >= MAX_JOBS {
        return error_reply(
            message,
            "org.freedesktop.DBus.Error.LimitsExceeded",
            "too many active KIO jobs",
        );
    }
    let Some(owner) = message.sender().map(|value| value.to_string()) else {
        return error_reply(
            message,
            "org.freedesktop.DBus.Error.AccessDenied",
            "KIO job has no sender identity",
        );
    };
    if !owner.starts_with(':') {
        return error_reply(
            message,
            "org.freedesktop.DBus.Error.AccessDenied",
            "KIO job sender is not a unique bus owner",
        );
    }
    let items = message.get_items();
    let app_id = match request_app_id(interface, &items) {
        Ok(app_id) => app_id,
        Err(()) => {
            return error_reply(
                message,
                "org.freedesktop.DBus.Error.InvalidArgs",
                "invalid KIO requestView arguments",
            );
        }
    };
    let Some(id) = allocate_job_id(jobs, next_job_id) else {
        return error_reply(
            message,
            "org.freedesktop.DBus.Error.LimitsExceeded",
            "KIO job identifiers exhausted",
        );
    };
    let path = format!("{JOB_PATH_PREFIX}{id}");
    jobs.insert(
        path.clone(),
        JobView {
            owner,
            app_id,
            progress: JobProgress { percent: None },
            suspended: false,
        },
    );
    *changed = true;
    let Ok(path) = dbus::Path::new(path) else {
        jobs.remove(&format!(
            "{JOB_PATH_PREFIX}{}",
            next_job_id.saturating_sub(1)
        ));
        return error_reply(
            message,
            "org.freedesktop.DBus.Error.Failed",
            "could not allocate KIO job path",
        );
    };
    MessageDisposition::Reply(message.method_return().append1(path))
}

fn request_app_id(interface: &str, items: &[MessageItem]) -> Result<Option<DesktopAppId>, ()> {
    match (interface, items) {
        (
            JOB_VIEW_SERVER_V2_INTERFACE,
            [
                MessageItem::Str(entry),
                MessageItem::Int32(_),
                MessageItem::Dict(_),
            ],
        ) => Ok(desktop_app_id(entry)),
        (
            JOB_VIEW_SERVER_INTERFACE,
            [
                MessageItem::Str(_),
                MessageItem::Str(_),
                MessageItem::Int32(_),
            ],
        ) => Ok(None),
        _ => Err(()),
    }
}

fn update_job(
    message: &Message,
    path: &str,
    interface: &str,
    member: &str,
    jobs: &mut BTreeMap<String, JobView>,
    changed: &mut bool,
) -> MessageDisposition {
    let Some(sender) = message.sender().map(|value| value.to_string()) else {
        return error_reply(
            message,
            "org.freedesktop.DBus.Error.AccessDenied",
            "KIO update has no sender identity",
        );
    };
    let Some(job) = jobs.get(path) else {
        return error_reply(
            message,
            "org.freedesktop.DBus.Error.UnknownObject",
            "KIO job no longer exists",
        );
    };
    if job.owner != sender {
        return error_reply(
            message,
            "org.freedesktop.DBus.Error.AccessDenied",
            "KIO job belongs to another bus owner",
        );
    }

    let items = message.get_items();
    let outcome = {
        let Some(job) = jobs.get_mut(path) else {
            return error_reply(
                message,
                "org.freedesktop.DBus.Error.UnknownObject",
                "KIO job no longer exists",
            );
        };
        apply_job_update(job, interface, member, &items)
    };
    match outcome {
        Ok(JobUpdate::Terminated) => {
            jobs.remove(path);
            *changed = true;
            MessageDisposition::Handled
        }
        Ok(JobUpdate::Changed) => {
            *changed = true;
            MessageDisposition::Handled
        }
        Ok(JobUpdate::Unchanged) => MessageDisposition::Handled,
        Ok(JobUpdate::DescriptionFieldResult) => {
            if message.get_no_reply() {
                MessageDisposition::Handled
            } else {
                MessageDisposition::Reply(message.method_return().append1(false))
            }
        }
        Err(JobUpdateError::Malformed) => error_reply(
            message,
            "org.freedesktop.DBus.Error.InvalidArgs",
            "invalid KIO JobView arguments",
        ),
        Err(JobUpdateError::UnknownMethod) => error_reply(
            message,
            "org.freedesktop.DBus.Error.UnknownMethod",
            "unsupported KIO JobView method",
        ),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobUpdate {
    Unchanged,
    Changed,
    Terminated,
    DescriptionFieldResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobUpdateError {
    Malformed,
    UnknownMethod,
}

fn apply_job_update(
    job: &mut JobView,
    interface: &str,
    member: &str,
    items: &[MessageItem],
) -> Result<JobUpdate, JobUpdateError> {
    let malformed = || JobUpdateError::Malformed;
    match (interface, member) {
        (JOB_VIEW_V3_INTERFACE, "terminate") => match items {
            [
                MessageItem::UInt32(_),
                MessageItem::Str(_),
                MessageItem::Dict(_),
            ] => Ok(JobUpdate::Terminated),
            _ => Err(malformed()),
        },
        (JOB_VIEW_V2_INTERFACE, "terminate") => match items {
            [MessageItem::Str(_)] => Ok(JobUpdate::Terminated),
            _ => Err(malformed()),
        },
        (JOB_VIEW_V3_INTERFACE, "update") => {
            let Ok((percent, suspended)) = parse_v3_update(items) else {
                let changed = job.progress.percent.take().is_some();
                return if changed {
                    Ok(JobUpdate::Changed)
                } else {
                    Err(malformed())
                };
            };
            let mut changed = false;
            if let Some(percent) = percent {
                changed |= job.progress.percent != percent;
                job.progress.percent = percent;
            }
            if let Some(suspended) = suspended {
                changed |= job.suspended != suspended;
                job.suspended = suspended;
            }
            Ok(if changed {
                JobUpdate::Changed
            } else {
                JobUpdate::Unchanged
            })
        }
        (JOB_VIEW_V2_INTERFACE, "setPercent") => match items {
            [MessageItem::UInt32(value)] => {
                let percent = (*value <= 100).then_some(*value as u8);
                let changed = job.progress.percent != percent;
                job.progress.percent = percent;
                Ok(if changed {
                    JobUpdate::Changed
                } else {
                    JobUpdate::Unchanged
                })
            }
            _ => Err(malformed()),
        },
        (JOB_VIEW_V2_INTERFACE, "setSuspended") => match items {
            [MessageItem::Bool(suspended)] => {
                let changed = job.suspended != *suspended;
                job.suspended = *suspended;
                Ok(if changed {
                    JobUpdate::Changed
                } else {
                    JobUpdate::Unchanged
                })
            }
            _ => Err(malformed()),
        },
        (JOB_VIEW_V2_INTERFACE, "setTotalAmount" | "setProcessedAmount") => match items {
            [MessageItem::UInt64(_), MessageItem::Str(_)] => Ok(JobUpdate::Unchanged),
            _ => Err(malformed()),
        },
        (JOB_VIEW_V2_INTERFACE, "setSpeed") => match items {
            [MessageItem::UInt64(_)] => Ok(JobUpdate::Unchanged),
            _ => Err(malformed()),
        },
        (JOB_VIEW_V2_INTERFACE, "setInfoMessage") => match items {
            [MessageItem::Str(_)] => Ok(JobUpdate::Unchanged),
            _ => Err(malformed()),
        },
        (JOB_VIEW_V2_INTERFACE, "setDescriptionField") => match items {
            [
                MessageItem::UInt32(_),
                MessageItem::Str(_),
                MessageItem::Str(_),
            ] => Ok(JobUpdate::DescriptionFieldResult),
            _ => Err(malformed()),
        },
        (JOB_VIEW_V2_INTERFACE, "clearDescriptionField" | "setError") => match items {
            [MessageItem::UInt32(_)] => Ok(JobUpdate::Unchanged),
            _ => Err(malformed()),
        },
        (JOB_VIEW_V2_INTERFACE, "setDestUrl") => match items {
            [MessageItem::Variant(_)] => Ok(JobUpdate::Unchanged),
            _ => Err(malformed()),
        },
        _ => Err(JobUpdateError::UnknownMethod),
    }
}

fn parse_v3_update(items: &[MessageItem]) -> Result<(Option<Option<u8>>, Option<bool>), ()> {
    let [MessageItem::Dict(properties)] = items else {
        return Err(());
    };
    let mut percent = None;
    let mut suspended = None;
    for (key, value) in properties.clone().into_vec() {
        let MessageItem::Str(key) = key else {
            return Err(());
        };
        match key.as_str() {
            "percent" => match unwrap_variant(value) {
                MessageItem::UInt32(value) if value <= 100 => percent = Some(Some(value as u8)),
                MessageItem::UInt32(_) => percent = Some(None),
                _ => return Err(()),
            },
            "suspended" => match unwrap_variant(value) {
                MessageItem::Bool(value) => suspended = Some(value),
                _ => return Err(()),
            },
            _ => {}
        }
    }
    Ok((percent, suspended))
}

fn desktop_app_id(value: &str) -> Option<DesktopAppId> {
    (!value.is_empty()
        && value.len() <= MAX_DESKTOP_ENTRY_BYTES
        && !value.starts_with('.')
        && !value.ends_with(".desktop")
        && value
            .chars()
            .all(|c| !c.is_control() && c != '/' && c != '\\'))
    .then(|| DesktopAppId::new(value))
}

fn allocate_job_id(jobs: &BTreeMap<String, JobView>, next: &mut u64) -> Option<u64> {
    for _ in 0..=jobs.len() {
        let id = *next;
        *next = next.checked_add(1)?;
        if !jobs.contains_key(&format!("{JOB_PATH_PREFIX}{id}")) {
            return Some(id);
        }
    }
    None
}

fn unwrap_variant(item: MessageItem) -> MessageItem {
    match item {
        MessageItem::Variant(value) => unwrap_variant(*value),
        item => item,
    }
}

fn error_reply(message: &Message, name: &str, text: &str) -> MessageDisposition {
    if message.get_no_reply() {
        return MessageDisposition::Handled;
    }
    let Ok(name) = ErrorName::new(name) else {
        return MessageDisposition::Handled;
    };
    let Ok(text) = CString::new(text) else {
        return MessageDisposition::Handled;
    };
    MessageDisposition::Reply(message.error(&name, &text))
}

fn request_name(pump: &BusPump, name: &str) -> FlameResult<bool> {
    let call = Message::new_method_call(DBUS_SERVICE, DBUS_PATH, DBUS_INTERFACE, "RequestName")
        .map_err(|error| FlameError::new(ErrorCode::InvalidArgument, error))?
        .append2(name.to_owned(), 0x4_u32);
    let reply = pump.send_with_reply(call, Duration::from_secs(2))?;
    if reply.msg_type() == MessageType::Error {
        return Err(FlameError::unavailable("D-Bus name request failed"));
    }
    match reply.read1::<u32>() {
        Ok(1 | 4) => Ok(true),
        Ok(_) => Ok(false),
        Err(error) => Err(FlameError::invalid(format!(
            "D-Bus name request reply is invalid: {error}"
        ))),
    }
}

fn release_name(pump: &BusPump, name: &str) -> FlameResult<()> {
    let call = Message::new_method_call(DBUS_SERVICE, DBUS_PATH, DBUS_INTERFACE, "ReleaseName")
        .map_err(|error| FlameError::new(ErrorCode::InvalidArgument, error))?
        .append1(name.to_owned());
    let reply = pump.send_with_reply(call, Duration::from_secs(2))?;
    if reply.msg_type() == MessageType::Error {
        return Err(FlameError::unavailable("D-Bus name release failed"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_api::panels::{TaskEntry, TaskEntryKind};
    use flamewm_api::{TaskEntryId, WindowRef};

    fn job(app_id: Option<&str>, percent: Option<u8>) -> JobView {
        JobView {
            owner: ":1.42".to_owned(),
            app_id: app_id.map(DesktopAppId::new),
            progress: JobProgress { percent },
            suspended: false,
        }
    }

    fn task(app_id: &str, id: u64) -> TaskEntry {
        let window = WindowRef::new(id, 1);
        TaskEntry {
            id: TaskEntryId::window(window),
            app_id: DesktopAppId::new(app_id),
            kind: TaskEntryKind::Window { window },
            order_index: id as usize,
        }
    }

    fn panels(tasks: Vec<TaskEntry>) -> PanelsSnapshot {
        PanelsSnapshot {
            revision: 1,
            panels: Vec::new(),
            tasks,
            pinned_apps: Vec::new(),
        }
    }

    #[test]
    fn task_progress_averages_jobs_and_suppresses_unknown_or_ambiguous_identity() {
        let mut jobs = BTreeMap::new();
        jobs.insert("/job/1".to_owned(), job(Some("org.kde.dolphin"), Some(20)));
        jobs.insert("/job/2".to_owned(), job(Some("org.kde.dolphin"), Some(80)));
        let one_task = panels(vec![task("org.kde.dolphin", 1)]);
        let progress = task_progress_for_jobs(&jobs, &one_task);
        assert_eq!(progress.len(), 1);
        assert_eq!(progress[0].progress.percent(), 50);

        jobs.insert("/job/3".to_owned(), job(Some("org.kde.dolphin"), None));
        assert!(task_progress_for_jobs(&jobs, &one_task).is_empty());
        jobs.remove("/job/3");
        assert!(
            task_progress_for_jobs(
                &jobs,
                &panels(vec![task("org.kde.dolphin", 1), task("org.kde.dolphin", 2)])
            )
            .is_empty()
        );
        jobs.insert("/job/3".to_owned(), job(None, Some(40)));
        assert_eq!(task_progress_for_jobs(&jobs, &one_task).len(), 1);
    }

    #[test]
    fn kf6_desktop_entry_identity_is_strict_and_v1_display_names_are_not_identity() {
        assert_eq!(
            desktop_app_id("org.kde.dolphin"),
            Some(DesktopAppId::new("org.kde.dolphin"))
        );
        assert_eq!(desktop_app_id("../dolphin"), None);
        assert_eq!(desktop_app_id("org.kde.dolphin.desktop"), None);
        let items = [
            MessageItem::Str("Dolphin".to_owned()),
            MessageItem::Str("folder".to_owned()),
            MessageItem::Int32(0),
        ];
        assert_eq!(request_app_id(JOB_VIEW_SERVER_INTERFACE, &items), Ok(None));
    }

    #[test]
    fn kf6_jobview_v3_updates_percent_and_suspend_state_and_clears_on_termination() {
        let mut job = job(Some("org.kde.dolphin"), None);
        let update = MessageItem::from_dict(
            [
                Ok::<_, ()>(("percent".to_owned(), MessageItem::UInt32(67))),
                Ok(("suspended".to_owned(), MessageItem::Bool(true))),
            ]
            .into_iter(),
        );
        let MessageItem::Dict(_) = update.as_ref().unwrap() else {
            panic!("request update should be a dict");
        };
        assert_eq!(
            apply_job_update(
                &mut job,
                JOB_VIEW_V3_INTERFACE,
                "update",
                &[update.unwrap()]
            ),
            Ok(JobUpdate::Changed)
        );
        assert_eq!(job.progress.percent, Some(67));
        assert!(job.suspended);

        assert_eq!(
            apply_job_update(
                &mut job,
                JOB_VIEW_V3_INTERFACE,
                "terminate",
                &[
                    MessageItem::UInt32(0),
                    MessageItem::Str(String::new()),
                    MessageItem::from_dict(std::iter::empty::<Result<(String, MessageItem), ()>>())
                        .unwrap()
                ],
            ),
            Ok(JobUpdate::Terminated)
        );
    }

    #[test]
    fn malformed_percent_clears_progress_and_v2_suspend_resume_is_stateful() {
        let mut job = job(Some("org.kde.dolphin"), Some(42));
        assert_eq!(
            apply_job_update(
                &mut job,
                JOB_VIEW_V2_INTERFACE,
                "setPercent",
                &[MessageItem::UInt32(101)]
            ),
            Ok(JobUpdate::Changed)
        );
        assert_eq!(job.progress.percent, None);
        assert_eq!(
            apply_job_update(
                &mut job,
                JOB_VIEW_V2_INTERFACE,
                "setSuspended",
                &[MessageItem::Bool(true)]
            ),
            Ok(JobUpdate::Changed)
        );
        assert!(job.suspended);
        assert_eq!(
            apply_job_update(
                &mut job,
                JOB_VIEW_V2_INTERFACE,
                "setSuspended",
                &[MessageItem::Bool(false)]
            ),
            Ok(JobUpdate::Changed)
        );
        assert!(!job.suspended);
    }

    #[test]
    fn job_paths_are_monotonic_and_owner_bound() {
        let mut jobs = BTreeMap::new();
        let first = allocate_job_id(&jobs, &mut 1).unwrap();
        jobs.insert(
            format!("{JOB_PATH_PREFIX}{first}"),
            job(Some("org.kde.dolphin"), Some(1)),
        );
        let mut next = first + 1;
        assert_eq!(allocate_job_id(&jobs, &mut next), Some(2));
        assert_eq!(
            jobs.get(&format!("{JOB_PATH_PREFIX}{first}"))
                .unwrap()
                .owner,
            ":1.42"
        );
    }
}
