use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Arc, Mutex},
};

use chrono::{DateTime, Utc};
use rs12306_core::{Passenger, PassengerId, PassengerType, SeatType, TaskStatus, TicketTask};
use rusqlite::{Connection, params};
use serde::Serialize;
use thiserror::Error;
use uuid::Uuid;

pub const DEFAULT_DATABASE_PATH: &str = "./data/12306-rs.sqlite";

const MIGRATIONS: &str = r#"
pragma foreign_keys = on;

create table if not exists app_settings (
    key text primary key,
    value text not null,
    updated_at text not null
);

create table if not exists request_rate_limits (
    name text primary key,
    next_allowed_at_ms integer not null
);

create table if not exists sessions (
    id text primary key,
    state text not null,
    cookies_json text,
    csrf_token text,
    expires_at text,
    created_at text not null,
    updated_at text not null
);

create table if not exists passengers (
    id text primary key,
    name text not null,
    id_masked text not null,
    passenger_type text not null,
    raw_ref text,
    created_at text not null,
    updated_at text not null
);

create table if not exists ticket_tasks (
    id text primary key,
    from_name text not null,
    from_code text not null,
    to_name text not null,
    to_code text not null,
    accept_no_seat integer not null,
    enable_waitlist integer not null,
    enable_strong_waitlist integer not null,
    query_interval_ms integer not null,
    status text not null,
    remark text,
    created_at text not null,
    updated_at text not null
);

create table if not exists task_dates (
    task_id text not null references ticket_tasks(id) on delete cascade,
    travel_date text not null,
    priority integer not null,
    primary key (task_id, travel_date)
);

create table if not exists task_passengers (
    task_id text not null references ticket_tasks(id) on delete cascade,
    passenger_id text not null,
    priority integer not null,
    primary key (task_id, passenger_id)
);

create table if not exists task_seat_filters (
    task_id text not null references ticket_tasks(id) on delete cascade,
    seat_type text not null,
    priority integer not null,
    primary key (task_id, seat_type)
);

create table if not exists task_train_filters (
    task_id text not null references ticket_tasks(id) on delete cascade,
    filter_type text not null,
    train_no text not null,
    primary key (task_id, filter_type, train_no)
);

create table if not exists task_new_train_settings (
    task_id text primary key references ticket_tasks(id) on delete cascade,
    policy text not null,
    new_trains_only integer not null
);

create table if not exists task_execution_settings (
    task_id text primary key references ticket_tasks(id) on delete cascade,
    depart_after text,
    depart_before text,
    start_at text,
    choose_seats text
);

create table if not exists task_train_monitor_dates (
    task_id text not null references ticket_tasks(id) on delete cascade,
    travel_date text not null,
    initialized_at text not null,
    primary key (task_id, travel_date)
);

create table if not exists task_train_observations (
    task_id text not null references ticket_tasks(id) on delete cascade,
    travel_date text not null,
    train_no text not null,
    is_new integer not null,
    added_notified_at text,
    available_notified_at text,
    first_seen_at text not null,
    last_seen_at text not null,
    primary key (task_id, travel_date, train_no)
);

create table if not exists task_train_claims (
    task_id text not null references ticket_tasks(id) on delete cascade,
    travel_date text not null,
    train_no text not null,
    action text not null,
    created_at text not null,
    primary key (task_id, travel_date, train_no, action)
);

create table if not exists task_logs (
    id text primary key,
    task_id text not null references ticket_tasks(id) on delete cascade,
    level text not null,
    event text not null,
    message text not null,
    context_json text,
    created_at text not null
);

create table if not exists orders (
    id text primary key,
    task_id text not null references ticket_tasks(id) on delete cascade,
    order_no text,
    train_no text not null,
    travel_date text not null,
    seat_type text not null,
    state text not null,
    pay_deadline text,
    created_at text not null,
    updated_at text not null
);

create table if not exists standby_orders (
    id text primary key,
    task_id text not null references ticket_tasks(id) on delete cascade,
    standby_no text,
    state text not null,
    queue_position integer,
    pay_deadline text,
    created_at text not null,
    updated_at text not null
);

pragma user_version = 2;
"#;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("failed to prepare database directory {path}: {source}")]
    PrepareDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to secure database file {path}: {source}")]
    SecureDatabaseFile {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("database connection lock was poisoned")]
    LockPoisoned,
    #[error("task not found: {0}")]
    TaskNotFound(String),
    #[error("invalid stored {kind}: {value}")]
    InvalidStoredValue { kind: &'static str, value: String },
    #[error("invalid task status transition: {from} -> {to}")]
    InvalidStatusTransition { from: String, to: String },
}

#[derive(Debug, Clone)]
pub struct Database {
    connection: Arc<Mutex<Connection>>,
}

impl Database {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let path = path.as_ref();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(|source| StorageError::PrepareDirectory {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let connection = Connection::open(path)?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.execute_batch(
            "pragma journal_mode = wal; pragma synchronous = normal; pragma foreign_keys = on;",
        )?;
        secure_database_file(path)?;
        let database = Self {
            connection: Arc::new(Mutex::new(connection)),
        };
        database.init()?;
        Ok(database)
    }

    pub fn open_in_memory() -> Result<Self, StorageError> {
        let connection = Connection::open_in_memory()?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        let database = Self {
            connection: Arc::new(Mutex::new(connection)),
        };
        database.init()?;
        Ok(database)
    }

    pub fn init(&self) -> Result<(), StorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StorageError::LockPoisoned)?;
        connection.execute_batch(MIGRATIONS)?;
        Ok(())
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>, StorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StorageError::LockPoisoned)?;
        let mut statement = connection.prepare("select value from app_settings where key = ?1")?;
        match statement.query_row(params![key], |row| row.get(0)) {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(StorageError::Sqlite(error)),
        }
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), StorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StorageError::LockPoisoned)?;
        let updated_at = format_datetime(Utc::now());
        connection.execute(
            r#"
            insert into app_settings (key, value, updated_at) values (?1, ?2, ?3)
            on conflict(key) do update set
                value = excluded.value,
                updated_at = excluded.updated_at
            "#,
            params![key, value, updated_at],
        )?;
        Ok(())
    }

    pub fn delete_setting(&self, key: &str) -> Result<(), StorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StorageError::LockPoisoned)?;
        connection.execute("delete from app_settings where key = ?1", params![key])?;
        Ok(())
    }

    pub fn reserve_request_slot(
        &self,
        name: &str,
        min_spacing_ms: u64,
    ) -> Result<u64, StorageError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| StorageError::LockPoisoned)?;
        let transaction =
            connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let now = Utc::now().timestamp_millis();
        let current = transaction
            .query_row(
                "select next_allowed_at_ms from request_rate_limits where name = ?1",
                params![name],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or(now);
        let reserved_at = current.max(now);
        transaction.execute(
            r#"
            insert into request_rate_limits (name, next_allowed_at_ms) values (?1, ?2)
            on conflict(name) do update set next_allowed_at_ms = excluded.next_allowed_at_ms
            "#,
            params![name, reserved_at.saturating_add(min_spacing_ms as i64)],
        )?;
        transaction.commit()?;
        Ok(reserved_at.saturating_sub(now) as u64)
    }

    pub fn save_passenger(&self, passenger: &Passenger) -> Result<(), StorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StorageError::LockPoisoned)?;
        let now = format_datetime(Utc::now());
        connection.execute(
            r#"
            insert into passengers (
                id, name, id_masked, passenger_type, created_at, updated_at
            ) values (?1, ?2, ?3, ?4, ?5, ?6)
            on conflict(id) do update set
                name = excluded.name,
                id_masked = excluded.id_masked,
                passenger_type = excluded.passenger_type,
                updated_at = excluded.updated_at
            "#,
            params![
                passenger.id.0.to_string(),
                &passenger.name,
                &passenger.id_masked,
                passenger.passenger_type.as_str(),
                &now,
                &now
            ],
        )?;
        Ok(())
    }

    pub fn list_passengers(&self) -> Result<Vec<Passenger>, StorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StorageError::LockPoisoned)?;
        let mut statement = connection.prepare(
            r#"
            select id, name, id_masked, passenger_type
            from passengers
            order by updated_at desc
            "#,
        )?;
        let passengers = statement
            .query_map([], |row| {
                let id = row.get::<_, String>(0)?;
                let passenger_type = row.get::<_, String>(3)?;
                Ok((id, row.get(1)?, row.get(2)?, passenger_type))
            })?
            .map(|row| {
                let (id, name, id_masked, passenger_type) = row?;
                Ok(Passenger {
                    id: PassengerId(Uuid::parse_str(&id).map_err(|_| {
                        StorageError::InvalidStoredValue {
                            kind: "passenger_id",
                            value: id.clone(),
                        }
                    })?),
                    name,
                    id_masked,
                    passenger_type: PassengerType::from_str(&passenger_type).map_err(|_| {
                        StorageError::InvalidStoredValue {
                            kind: "passenger_type",
                            value: passenger_type.clone(),
                        }
                    })?,
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        Ok(passengers)
    }

    pub fn save_task(&self, task: &TicketTask) -> Result<(), StorageError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| StorageError::LockPoisoned)?;
        let transaction = connection.transaction()?;
        let created_at = format_datetime(task.created_at);
        let updated_at = format_datetime(task.updated_at);
        let task_id = task.id.0.to_string();
        let old_route = transaction
            .query_row(
                "select from_code, to_code from ticket_tasks where id = ?1",
                params![&task_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .ok();
        let old_dates = query_string_list(
            &transaction,
            "select travel_date from task_dates where task_id = ?1",
            &task_id,
        )?;

        transaction.execute(
            r#"
            insert into ticket_tasks (
                id, from_name, from_code, to_name, to_code, accept_no_seat,
                enable_waitlist, enable_strong_waitlist, query_interval_ms,
                status, remark, created_at, updated_at
            ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
            on conflict(id) do update set
                from_name = excluded.from_name,
                from_code = excluded.from_code,
                to_name = excluded.to_name,
                to_code = excluded.to_code,
                accept_no_seat = excluded.accept_no_seat,
                enable_waitlist = excluded.enable_waitlist,
                enable_strong_waitlist = excluded.enable_strong_waitlist,
                query_interval_ms = excluded.query_interval_ms,
                status = excluded.status,
                remark = excluded.remark,
                updated_at = excluded.updated_at
            "#,
            params![
                &task_id,
                &task.from.name,
                &task.from.code,
                &task.to.name,
                &task.to.code,
                task.accept_no_seat,
                task.enable_waitlist,
                task.enable_strong_waitlist,
                task.query_interval_ms,
                task.status.as_str(),
                task.remark.as_deref(),
                &created_at,
                &updated_at
            ],
        )?;

        transaction.execute(
            r#"
            insert into task_new_train_settings (task_id, policy, new_trains_only)
            values (?1, ?2, ?3)
            on conflict(task_id) do update set
                policy = excluded.policy,
                new_trains_only = excluded.new_trains_only
            "#,
            params![
                &task_id,
                task.new_train_policy.as_str(),
                task.new_trains_only
            ],
        )?;

        let route_changed =
            old_route.is_some_and(|(from, to)| from != task.from.code || to != task.to.code);
        if route_changed {
            transaction.execute(
                "delete from task_train_monitor_dates where task_id = ?1",
                params![&task_id],
            )?;
            transaction.execute(
                "delete from task_train_observations where task_id = ?1",
                params![&task_id],
            )?;
        } else {
            let new_dates: HashSet<_> = task.dates.iter().map(ToString::to_string).collect();
            for removed_date in old_dates
                .iter()
                .filter(|date| !new_dates.contains(date.as_str()))
            {
                transaction.execute(
                    "delete from task_train_monitor_dates where task_id = ?1 and travel_date = ?2",
                    params![&task_id, removed_date],
                )?;
                transaction.execute(
                    "delete from task_train_observations where task_id = ?1 and travel_date = ?2",
                    params![&task_id, removed_date],
                )?;
            }
        }

        transaction.execute(
            "delete from task_dates where task_id = ?1",
            params![&task_id],
        )?;
        for (priority, date) in task.dates.iter().enumerate() {
            transaction.execute(
                "insert into task_dates (task_id, travel_date, priority) values (?1, ?2, ?3)",
                params![&task_id, date.to_string(), priority as i64],
            )?;
        }

        transaction.execute(
            "delete from task_passengers where task_id = ?1",
            params![&task_id],
        )?;
        for (priority, passenger_id) in task.passengers.iter().enumerate() {
            transaction.execute(
                "insert into task_passengers (task_id, passenger_id, priority) values (?1, ?2, ?3)",
                params![&task_id, passenger_id.0.to_string(), priority as i64],
            )?;
        }

        transaction.execute(
            "delete from task_seat_filters where task_id = ?1",
            params![&task_id],
        )?;
        for (priority, seat) in task.seat_preferences.iter().enumerate() {
            transaction.execute(
                "insert into task_seat_filters (task_id, seat_type, priority) values (?1, ?2, ?3)",
                params![&task_id, seat.as_str(), priority as i64],
            )?;
        }

        transaction.execute(
            "delete from task_train_filters where task_id = ?1",
            params![&task_id],
        )?;
        for filter in &task.train_filters {
            transaction.execute(
                "insert into task_train_filters (task_id, filter_type, train_no) values (?1, ?2, ?3)",
                params![&task_id, filter.kind.as_str(), &filter.train_no],
            )?;
        }

        transaction.commit()?;
        Ok(())
    }

    pub fn save_task_execution_settings(
        &self,
        task_id: &str,
        settings: &TaskExecutionSettings,
    ) -> Result<(), StorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StorageError::LockPoisoned)?;
        connection.execute(
            r#"
            insert into task_execution_settings (
                task_id, depart_after, depart_before, start_at, choose_seats
            ) values (?1, ?2, ?3, ?4, ?5)
            on conflict(task_id) do update set
                depart_after = excluded.depart_after,
                depart_before = excluded.depart_before,
                start_at = excluded.start_at,
                choose_seats = excluded.choose_seats
            "#,
            params![
                task_id,
                settings.depart_after.as_deref(),
                settings.depart_before.as_deref(),
                settings.start_at.as_deref(),
                settings.choose_seats.as_deref()
            ],
        )?;
        Ok(())
    }

    pub fn list_task_summaries(&self) -> Result<Vec<TaskSummary>, StorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StorageError::LockPoisoned)?;
        let mut statement = connection.prepare(
            r#"
            select id, from_name, to_name, status, query_interval_ms, updated_at
            from ticket_tasks
            order by updated_at desc
            "#,
        )?;
        let summaries = statement
            .query_map([], |row| {
                Ok(TaskSummary {
                    id: row.get(0)?,
                    from_name: row.get(1)?,
                    to_name: row.get(2)?,
                    status: row.get(3)?,
                    query_interval_ms: row.get::<_, i64>(4)? as u64,
                    updated_at: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(summaries)
    }

    pub fn get_task_details(&self, task_id: &str) -> Result<TaskDetails, StorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StorageError::LockPoisoned)?;
        let mut task = connection
            .query_row(
                r#"
                select id, from_name, from_code, to_name, to_code, accept_no_seat,
                    enable_waitlist, enable_strong_waitlist, query_interval_ms,
                    status, remark, created_at, updated_at,
                    coalesce(new_train.policy, 'off'),
                    coalesce(new_train.new_trains_only, 0)
                from ticket_tasks task
                left join task_new_train_settings new_train on new_train.task_id = task.id
                where task.id = ?1
                "#,
                params![task_id],
                |row| {
                    Ok(TaskDetails {
                        id: row.get(0)?,
                        from_name: row.get(1)?,
                        from_code: row.get(2)?,
                        to_name: row.get(3)?,
                        to_code: row.get(4)?,
                        accept_no_seat: row.get(5)?,
                        enable_waitlist: row.get(6)?,
                        enable_strong_waitlist: row.get(7)?,
                        query_interval_ms: row.get::<_, i64>(8)? as u64,
                        status: row.get(9)?,
                        remark: row.get(10)?,
                        created_at: row.get(11)?,
                        updated_at: row.get(12)?,
                        new_train_policy: row.get(13)?,
                        new_trains_only: row.get(14)?,
                        dates: Vec::new(),
                        passenger_ids: Vec::new(),
                        seat_types: Vec::new(),
                        train_include: Vec::new(),
                        train_exclude: Vec::new(),
                        depart_after: None,
                        depart_before: None,
                        start_at: None,
                        choose_seats: None,
                    })
                },
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => {
                    StorageError::TaskNotFound(task_id.to_string())
                }
                error => StorageError::Sqlite(error),
            })?;

        task.dates = query_string_list(
            &connection,
            "select travel_date from task_dates where task_id = ?1 order by priority asc",
            task_id,
        )?;
        task.passenger_ids = query_string_list(
            &connection,
            "select passenger_id from task_passengers where task_id = ?1 order by priority asc",
            task_id,
        )?;
        task.seat_types = query_string_list(
            &connection,
            "select seat_type from task_seat_filters where task_id = ?1 order by priority asc",
            task_id,
        )?;
        task.train_include = query_string_list(
            &connection,
            "select train_no from task_train_filters where task_id = ?1 and filter_type = 'include' order by train_no asc",
            task_id,
        )?;
        task.train_exclude = query_string_list(
            &connection,
            "select train_no from task_train_filters where task_id = ?1 and filter_type = 'exclude' order by train_no asc",
            task_id,
        )?;
        if let Ok(settings) = connection.query_row(
            r#"
            select depart_after, depart_before, start_at, choose_seats
            from task_execution_settings where task_id = ?1
            "#,
            params![task_id],
            |row| {
                Ok(TaskExecutionSettings {
                    depart_after: row.get(0)?,
                    depart_before: row.get(1)?,
                    start_at: row.get(2)?,
                    choose_seats: row.get(3)?,
                })
            },
        ) {
            task.depart_after = settings.depart_after;
            task.depart_before = settings.depart_before;
            task.start_at = settings.start_at;
            task.choose_seats = settings.choose_seats;
        }

        Ok(task)
    }

    pub fn observe_task_trains(
        &self,
        task_id: &str,
        travel_date: &str,
        train_numbers: &[String],
    ) -> Result<TrainObservationBatch, StorageError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| StorageError::LockPoisoned)?;
        let transaction = connection.transaction()?;
        let initialized = transaction.query_row(
            "select exists(select 1 from task_train_monitor_dates where task_id = ?1 and travel_date = ?2)",
            params![task_id, travel_date],
            |row| row.get::<_, bool>(0),
        )?;
        let now = Utc::now().to_rfc3339();

        if !initialized {
            transaction.execute(
                "insert into task_train_monitor_dates (task_id, travel_date, initialized_at) values (?1, ?2, ?3)",
                params![task_id, travel_date, &now],
            )?;
            for train_no in normalized_train_numbers(train_numbers) {
                transaction.execute(
                    r#"
                    insert or ignore into task_train_observations (
                        task_id, travel_date, train_no, is_new, first_seen_at, last_seen_at
                    ) values (?1, ?2, ?3, 0, ?4, ?4)
                    "#,
                    params![task_id, travel_date, train_no, &now],
                )?;
            }
            transaction.commit()?;
            return Ok(TrainObservationBatch {
                baseline_created: true,
                observations: Vec::new(),
            });
        }

        let mut observations = Vec::new();
        for train_no in normalized_train_numbers(train_numbers) {
            let existed = transaction.query_row(
                "select exists(select 1 from task_train_observations where task_id = ?1 and travel_date = ?2 and train_no = ?3)",
                params![task_id, travel_date, &train_no],
                |row| row.get::<_, bool>(0),
            )?;
            transaction.execute(
                r#"
                insert into task_train_observations (
                    task_id, travel_date, train_no, is_new, first_seen_at, last_seen_at
                ) values (?1, ?2, ?3, 1, ?4, ?4)
                on conflict(task_id, travel_date, train_no) do update set
                    last_seen_at = excluded.last_seen_at
                "#,
                params![task_id, travel_date, &train_no, &now],
            )?;
            let observation = transaction.query_row(
                r#"
                select train_no, is_new, added_notified_at is not null,
                    available_notified_at is not null
                from task_train_observations
                where task_id = ?1 and travel_date = ?2 and train_no = ?3
                "#,
                params![task_id, travel_date, &train_no],
                |row| {
                    Ok(NewTrainObservation {
                        train_no: row.get(0)?,
                        is_new: row.get(1)?,
                        first_observed: !existed,
                        added_notified: row.get(2)?,
                        available_notified: row.get(3)?,
                    })
                },
            )?;
            observations.push(observation);
        }
        transaction.commit()?;
        Ok(TrainObservationBatch {
            baseline_created: false,
            observations,
        })
    }

    pub fn mark_new_train_added_notified(
        &self,
        task_id: &str,
        travel_date: &str,
        train_no: &str,
    ) -> Result<(), StorageError> {
        self.mark_train_notification(task_id, travel_date, train_no, "added_notified_at")
    }

    pub fn list_new_trains(&self, task_id: &str) -> Result<Vec<NewTrainRecord>, StorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StorageError::LockPoisoned)?;
        let mut statement = connection.prepare(
            r#"
            select travel_date, train_no, added_notified_at is not null,
                available_notified_at is not null, first_seen_at, last_seen_at
            from task_train_observations
            where task_id = ?1 and is_new = 1
            order by travel_date asc, first_seen_at asc, train_no asc
            "#,
        )?;
        let records = statement
            .query_map(params![task_id], |row| {
                Ok(NewTrainRecord {
                    travel_date: row.get(0)?,
                    train_no: row.get(1)?,
                    added_notified: row.get(2)?,
                    available_notified: row.get(3)?,
                    first_seen_at: row.get(4)?,
                    last_seen_at: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    pub fn mark_new_train_available_notified(
        &self,
        task_id: &str,
        travel_date: &str,
        train_no: &str,
    ) -> Result<(), StorageError> {
        self.mark_train_notification(task_id, travel_date, train_no, "available_notified_at")
    }

    fn mark_train_notification(
        &self,
        task_id: &str,
        travel_date: &str,
        train_no: &str,
        column: &str,
    ) -> Result<(), StorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StorageError::LockPoisoned)?;
        let sql = format!(
            "update task_train_observations set {column} = ?1 where task_id = ?2 and travel_date = ?3 and train_no = ?4"
        );
        connection.execute(
            &sql,
            params![Utc::now().to_rfc3339(), task_id, travel_date, train_no],
        )?;
        Ok(())
    }

    pub fn try_claim_train_action(
        &self,
        task_id: &str,
        travel_date: &str,
        train_no: &str,
        action: &str,
    ) -> Result<bool, StorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StorageError::LockPoisoned)?;
        let changed = connection.execute(
            r#"
            insert or ignore into task_train_claims (
                task_id, travel_date, train_no, action, created_at
            ) values (?1, ?2, ?3, ?4, ?5)
            "#,
            params![
                task_id,
                travel_date,
                train_no.to_uppercase(),
                action,
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(changed == 1)
    }

    pub fn release_task_train_actions(&self, task_id: &str) -> Result<usize, StorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StorageError::LockPoisoned)?;
        Ok(connection.execute(
            "delete from task_train_claims where task_id = ?1",
            params![task_id],
        )?)
    }

    pub fn update_task_status(
        &self,
        task_id: &str,
        next_status: TaskStatus,
    ) -> Result<TaskDetails, StorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StorageError::LockPoisoned)?;
        let current_status = connection
            .query_row(
                "select status from ticket_tasks where id = ?1",
                params![task_id],
                |row| row.get::<_, String>(0),
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => {
                    StorageError::TaskNotFound(task_id.to_string())
                }
                error => StorageError::Sqlite(error),
            })?;
        let current_status = parse_status(&current_status)?;

        if !current_status.can_transition_to(next_status) {
            return Err(StorageError::InvalidStatusTransition {
                from: current_status.as_str().to_string(),
                to: next_status.as_str().to_string(),
            });
        }

        let now = Utc::now().to_rfc3339();
        connection.execute(
            "update ticket_tasks set status = ?1, updated_at = ?2 where id = ?3",
            params![next_status.as_str(), now, task_id],
        )?;

        drop(connection);
        self.append_task_log(
            task_id,
            "info",
            "task_status_changed",
            &format!(
                "task status changed: {} -> {}",
                current_status.as_str(),
                next_status.as_str()
            ),
            None,
        )?;
        self.get_task_details(task_id)
    }

    pub fn append_task_log(
        &self,
        task_id: &str,
        level: &str,
        event: &str,
        message: &str,
        context_json: Option<&str>,
    ) -> Result<(), StorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StorageError::LockPoisoned)?;
        connection.execute(
            r#"
            insert into task_logs (id, task_id, level, event, message, context_json, created_at)
            values (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                Uuid::new_v4().to_string(),
                task_id,
                level,
                event,
                message,
                context_json,
                Utc::now().to_rfc3339()
            ],
        )?;
        connection.execute(
            r#"
            delete from task_logs
            where task_id = ?1 and rowid not in (
                select rowid from task_logs where task_id = ?1
                order by rowid desc limit 5000
            )
            "#,
            params![task_id],
        )?;
        Ok(())
    }

    pub fn save_order(
        &self,
        task_id: &str,
        order_no: &str,
        train_no: &str,
        travel_date: &str,
        seat_type: &str,
    ) -> Result<(), StorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StorageError::LockPoisoned)?;
        let now = Utc::now().to_rfc3339();
        connection.execute(
            r#"
            insert into orders (
                id, task_id, order_no, train_no, travel_date, seat_type,
                state, created_at, updated_at
            ) values (?1, ?2, ?3, ?4, ?5, ?6, 'pending_payment', ?7, ?7)
            "#,
            params![
                Uuid::new_v4().to_string(),
                task_id,
                order_no,
                train_no,
                travel_date,
                seat_type,
                now
            ],
        )?;
        Ok(())
    }

    pub fn order_for_task(&self, task_id: &str) -> Result<Option<OrderRecord>, StorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StorageError::LockPoisoned)?;
        let order = connection.query_row(
            r#"
            select order_no, train_no, travel_date, seat_type, state, created_at
            from orders where task_id = ?1 order by created_at desc limit 1
            "#,
            params![task_id],
            |row| {
                Ok(OrderRecord {
                    order_no: row.get(0)?,
                    train_no: row.get(1)?,
                    travel_date: row.get(2)?,
                    seat_type: row.get(3)?,
                    state: row.get(4)?,
                    created_at: row.get(5)?,
                })
            },
        );
        match order {
            Ok(order) => Ok(Some(order)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(StorageError::Sqlite(error)),
        }
    }

    pub fn save_standby_order(
        &self,
        task_id: &str,
        standby_no: Option<&str>,
    ) -> Result<(), StorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StorageError::LockPoisoned)?;
        let now = Utc::now().to_rfc3339();
        connection.execute(
            r#"
            insert into standby_orders (
                id, task_id, standby_no, state, created_at, updated_at
            ) values (?1, ?2, ?3, 'submitted', ?4, ?4)
            "#,
            params![Uuid::new_v4().to_string(), task_id, standby_no, now],
        )?;
        Ok(())
    }

    pub fn list_task_logs(&self, task_id: &str) -> Result<Vec<TaskLog>, StorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StorageError::LockPoisoned)?;

        // Ensure the task exists so callers can distinguish an empty log from a missing task.
        connection
            .query_row(
                "select id from ticket_tasks where id = ?1",
                params![task_id],
                |row| row.get::<_, String>(0),
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => {
                    StorageError::TaskNotFound(task_id.to_string())
                }
                error => StorageError::Sqlite(error),
            })?;

        let mut statement = connection.prepare(
            r#"
            select id, task_id, level, event, message, context_json, created_at
            from task_logs
            where task_id = ?1
            order by created_at asc
            "#,
        )?;
        let logs = statement
            .query_map(params![task_id], |row| {
                Ok(TaskLog {
                    id: row.get(0)?,
                    task_id: row.get(1)?,
                    level: row.get(2)?,
                    event: row.get(3)?,
                    message: row.get(4)?,
                    context_json: row.get(5)?,
                    created_at: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(logs)
    }

    pub fn task_count(&self) -> Result<u64, StorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StorageError::LockPoisoned)?;
        let count = connection.query_row("select count(*) from ticket_tasks", [], |row| {
            row.get::<_, i64>(0)
        })?;
        Ok(count as u64)
    }

    pub fn session_state(&self) -> Result<String, StorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StorageError::LockPoisoned)?;
        let state = connection.query_row(
            "select state from sessions where id = 'default'",
            [],
            |row| row.get::<_, String>(0),
        );
        match state {
            Ok(state) => Ok(state),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok("logged_out".to_string()),
            Err(error) => Err(StorageError::Sqlite(error)),
        }
    }

    pub fn session_cookies(&self) -> Result<Option<String>, StorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StorageError::LockPoisoned)?;
        match connection.query_row(
            "select cookies_json from sessions where id = 'default'",
            [],
            |row| row.get::<_, Option<String>>(0),
        ) {
            Ok(cookies) => Ok(cookies),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(StorageError::Sqlite(error)),
        }
    }

    pub fn set_session_state(&self, state: &str) -> Result<(), StorageError> {
        self.set_session(state, None)
    }

    pub fn set_session(&self, state: &str, cookies: Option<&str>) -> Result<(), StorageError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| StorageError::LockPoisoned)?;
        let now = Utc::now().to_rfc3339();
        connection.execute(
            r#"
            insert into sessions (id, state, cookies_json, created_at, updated_at)
            values ('default', ?1, ?2, ?3, ?3)
            on conflict(id) do update set
                state = excluded.state,
                cookies_json = excluded.cookies_json,
                updated_at = excluded.updated_at
            "#,
            params![state, cookies, now],
        )?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskSummary {
    pub id: String,
    pub from_name: String,
    pub to_name: String,
    pub status: String,
    pub query_interval_ms: u64,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskDetails {
    pub id: String,
    pub from_name: String,
    pub from_code: String,
    pub to_name: String,
    pub to_code: String,
    pub accept_no_seat: bool,
    pub enable_waitlist: bool,
    pub enable_strong_waitlist: bool,
    pub new_train_policy: String,
    pub new_trains_only: bool,
    pub query_interval_ms: u64,
    pub status: String,
    pub remark: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub dates: Vec<String>,
    pub passenger_ids: Vec<String>,
    pub seat_types: Vec<String>,
    pub train_include: Vec<String>,
    pub train_exclude: Vec<String>,
    pub depart_after: Option<String>,
    pub depart_before: Option<String>,
    pub start_at: Option<String>,
    pub choose_seats: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TaskExecutionSettings {
    pub depart_after: Option<String>,
    pub depart_before: Option<String>,
    pub start_at: Option<String>,
    pub choose_seats: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrainObservationBatch {
    pub baseline_created: bool,
    pub observations: Vec<NewTrainObservation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTrainObservation {
    pub train_no: String,
    pub is_new: bool,
    pub first_observed: bool,
    pub added_notified: bool,
    pub available_notified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NewTrainRecord {
    pub travel_date: String,
    pub train_no: String,
    pub added_notified: bool,
    pub available_notified: bool,
    pub first_seen_at: String,
    pub last_seen_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskLog {
    pub id: String,
    pub task_id: String,
    pub level: String,
    pub event: String,
    pub message: String,
    pub context_json: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OrderRecord {
    pub order_no: String,
    pub train_no: String,
    pub travel_date: String,
    pub seat_type: String,
    pub state: String,
    pub created_at: String,
}

#[cfg(unix)]
fn secure_database_file(path: &Path) -> Result<(), StorageError> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|source| {
        StorageError::SecureDatabaseFile {
            path: path.to_path_buf(),
            source,
        }
    })
}

#[cfg(not(unix))]
fn secure_database_file(_path: &Path) -> Result<(), StorageError> {
    Ok(())
}

fn format_datetime(value: DateTime<Utc>) -> String {
    value.to_rfc3339()
}

pub fn status_for_storage(status: TaskStatus) -> &'static str {
    status.as_str()
}

fn query_string_list(
    connection: &Connection,
    sql: &str,
    task_id: &str,
) -> Result<Vec<String>, StorageError> {
    let mut statement = connection.prepare(sql)?;
    let values = statement
        .query_map(params![task_id], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(values)
}

fn normalized_train_numbers(train_numbers: &[String]) -> HashSet<String> {
    train_numbers
        .iter()
        .map(|train| train.trim().to_uppercase())
        .filter(|train| !train.is_empty())
        .collect()
}

fn parse_status(value: &str) -> Result<TaskStatus, StorageError> {
    TaskStatus::from_str(value).map_err(|_| StorageError::InvalidStoredValue {
        kind: "task_status",
        value: value.to_string(),
    })
}

#[allow(dead_code)]
fn parse_seat_type(value: &str) -> Result<SeatType, StorageError> {
    SeatType::from_str(value).map_err(|_| StorageError::InvalidStoredValue {
        kind: "seat_type",
        value: value.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use rs12306_core::{
        DEFAULT_QUERY_INTERVAL_MS, NewTicketTask, NewTrainPolicy, PassengerId, SeatType, Station,
        TrainFilter, TrainFilterKind,
    };

    use super::*;

    #[test]
    fn initializes_schema_and_stores_task() {
        let database = Database::open_in_memory().unwrap();
        let task = NewTicketTask {
            from: Station {
                name: "上海".to_string(),
                code: "SHH".to_string(),
            },
            to: Station {
                name: "北京".to_string(),
                code: "BJP".to_string(),
            },
            dates: vec![NaiveDate::from_ymd_opt(2026, 7, 10).unwrap()],
            passengers: vec![PassengerId::new()],
            seat_preferences: vec![SeatType::SecondClass],
            accept_no_seat: false,
            train_filters: vec![TrainFilter {
                kind: TrainFilterKind::Include,
                train_no: "G102".to_string(),
            }],
            enable_waitlist: true,
            enable_strong_waitlist: true,
            new_train_policy: NewTrainPolicy::Off,
            new_trains_only: false,
            query_interval_ms: Some(DEFAULT_QUERY_INTERVAL_MS),
            remark: None,
        }
        .build()
        .unwrap();

        database.save_task(&task).unwrap();

        assert_eq!(database.task_count().unwrap(), 1);
        let summaries = database.list_task_summaries().unwrap();
        assert_eq!(summaries[0].from_name, "上海");
        assert_eq!(summaries[0].status, "created");
    }

    #[test]
    fn reads_task_details_and_updates_status() {
        let database = Database::open_in_memory().unwrap();
        let task = NewTicketTask {
            from: Station {
                name: "上海".to_string(),
                code: "SHH".to_string(),
            },
            to: Station {
                name: "北京".to_string(),
                code: "BJP".to_string(),
            },
            dates: vec![NaiveDate::from_ymd_opt(2026, 7, 10).unwrap()],
            passengers: vec![PassengerId::new()],
            seat_preferences: vec![SeatType::SecondClass],
            accept_no_seat: false,
            train_filters: vec![TrainFilter {
                kind: TrainFilterKind::Include,
                train_no: "G102".to_string(),
            }],
            enable_waitlist: true,
            enable_strong_waitlist: true,
            new_train_policy: NewTrainPolicy::AutoOrder,
            new_trains_only: false,
            query_interval_ms: Some(DEFAULT_QUERY_INTERVAL_MS),
            remark: Some("demo".to_string()),
        }
        .build()
        .unwrap();
        let task_id = task.id.0.to_string();
        database.save_task(&task).unwrap();
        database
            .save_task_execution_settings(
                &task_id,
                &TaskExecutionSettings {
                    depart_after: Some("08:00".to_string()),
                    depart_before: Some("18:00".to_string()),
                    start_at: Some("2026-07-10T00:00:00+00:00".to_string()),
                    choose_seats: Some("1A".to_string()),
                },
            )
            .unwrap();

        let details = database.get_task_details(&task_id).unwrap();
        assert_eq!(details.dates, vec!["2026-07-10"]);
        assert_eq!(details.seat_types, vec!["second_class"]);
        assert_eq!(details.train_include, vec!["G102"]);
        assert_eq!(details.depart_after.as_deref(), Some("08:00"));
        assert_eq!(details.choose_seats.as_deref(), Some("1A"));

        database
            .save_order(&task_id, "E123456789", "G102", "2026-07-10", "second_class")
            .unwrap();
        let order = database.order_for_task(&task_id).unwrap().unwrap();
        assert_eq!(order.order_no, "E123456789");
        assert_eq!(order.state, "pending_payment");
        database
            .save_standby_order(&task_id, Some("H123456789"))
            .unwrap();

        let updated = database
            .update_task_status(&task_id, TaskStatus::Running)
            .unwrap();
        assert_eq!(updated.status, "running");
        assert_eq!(database.list_task_logs(&task_id).unwrap().len(), 1);
    }

    #[test]
    fn rejects_invalid_status_transition() {
        let database = Database::open_in_memory().unwrap();
        let task = NewTicketTask {
            from: Station {
                name: "上海".to_string(),
                code: "SHH".to_string(),
            },
            to: Station {
                name: "北京".to_string(),
                code: "BJP".to_string(),
            },
            dates: vec![NaiveDate::from_ymd_opt(2026, 7, 10).unwrap()],
            passengers: vec![PassengerId::new()],
            seat_preferences: vec![SeatType::SecondClass],
            accept_no_seat: false,
            train_filters: Vec::new(),
            enable_waitlist: false,
            enable_strong_waitlist: false,
            new_train_policy: NewTrainPolicy::Off,
            new_trains_only: false,
            query_interval_ms: Some(DEFAULT_QUERY_INTERVAL_MS),
            remark: None,
        }
        .build()
        .unwrap();
        let task_id = task.id.0.to_string();
        database.save_task(&task).unwrap();

        let error = database
            .update_task_status(&task_id, TaskStatus::PendingPayment)
            .unwrap_err();

        assert!(matches!(
            error,
            StorageError::InvalidStatusTransition { .. }
        ));
    }

    #[test]
    fn stores_session_state() {
        let database = Database::open_in_memory().unwrap();

        assert_eq!(database.session_state().unwrap(), "logged_out");
        database.set_session_state("verification_required").unwrap();

        assert_eq!(database.session_state().unwrap(), "verification_required");
    }

    #[test]
    fn stores_app_settings() {
        let database = Database::open_in_memory().unwrap();

        assert_eq!(database.get_setting("query_interval_ms").unwrap(), None);
        database.set_setting("query_interval_ms", "5000").unwrap();

        assert_eq!(
            database.get_setting("query_interval_ms").unwrap(),
            Some("5000".to_string())
        );
        database.delete_setting("query_interval_ms").unwrap();
        assert_eq!(database.get_setting("query_interval_ms").unwrap(), None);
    }

    #[test]
    fn tracks_new_trains_per_date_and_deduplicates_actions() {
        let database = Database::open_in_memory().unwrap();
        let mut task = NewTicketTask {
            from: Station {
                name: "上海".to_string(),
                code: "SHH".to_string(),
            },
            to: Station {
                name: "嘉兴".to_string(),
                code: "JXH".to_string(),
            },
            dates: vec![NaiveDate::from_ymd_opt(2026, 7, 10).unwrap()],
            passengers: vec![PassengerId::new()],
            seat_preferences: vec![SeatType::SecondClass],
            accept_no_seat: false,
            train_filters: Vec::new(),
            enable_waitlist: true,
            enable_strong_waitlist: false,
            new_train_policy: NewTrainPolicy::AutoOrder,
            new_trains_only: true,
            query_interval_ms: Some(DEFAULT_QUERY_INTERVAL_MS),
            remark: None,
        }
        .build()
        .unwrap();
        let task_id = task.id.0.to_string();
        database.save_task(&task).unwrap();

        let baseline = database
            .observe_task_trains(&task_id, "2026-07-10", &["G1".to_string()])
            .unwrap();
        assert!(baseline.baseline_created);
        assert!(baseline.observations.is_empty());

        let observed = database
            .observe_task_trains(
                &task_id,
                "2026-07-10",
                &["G1".to_string(), "G2".to_string()],
            )
            .unwrap();
        assert!(!observed.baseline_created);
        assert!(
            !observed
                .observations
                .iter()
                .find(|item| item.train_no == "G1")
                .unwrap()
                .is_new
        );
        let new_train = observed
            .observations
            .iter()
            .find(|item| item.train_no == "G2")
            .unwrap();
        assert!(new_train.is_new);
        assert!(!new_train.added_notified);

        database
            .mark_new_train_added_notified(&task_id, "2026-07-10", "G2")
            .unwrap();
        database
            .mark_new_train_available_notified(&task_id, "2026-07-10", "G2")
            .unwrap();
        let observed = database
            .observe_task_trains(&task_id, "2026-07-10", &["G2".to_string()])
            .unwrap();
        assert!(observed.observations[0].added_notified);
        assert!(observed.observations[0].available_notified);

        assert!(
            database
                .try_claim_train_action(&task_id, "2026-07-10", "G2", "order")
                .unwrap()
        );
        assert!(
            !database
                .try_claim_train_action(&task_id, "2026-07-10", "G2", "order")
                .unwrap()
        );
        assert_eq!(database.release_task_train_actions(&task_id).unwrap(), 1);
        assert!(
            database
                .try_claim_train_action(&task_id, "2026-07-10", "G2", "order")
                .unwrap()
        );

        task.from.code = "AOH".to_string();
        database.save_task(&task).unwrap();
        assert!(
            database
                .observe_task_trains(&task_id, "2026-07-10", &["G2".to_string()])
                .unwrap()
                .baseline_created
        );
    }

    #[test]
    fn stores_passengers() {
        let database = Database::open_in_memory().unwrap();
        let passenger = Passenger {
            id: PassengerId::new(),
            name: "张三".to_string(),
            id_masked: "110***********1234".to_string(),
            passenger_type: PassengerType::Adult,
        };

        database.save_passenger(&passenger).unwrap();

        assert_eq!(database.list_passengers().unwrap(), vec![passenger]);
    }

    #[test]
    fn reserves_global_request_slots() {
        let database = Database::open_in_memory().unwrap();

        assert_eq!(database.reserve_request_slot("query", 500).unwrap(), 0);
        assert!(database.reserve_request_slot("query", 500).unwrap() >= 490);
    }
}
