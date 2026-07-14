use rs12306_client_12306::{TicketAvailability, TicketCandidate};
use rs12306_core::{NewTrainPolicy, SeatType, TicketTask, TrainFilterKind};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SchedulerError {
    #[error("task is not runnable from current status")]
    TaskNotRunnable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskDecision {
    SubmitOrder(TicketCandidate),
    SubmitStrongWaitlist,
    ContinueQuerying,
}

pub fn decide_next_action(task: &TicketTask, candidates: &[TicketCandidate]) -> TaskDecision {
    if task.new_trains_only && task.new_train_policy == NewTrainPolicy::NotifyOnly {
        return TaskDecision::ContinueQuerying;
    }
    if let Some(candidate) = candidates.iter().find(|candidate| {
        matches_task(task, candidate) && has_acceptable_availability(task, candidate)
    }) {
        return TaskDecision::SubmitOrder(candidate.clone());
    }

    if task.enable_waitlist && task.enable_strong_waitlist {
        return TaskDecision::SubmitStrongWaitlist;
    }

    TaskDecision::ContinueQuerying
}

fn matches_task(task: &TicketTask, candidate: &TicketCandidate) -> bool {
    if !task.dates.contains(&candidate.date) {
        return false;
    }

    if task.from.code != candidate.from.code || task.to.code != candidate.to.code {
        return false;
    }

    let includes: Vec<_> = task
        .train_filters
        .iter()
        .filter(|filter| filter.kind == TrainFilterKind::Include)
        .collect();
    if !includes.is_empty()
        && !includes
            .iter()
            .any(|filter| filter.train_no.eq_ignore_ascii_case(&candidate.train_no))
    {
        return false;
    }

    let excluded = task
        .train_filters
        .iter()
        .filter(|filter| filter.kind == TrainFilterKind::Exclude)
        .any(|filter| filter.train_no.eq_ignore_ascii_case(&candidate.train_no));
    if excluded {
        return false;
    }

    true
}

fn has_acceptable_availability(task: &TicketTask, candidate: &TicketCandidate) -> bool {
    match candidate.remaining {
        TicketAvailability::Available { .. } | TicketAvailability::Limited => {
            task.seat_preferences.contains(&candidate.seat_type)
        }
        TicketAvailability::NoSeatOnly => {
            task.accept_no_seat && candidate.seat_type == SeatType::NoSeat
        }
        TicketAvailability::SoldOut => false,
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use rs12306_client_12306::{TicketAvailability, TicketCandidate};
    use rs12306_core::{
        DEFAULT_QUERY_INTERVAL_MS, NewTicketTask, NewTrainPolicy, PassengerId, SeatType, Station,
        TrainFilter, TrainFilterKind,
    };

    use super::*;

    fn station(name: &str, code: &str) -> Station {
        Station {
            name: name.to_string(),
            code: code.to_string(),
        }
    }

    fn task(enable_waitlist: bool, enable_strong_waitlist: bool) -> TicketTask {
        NewTicketTask {
            from: station("上海", "SHH"),
            to: station("北京", "BJP"),
            dates: vec![NaiveDate::from_ymd_opt(2026, 7, 10).unwrap()],
            passengers: vec![PassengerId::new()],
            seat_preferences: vec![SeatType::SecondClass],
            accept_no_seat: false,
            train_filters: vec![TrainFilter {
                kind: TrainFilterKind::Include,
                train_no: "G102".to_string(),
            }],
            enable_waitlist,
            enable_strong_waitlist,
            new_train_policy: NewTrainPolicy::Off,
            new_trains_only: false,
            query_interval_ms: Some(DEFAULT_QUERY_INTERVAL_MS),
            remark: None,
        }
        .build()
        .unwrap()
    }

    fn candidate(remaining: TicketAvailability) -> TicketCandidate {
        TicketCandidate {
            train_no: "G102".to_string(),
            from: station("上海", "SHH"),
            to: station("北京", "BJP"),
            date: NaiveDate::from_ymd_opt(2026, 7, 10).unwrap(),
            seat_type: SeatType::SecondClass,
            remaining,
            waitlist_available: true,
        }
    }

    #[test]
    fn submits_order_when_matching_ticket_is_available() {
        let decision = decide_next_action(
            &task(true, true),
            &[candidate(TicketAvailability::Available { count: Some(2) })],
        );

        assert!(matches!(decision, TaskDecision::SubmitOrder(_)));
    }

    #[test]
    fn strong_waitlist_takes_priority_when_no_ticket_matches() {
        let decision =
            decide_next_action(&task(true, true), &[candidate(TicketAvailability::SoldOut)]);

        assert_eq!(decision, TaskDecision::SubmitStrongWaitlist);
    }

    #[test]
    fn continues_querying_when_waitlist_is_disabled() {
        let decision = decide_next_action(
            &task(false, false),
            &[candidate(TicketAvailability::SoldOut)],
        );

        assert_eq!(decision, TaskDecision::ContinueQuerying);
    }
}
