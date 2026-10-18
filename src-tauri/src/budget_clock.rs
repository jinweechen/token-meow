//! A paired wall/monotonic reading for short-lived budget evidence and permits.
//! Wall time selects the budget period; monotonic time bounds elapsed time even
//! when the operating-system clock is adjusted.

use chrono::{DateTime, Utc};
use std::sync::OnceLock;
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Reading {
    pub utc: DateTime<Utc>,
    pub mono_ms: u64,
}

impl Reading {
    pub(crate) fn checked_deadline(self, lifetime_ms: u64) -> Option<Self> {
        let wall_ms = i64::try_from(lifetime_ms).ok()?;
        Some(Self {
            utc: self
                .utc
                .checked_add_signed(chrono::Duration::try_milliseconds(wall_ms)?)?,
            mono_ms: self.mono_ms.checked_add(lifetime_ms)?,
        })
    }
}

pub(crate) fn now() -> Reading {
    #[cfg(test)]
    if let Some(reading) = test_support::current() {
        return reading;
    }

    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    Reading {
        utc: Utc::now(),
        // Saturation makes checked deadlines fail closed on overflow.
        mono_ms: u64::try_from(ORIGIN.get_or_init(Instant::now).elapsed().as_millis())
            .unwrap_or(u64::MAX),
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::Reading;
    use std::cell::Cell;
    use std::marker::PhantomData;
    use std::rc::Rc;

    thread_local! {
        static OVERRIDE: Cell<Option<Reading>> = const { Cell::new(None) };
    }

    pub(super) fn current() -> Option<Reading> {
        OVERRIDE.with(Cell::get)
    }

    /// Scoped to the current thread; nested clocks restore the prior reading.
    pub(crate) struct TestClock {
        previous: Option<Reading>,
        _not_send: PhantomData<Rc<()>>,
    }

    impl TestClock {
        pub(crate) fn new(reading: Reading) -> Self {
            Self {
                previous: OVERRIDE.with(|slot| slot.replace(Some(reading))),
                _not_send: PhantomData,
            }
        }

        pub(crate) fn set(&self, reading: Reading) {
            OVERRIDE.with(|slot| slot.set(Some(reading)));
        }
    }

    impl Drop for TestClock {
        fn drop(&mut self) {
            OVERRIDE.with(|slot| slot.set(self.previous));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{now, test_support::TestClock, Reading};
    use chrono::{TimeZone, Utc};

    fn reading(mono_ms: u64) -> Reading {
        Reading {
            utc: Utc.timestamp_opt(1_800_000_000, 0).unwrap(),
            mono_ms,
        }
    }

    #[test]
    fn nested_overrides_restore_and_do_not_leak_to_other_threads() {
        let outer = TestClock::new(reading(12));
        {
            let inner = TestClock::new(reading(34));
            assert_eq!(now(), reading(34));
            inner.set(reading(56));
            assert_eq!(now(), reading(56));
            assert!(
                std::thread::spawn(|| super::test_support::current().is_none())
                    .join()
                    .unwrap()
            );
        }
        assert_eq!(now(), reading(12));
        outer.set(reading(78));
        assert_eq!(now(), reading(78));
        drop(outer);
        assert!(super::test_support::current().is_none());
    }

    #[test]
    fn deadline_requires_both_clocks_to_be_representable() {
        let start = reading(12);
        let deadline = start.checked_deadline(1_000).unwrap();
        assert_eq!(deadline.mono_ms, 1_012);
        assert_eq!((deadline.utc - start.utc).num_milliseconds(), 1_000);
        assert!(reading(u64::MAX).checked_deadline(1).is_none());
        assert!(start.checked_deadline(u64::MAX).is_none());
        assert!(Reading {
            utc: chrono::DateTime::<Utc>::MAX_UTC,
            mono_ms: 0
        }
        .checked_deadline(1)
        .is_none());
    }
}
