use std::str::FromStr;

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

pub const DEFAULT_QUERY_INTERVAL_MS: u64 = 3_000;
pub const MIN_QUERY_INTERVAL_MS: u64 = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId(pub Uuid);

impl TaskId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for TaskId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PassengerId(pub Uuid);

impl PassengerId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for PassengerId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Station {
    pub name: String,
    pub code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PassengerType {
    Adult,
    Child,
    Student,
    DisabledMilitary,
}

impl PassengerType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Adult => "adult",
            Self::Child => "child",
            Self::Student => "student",
            Self::DisabledMilitary => "disabled_military",
        }
    }
}

impl FromStr for PassengerType {
    type Err = ParseDomainValueError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "adult" => Ok(Self::Adult),
            "child" => Ok(Self::Child),
            "student" => Ok(Self::Student),
            "disabled_military" => Ok(Self::DisabledMilitary),
            _ => Err(ParseDomainValueError {
                kind: "passenger_type",
                value: value.to_string(),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Passenger {
    pub id: PassengerId,
    pub name: String,
    pub id_masked: String,
    pub passenger_type: PassengerType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeatType {
    Business,
    FirstClass,
    SecondClass,
    SoftSleeper,
    HardSleeper,
    HardSeat,
    NoSeat,
}

impl SeatType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Business => "business",
            Self::FirstClass => "first_class",
            Self::SecondClass => "second_class",
            Self::SoftSleeper => "soft_sleeper",
            Self::HardSleeper => "hard_sleeper",
            Self::HardSeat => "hard_seat",
            Self::NoSeat => "no_seat",
        }
    }
}

impl FromStr for SeatType {
    type Err = ParseDomainValueError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "business" => Ok(Self::Business),
            "first_class" => Ok(Self::FirstClass),
            "second_class" => Ok(Self::SecondClass),
            "soft_sleeper" => Ok(Self::SoftSleeper),
            "hard_sleeper" => Ok(Self::HardSleeper),
            "hard_seat" => Ok(Self::HardSeat),
            "no_seat" => Ok(Self::NoSeat),
            _ => Err(ParseDomainValueError {
                kind: "seat_type",
                value: value.to_string(),
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Created,
    Running,
    Querying,
    WaitingLogin,
    VerificationRequired,
    Paused,
    Submitting,
    PendingPayment,
    CandidateSubmitting,
    CandidateSubmitted,
    CandidatePendingPayment,
    Failed,
    Cancelled,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Running => "running",
            Self::Querying => "querying",
            Self::WaitingLogin => "waiting_login",
            Self::VerificationRequired => "verification_required",
            Self::Paused => "paused",
            Self::Submitting => "submitting",
            Self::PendingPayment => "pending_payment",
            Self::CandidateSubmitting => "candidate_submitting",
            Self::CandidateSubmitted => "candidate_submitted",
            Self::CandidatePendingPayment => "candidate_pending_payment",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn can_transition_to(self, next: Self) -> bool {
        use TaskStatus::*;

        matches!(
            (self, next),
            (Created, Running)
                | (Created, Cancelled)
                | (Running, Querying)
                | (Running, Paused)
                | (Running, Cancelled)
                | (Querying, Submitting)
                | (Querying, CandidateSubmitting)
                | (Querying, WaitingLogin)
                | (Querying, VerificationRequired)
                | (Querying, Paused)
                | (Querying, Failed)
                | (Querying, Cancelled)
                | (WaitingLogin, Running)
                | (WaitingLogin, VerificationRequired)
                | (WaitingLogin, Cancelled)
                | (VerificationRequired, Running)
                | (VerificationRequired, Cancelled)
                | (Paused, Running)
                | (Paused, Cancelled)
                | (Submitting, Querying)
                | (Submitting, PendingPayment)
                | (Submitting, WaitingLogin)
                | (Submitting, VerificationRequired)
                | (Submitting, Failed)
                | (CandidateSubmitting, CandidateSubmitted)
                | (CandidateSubmitting, Querying)
                | (CandidateSubmitting, WaitingLogin)
                | (CandidateSubmitting, VerificationRequired)
                | (CandidateSubmitting, Failed)
                | (CandidateSubmitted, CandidatePendingPayment)
                | (CandidateSubmitted, Cancelled)
                | (PendingPayment, Cancelled)
                | (CandidatePendingPayment, Cancelled)
        )
    }
}

impl FromStr for TaskStatus {
    type Err = ParseDomainValueError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "created" => Ok(Self::Created),
            "running" => Ok(Self::Running),
            "querying" => Ok(Self::Querying),
            "waiting_login" => Ok(Self::WaitingLogin),
            "verification_required" => Ok(Self::VerificationRequired),
            "paused" => Ok(Self::Paused),
            "submitting" => Ok(Self::Submitting),
            "pending_payment" => Ok(Self::PendingPayment),
            "candidate_submitting" => Ok(Self::CandidateSubmitting),
            "candidate_submitted" => Ok(Self::CandidateSubmitted),
            "candidate_pending_payment" => Ok(Self::CandidatePendingPayment),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(ParseDomainValueError {
                kind: "task_status",
                value: value.to_string(),
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderState {
    None,
    Submitting,
    PendingPayment,
    Expired,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitlistState {
    None,
    Submitting,
    Submitted,
    FulfilledPendingPayment,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrainFilterKind {
    Include,
    Exclude,
}

impl TrainFilterKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Include => "include",
            Self::Exclude => "exclude",
        }
    }
}

impl FromStr for TrainFilterKind {
    type Err = ParseDomainValueError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "include" => Ok(Self::Include),
            "exclude" => Ok(Self::Exclude),
            _ => Err(ParseDomainValueError {
                kind: "train_filter_kind",
                value: value.to_string(),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrainFilter {
    pub kind: TrainFilterKind,
    pub train_no: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NewTrainPolicy {
    #[default]
    Off,
    NotifyOnly,
    AutoOrder,
}

impl NewTrainPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::NotifyOnly => "notify_only",
            Self::AutoOrder => "auto_order",
        }
    }
}

impl FromStr for NewTrainPolicy {
    type Err = ParseDomainValueError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "off" => Ok(Self::Off),
            "notify_only" => Ok(Self::NotifyOnly),
            "auto_order" => Ok(Self::AutoOrder),
            _ => Err(ParseDomainValueError {
                kind: "new_train_policy",
                value: value.to_string(),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketTask {
    pub id: TaskId,
    pub from: Station,
    pub to: Station,
    pub dates: Vec<NaiveDate>,
    pub passengers: Vec<PassengerId>,
    pub seat_preferences: Vec<SeatType>,
    pub accept_no_seat: bool,
    pub train_filters: Vec<TrainFilter>,
    pub enable_waitlist: bool,
    pub enable_strong_waitlist: bool,
    pub new_train_policy: NewTrainPolicy,
    pub new_trains_only: bool,
    pub query_interval_ms: u64,
    pub status: TaskStatus,
    pub remark: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewTicketTask {
    pub from: Station,
    pub to: Station,
    pub dates: Vec<NaiveDate>,
    pub passengers: Vec<PassengerId>,
    pub seat_preferences: Vec<SeatType>,
    pub accept_no_seat: bool,
    pub train_filters: Vec<TrainFilter>,
    pub enable_waitlist: bool,
    pub enable_strong_waitlist: bool,
    pub new_train_policy: NewTrainPolicy,
    pub new_trains_only: bool,
    pub query_interval_ms: Option<u64>,
    pub remark: Option<String>,
}

impl NewTicketTask {
    pub fn build(self) -> Result<TicketTask, TaskValidationError> {
        let query_interval_ms = self.query_interval_ms.unwrap_or(DEFAULT_QUERY_INTERVAL_MS);
        validate_task(&self, query_interval_ms)?;

        let now = Utc::now();
        Ok(TicketTask {
            id: TaskId::new(),
            from: self.from,
            to: self.to,
            dates: self.dates,
            passengers: self.passengers,
            seat_preferences: self.seat_preferences,
            accept_no_seat: self.accept_no_seat,
            train_filters: self.train_filters,
            enable_waitlist: self.enable_waitlist,
            enable_strong_waitlist: self.enable_strong_waitlist,
            new_train_policy: self.new_train_policy,
            new_trains_only: self.new_trains_only,
            query_interval_ms,
            status: TaskStatus::Created,
            remark: self.remark,
            created_at: now,
            updated_at: now,
        })
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TaskValidationError {
    #[error("at least one travel date is required")]
    MissingDate,
    #[error("at least one passenger is required")]
    MissingPassenger,
    #[error("at least one seat preference is required unless no-seat fallback is enabled")]
    MissingSeatPreference,
    #[error("strong waitlist requires waitlist to be enabled")]
    StrongWaitlistRequiresWaitlist,
    #[error("new-trains-only tasks require notify_only or auto_order policy")]
    NewTrainsOnlyRequiresPolicy,
    #[error("notify-only new-train tasks cannot enable waitlist")]
    NotifyOnlyCannotWaitlist,
    #[error("query interval must be at least {min_ms}ms")]
    QueryIntervalTooLow { min_ms: u64 },
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("invalid {kind}: {value}")]
pub struct ParseDomainValueError {
    pub kind: &'static str,
    pub value: String,
}

fn validate_task(task: &NewTicketTask, query_interval_ms: u64) -> Result<(), TaskValidationError> {
    if task.dates.is_empty() {
        return Err(TaskValidationError::MissingDate);
    }
    if task.passengers.is_empty()
        && !(task.new_trains_only && task.new_train_policy == NewTrainPolicy::NotifyOnly)
    {
        return Err(TaskValidationError::MissingPassenger);
    }
    if task.seat_preferences.is_empty() && !task.accept_no_seat {
        return Err(TaskValidationError::MissingSeatPreference);
    }
    if task.enable_strong_waitlist && !task.enable_waitlist {
        return Err(TaskValidationError::StrongWaitlistRequiresWaitlist);
    }
    if task.new_trains_only && task.new_train_policy == NewTrainPolicy::Off {
        return Err(TaskValidationError::NewTrainsOnlyRequiresPolicy);
    }
    if task.new_trains_only
        && task.new_train_policy == NewTrainPolicy::NotifyOnly
        && task.enable_waitlist
    {
        return Err(TaskValidationError::NotifyOnlyCannotWaitlist);
    }
    if query_interval_ms < MIN_QUERY_INTERVAL_MS {
        return Err(TaskValidationError::QueryIntervalTooLow {
            min_ms: MIN_QUERY_INTERVAL_MS,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn station(name: &str, code: &str) -> Station {
        Station {
            name: name.to_string(),
            code: code.to_string(),
        }
    }

    fn valid_draft() -> NewTicketTask {
        NewTicketTask {
            from: station("上海", "SHH"),
            to: station("北京", "BJP"),
            dates: vec![NaiveDate::from_ymd_opt(2026, 7, 10).unwrap()],
            passengers: vec![PassengerId::new()],
            seat_preferences: vec![SeatType::SecondClass],
            accept_no_seat: false,
            train_filters: Vec::new(),
            enable_waitlist: true,
            enable_strong_waitlist: true,
            new_train_policy: NewTrainPolicy::Off,
            new_trains_only: false,
            query_interval_ms: Some(DEFAULT_QUERY_INTERVAL_MS),
            remark: None,
        }
    }

    #[test]
    fn builds_valid_task_with_created_status() {
        let task = valid_draft().build().unwrap();

        assert_eq!(task.status, TaskStatus::Created);
        assert_eq!(task.query_interval_ms, DEFAULT_QUERY_INTERVAL_MS);
        assert!(task.enable_strong_waitlist);
    }

    #[test]
    fn rejects_strong_waitlist_without_waitlist() {
        let mut draft = valid_draft();
        draft.enable_waitlist = false;

        let error = draft.build().unwrap_err();

        assert_eq!(error, TaskValidationError::StrongWaitlistRequiresWaitlist);
    }

    #[test]
    fn rejects_query_interval_below_minimum() {
        let mut draft = valid_draft();
        draft.query_interval_ms = Some(500);

        let error = draft.build().unwrap_err();

        assert_eq!(
            error,
            TaskValidationError::QueryIntervalTooLow {
                min_ms: MIN_QUERY_INTERVAL_MS
            }
        );
    }

    #[test]
    fn allows_notify_only_new_train_monitor_without_passengers() {
        let mut draft = valid_draft();
        draft.passengers.clear();
        draft.enable_waitlist = false;
        draft.enable_strong_waitlist = false;
        draft.new_train_policy = NewTrainPolicy::NotifyOnly;
        draft.new_trains_only = true;

        let task = draft.build().unwrap();

        assert!(task.passengers.is_empty());
        assert!(task.new_trains_only);
    }

    #[test]
    fn rejects_new_train_only_task_without_policy() {
        let mut draft = valid_draft();
        draft.new_trains_only = true;

        assert_eq!(
            draft.build().unwrap_err(),
            TaskValidationError::NewTrainsOnlyRequiresPolicy
        );
    }

    #[test]
    fn allows_no_seat_without_seat_preferences_when_explicitly_enabled() {
        let mut draft = valid_draft();
        draft.seat_preferences.clear();
        draft.accept_no_seat = true;

        let task = draft.build().unwrap();

        assert!(task.accept_no_seat);
        assert!(task.seat_preferences.is_empty());
    }

    #[test]
    fn pending_payment_is_terminal_for_automatic_progress() {
        assert!(!TaskStatus::PendingPayment.can_transition_to(TaskStatus::Querying));
        assert!(TaskStatus::PendingPayment.can_transition_to(TaskStatus::Cancelled));
    }

    #[test]
    fn candidate_submitted_can_become_candidate_pending_payment() {
        assert!(
            TaskStatus::CandidateSubmitted.can_transition_to(TaskStatus::CandidatePendingPayment)
        );
    }
}
