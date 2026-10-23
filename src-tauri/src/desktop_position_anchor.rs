//! 冻结用户位置和延迟写入的纯状态；不读取窗口、设置或业务数据。
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Ticket(u64);

#[derive(Clone, Copy)]
enum Hold {
    Preparing(Duration),
    Active,
}

pub(super) enum Selection<T> {
    Wait,
    Frozen(Ticket, T),
    Capture(Ticket),
}

pub(super) struct Owner<T> {
    generation: u64,
    frozen: Option<T>,
    hold: Option<Hold>,
}

impl<T> Default for Owner<T> {
    fn default() -> Self {
        Self {
            generation: 0,
            frozen: None,
            hold: None,
        }
    }
}

impl<T: Clone> Owner<T> {
    /// 宿主在 settings_update 序列内调用，与最终 writer 互斥；旧 job 立即失效。
    pub fn prepare(&mut self, value: T, now: Duration) -> Result<Ticket, String> {
        self.generation = self
            .generation
            .checked_add(1)
            .filter(|next| *next != u64::MAX)
            .ok_or("位置保存代次已耗尽，请重启应用")?;
        self.frozen = Some(value);
        self.hold = Some(Hold::Preparing(now));
        Ok(Ticket(self.generation))
    }

    pub fn complete(&mut self, ticket: Ticket, started: bool) {
        if ticket.0 == self.generation {
            if started {
                self.hold = Some(Hold::Active);
            } else if matches!(self.hold, Some(Hold::Preparing(_))) {
                self.hold = None;
            }
        }
    }

    pub fn version(&self) -> Ticket {
        Ticket(self.generation)
    }

    /// 调用者在询问 physics owner 前取得 ticket，迟到的空闲结果不能释放新冻结点。
    pub fn refresh(&mut self, ticket: Ticket, active: bool, now: Duration) {
        if ticket.0 != self.generation {
            return;
        }
        match self.hold {
            Some(Hold::Active) if !active => self.hold = None,
            Some(Hold::Preparing(at))
                if !active
                    && now
                        .checked_sub(at)
                        .is_some_and(|age| age >= Duration::from_secs(1)) =>
            {
                self.hold = None
            }
            _ => {}
        }
    }

    pub fn select(&self) -> Selection<T> {
        if self.hold.is_some() {
            return Selection::Wait;
        }
        let ticket = self.version();
        match &self.frozen {
            Some(value) => Selection::Frozen(ticket, value.clone()),
            None => Selection::Capture(ticket),
        }
    }

    pub fn writable(&self, ticket: Ticket) -> bool {
        ticket.0 == self.generation && self.hold.is_none() && self.generation != u64::MAX
    }

    pub fn finish(&mut self, ticket: Ticket) {
        if self.writable(ticket) {
            self.frozen = None;
        }
    }

    /// 显式恢复/显示器恢复另有新位置，旧冻结位置不能覆盖该恢复。
    pub fn invalidate(&mut self) {
        self.generation = self.generation.checked_add(1).unwrap_or(u64::MAX);
        self.frozen = None;
        self.hold = None;
    }

    /// 与刚冻结位置相同的迟到手动 Moved 只是确认，不能清掉新 release 的 hold。
    pub fn invalidate_unless_frozen_matches(&mut self, matches: impl FnOnce(&T) -> bool) -> bool {
        if self.frozen.as_ref().is_some_and(matches) {
            return false;
        }
        self.invalidate();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn late_legacy_job_cannot_write_after_new_release_and_running_motion_never_returns_auto_end() {
        let mut owner = Owner::<(i32, i32)>::default();
        let Selection::Capture(old_job) = owner.select() else {
            panic!()
        };
        let release = owner.prepare((120, 240), Duration::ZERO).unwrap();
        assert!(!owner.writable(old_job));
        assert!(matches!(owner.select(), Selection::Wait));
        owner.complete(release, true);
        owner.refresh(old_job, false, Duration::from_secs(2));
        assert!(matches!(owner.select(), Selection::Wait));
        owner.refresh(release, false, Duration::from_secs(2));
        let Selection::Frozen(job, point) = owner.select() else {
            panic!()
        };
        assert_eq!(point, (120, 240));
        assert!(owner.writable(job));
        owner.finish(old_job);
        assert!(matches!(owner.select(), Selection::Frozen(_, _)));
        owner.finish(job);
        assert!(matches!(owner.select(), Selection::Capture(_)));
    }
    #[test]
    fn rejected_motion_releases_hold_but_keeps_release_and_old_completion_cannot_release_new_hold()
    {
        let mut owner = Owner::default();
        let first = owner.prepare("first release", Duration::ZERO).unwrap();
        owner.complete(first, false);
        assert!(matches!(
            owner.select(),
            Selection::Frozen(_, "first release")
        ));
        let second = owner
            .prepare("new explicit position", Duration::from_secs(1))
            .unwrap();
        owner.complete(first, false);
        owner.refresh(first, false, Duration::from_secs(3));
        assert!(matches!(owner.select(), Selection::Wait));
        owner.complete(second, true);
        owner.invalidate();
        assert!(!owner.writable(second));
        assert!(matches!(owner.select(), Selection::Capture(_)));
    }
    #[test]
    fn interrupted_preparation_is_finite_and_version_exhaustion_fails_closed() {
        let mut owner = Owner::default();
        let ticket = owner.prepare(7, Duration::ZERO).unwrap();
        owner.refresh(ticket, false, Duration::from_millis(999));
        assert!(matches!(owner.select(), Selection::Wait));
        owner.refresh(ticket, false, Duration::from_secs(1));
        assert!(matches!(owner.select(), Selection::Frozen(_, 7)));
        owner.complete(ticket, true);
        assert!(
            matches!(owner.select(), Selection::Wait),
            "迟到的合法启动仍须重新占住 writer hold"
        );
        owner.refresh(ticket, false, Duration::from_secs(2));
        assert!(matches!(owner.select(), Selection::Frozen(_, 7)));
        owner.generation = u64::MAX;
        assert!(!owner.writable(owner.version()));
        assert!(owner.prepare(8, Duration::from_secs(2)).is_err());
    }
    #[test]
    fn late_manual_move_at_the_release_point_preserves_new_anchor_and_active_hold() {
        let mut owner = Owner::default();
        let ticket = owner.prepare((120, 240), Duration::ZERO).unwrap();
        owner.complete(ticket, true);
        assert!(!owner.invalidate_unless_frozen_matches(|point| *point == (120, 240)));
        assert!(matches!(owner.select(), Selection::Wait));
        assert_eq!(owner.version(), ticket);
        owner.refresh(ticket, false, Duration::from_secs(1));
        assert!(matches!(owner.select(), Selection::Frozen(_, (120, 240))));
        assert!(owner.invalidate_unless_frozen_matches(|point| *point == (121, 240)));
        assert!(!owner.writable(ticket));
        assert!(matches!(owner.select(), Selection::Capture(_)));
    }
}
