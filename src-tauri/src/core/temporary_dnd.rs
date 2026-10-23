//! 仅本次运行有效的临时免打扰；读取共享状态，不写账本或创建计时器。
use super::*;
use crate::budget_clock::Reading;
use crate::notification_gate::Gate;
use chrono::{DateTime, Utc};
use std::sync::{Arc, Mutex};

pub(super) type SharedRuntime = Arc<Mutex<Runtime>>;

pub(super) struct Runtime {
    gate: Arc<Gate>,
    lease: Option<Lease>,
    quiet_cutoff: Option<DateTime<Utc>>,
    arrival_fence: Option<i64>,
    last_clock: Option<Reading>,
    rebased_clock: bool,
}

struct Lease {
    deadline: Reading,
    last: Reading,
}

pub(super) struct Command {
    minutes: u64,
    arrival: i64,
}

#[derive(Clone)]
pub(super) struct Filter {
    active: bool,
    quiet_cutoff: Option<DateTime<Utc>>,
    arrival_fence: Option<i64>,
    future_cutoff: Option<DateTime<Utc>>,
}

impl Filter {
    pub(super) fn suppresses(&self, sequence: Option<i64>, created_at: Option<DateTime<Utc>>) -> bool {
        self.active
            || self.arrival_fence.is_some_and(|fence| sequence.is_some_and(|value| value <= fence))
            || self.quiet_cutoff.is_some_and(|cutoff| created_at.is_none_or(|value| value < cutoff))
            || self.future_cutoff.is_some_and(|now| created_at.is_none_or(|value| value > now))
    }
    pub(super) fn suppresses_event(&self, event: &Value, sequence: Option<i64>) -> bool {
        let created = event["createdAt"].as_str().and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&Utc));
        self.suppresses(sequence, created)
    }
}

fn arrival_high(db: &Connection) -> Result<i64> {
    db.query_row("SELECT COALESCE((SELECT seq FROM sqlite_sequence WHERE name='pet_inbox_feed'),0)",
        [], |row| row.get(0)).map_err(err)
}

pub(super) fn prepare(db: &Connection, patch: &Value) -> Result<Option<Command>> {
    for key in ["temporaryDoNotDisturb", "scheduledDoNotDisturb", "effectiveDoNotDisturb"] {
        if patch.get(key).is_some() {
            return Err(format!("{key} 由应用计算，不能直接保存"));
        }
    }
    let Some(value) = patch.get("temporaryDoNotDisturbMinutes") else { return Ok(None); };
    let minutes = value.as_u64().filter(|value| matches!(*value, 0 | 30 | 60))
        .ok_or("临时免打扰时长必须是整数0、30或60分钟")?;
    Ok(Some(Command { minutes, arrival: arrival_high(db)? }))
}

impl Runtime {
    pub(super) fn shared(gate: Arc<Gate>) -> SharedRuntime {
        Arc::new(Mutex::new(Self { gate, lease: None, quiet_cutoff: None, arrival_fence: None,
            last_clock: None, rebased_clock: false }))
    }

    fn advance_cutoff(&mut self, at: DateTime<Utc>) {
        self.quiet_cutoff = Some(self.quiet_cutoff.map_or(at, |previous| previous.max(at)));
    }

    fn advance_arrival(&mut self, high: i64) {
        self.arrival_fence = Some(self.arrival_fence.map_or(high, |previous| previous.max(high)));
    }

    // 只能在偏好保存成功之后调用；运行态切换不持锁回调Core或写数据库。
    pub(super) fn apply(&mut self, command: Command) -> Result<()> {
        let at = crate::budget_clock::now();
        let deadline = if command.minutes == 0 { None } else {
            Some(at.checked_deadline(command.minutes * 60_000).ok_or("临时免打扰截止时间超出范围")?)
        };
        if self.quiet_cutoff.is_some() && self.last_clock.is_some_and(|last| at.utc < last.utc) {
            self.quiet_cutoff = Some(at.utc);
            self.advance_arrival(command.arrival);
            self.rebased_clock = true;
        }
        if deadline.is_some() || self.lease.is_some() {
            self.advance_cutoff(at.utc);
            self.advance_arrival(command.arrival);
        }
        self.lease = deadline.map(|deadline| Lease { deadline, last: at });
        if self.quiet_cutoff.is_some() { self.last_clock = Some(at); }
        self.gate.invalidate();
        Ok(())
    }

    fn observe(&mut self, db: &Connection, at: Reading) -> Result<()> {
        if self.quiet_cutoff.is_some() && self.last_clock.is_some_and(|last| at.utc < last.utc) {
            // 新墙钟代次用到达序号保护旧积压，不能让旧的未来截止压住新事件。
            let high = arrival_high(db)?;
            self.quiet_cutoff = Some(at.utc);
            self.advance_arrival(high);
            self.lease = None;
            self.last_clock = Some(at);
            self.rebased_clock = true;
            self.gate.invalidate();
            return Ok(());
        }
        let Some(lease) = self.lease.as_ref() else {
            if self.quiet_cutoff.is_some() { self.last_clock = Some(at); }
            return Ok(());
        };
        let wall_elapsed = (at.utc - lease.last.utc).num_milliseconds();
        let mono_elapsed = at.mono_ms.checked_sub(lease.last.mono_ms);
        let clock_ok = wall_elapsed >= 0 && mono_elapsed.is_some_and(|elapsed|
            (i128::from(wall_elapsed) - i128::from(elapsed)).abs() <= 2_000);
        if !clock_ok || at.utc >= lease.deadline.utc || at.mono_ms >= lease.deadline.mono_ms {
            // 延迟观察只锁存真实截止，不把到期后新到来的记录并入静默水位。
            let cutoff = at.utc.min(lease.deadline.utc).max(lease.last.utc.min(lease.deadline.utc));
            self.advance_cutoff(cutoff);
            self.lease = None;
            self.last_clock = Some(at);
            self.gate.invalidate();
            return Ok(());
        }
        let high = arrival_high(db)?;
        self.advance_cutoff(at.utc);
        self.advance_arrival(high);
        self.lease.as_mut().unwrap().last = at;
        self.last_clock = Some(at);
        Ok(())
    }

    fn filter(&self, at: Reading) -> Filter {
        Filter { active: self.lease.is_some(), quiet_cutoff: self.quiet_cutoff, arrival_fence: self.arrival_fence,
            future_cutoff: self.rebased_clock.then_some(at.utc) }
    }

    fn projection(&self, at: Reading) -> Value {
        match self.lease.as_ref() {
            Some(lease) => {
                let wall = u64::try_from((lease.deadline.utc - at.utc).num_milliseconds()).unwrap_or(0).max(1);
                let remaining = lease.deadline.mono_ms.saturating_sub(at.mono_ms).min(wall);
                json!({"active":true,"until":lease.deadline.utc.to_rfc3339_opts(chrono::SecondsFormat::Millis,true),
                    "remainingMs":remaining,"deadlineMonoMs":lease.deadline.mono_ms})
            }
            None => json!({"active":false,"until":null,"remainingMs":0,"deadlineMonoMs":null}),
        }
    }
}

pub(super) fn read(shared: &SharedRuntime, db: &Connection) -> Result<(Value, Filter)> {
    let mut runtime = shared.lock().map_err(|_| "临时免打扰状态暂不可用")?;
    // 先拿运行态锁再成对采样，避免较早发起的读取在较晚状态之后伪装成时钟回退。
    let at = crate::budget_clock::now();
    runtime.observe(db, at)?;
    Ok((runtime.projection(at), runtime.filter(at)))
}

#[cfg(test)]
#[path = "temporary_dnd_tests.rs"]
mod tests;
