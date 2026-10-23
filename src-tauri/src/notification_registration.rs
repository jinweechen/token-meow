//! One bounded, silent transport registration. This never creates a domain event.
use serde::Serialize;

const NOT_REGISTERED: i32 = 0x80070490u32 as i32;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Failure {
    pub(crate) stage: &'static str,
    pub(crate) hresult: i32,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Outcome {
    pub(crate) initial_setting: Option<i32>,
    pub(crate) final_setting: Option<i32>,
    pub(crate) bootstrap_submitted: bool,
    pub(crate) cleanup_error: Option<Failure>,
    pub(crate) error: Option<Failure>,
}

pub(crate) trait Transport {
    type Prepared;
    fn setting(&mut self) -> Result<i32, Failure>;
    fn prepare(&mut self) -> Result<Self::Prepared, Failure>;
    /// false means the final permit check cancelled before any OS submission.
    fn submit(&mut self, prepared: Self::Prepared) -> Result<bool, Failure>;
    fn cleanup(&mut self) -> Result<(), Failure>;
}

#[derive(Default)]
pub(crate) struct Registration {
    outcome: Option<Outcome>,
}

pub(crate) enum Step {
    Deferred,
    Finished(Outcome),
    AlreadyFinished,
}

impl Registration {
    /// No retries after an OS submission (including ambiguous failure). A host
    /// denial before submission remains pending and performs no OS write.
    pub(crate) fn advance<T: Transport>(
        &mut self,
        connect: impl FnOnce() -> Result<T, Failure>,
        allowed: impl Fn() -> bool,
    ) -> Step {
        if self.outcome.is_some() {
            return Step::AlreadyFinished;
        }
        if !allowed() {
            return Step::Deferred;
        }
        let mut outcome = Outcome {
            initial_setting: None,
            final_setting: None,
            bootstrap_submitted: false,
            cleanup_error: None,
            error: None,
        };
        let result = (|| {
            let mut transport = connect()?;
            match transport.setting() {
                Ok(setting) => {
                    // Includes explicitly disabled settings; never enable them.
                    outcome.initial_setting = Some(setting);
                    outcome.final_setting = Some(setting);
                    return Ok(true);
                }
                Err(error) if error.hresult == NOT_REGISTERED => {}
                Err(error) => return Err(error),
            }
            let prepared = transport.prepare()?;
            if !allowed() {
                return Ok(false);
            }
            let submitted = transport.submit(prepared);
            if matches!(submitted, Ok(false)) {
                return Ok(false);
            }
            outcome.bootstrap_submitted = matches!(submitted, Ok(true));
            // The exact reserved tag/group is ours even if Show returned an
            // ambiguous error. Expiration remains a fallback for cleanup error.
            outcome.cleanup_error = transport.cleanup().err();
            submitted?;
            outcome.final_setting = Some(transport.setting()?);
            Ok(true)
        })();
        match result {
            Ok(false) => return Step::Deferred,
            Ok(true) => {}
            Err(error) => outcome.error = Some(error),
        }
        self.outcome = Some(outcome.clone());
        Step::Finished(outcome)
    }
}

#[cfg(all(windows, feature = "desktop"))]
pub(crate) mod windows_transport {
    use super::*;
    use crate::notification_gate::{windows_deadline_ticks, NotificationPermit};
    use windows::{
        core::{Interface, HSTRING},
        Data::Xml::Dom::XmlDocument,
        Foundation::{DateTime, IReference, PropertyValue},
        Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED},
        UI::Notifications::{ToastNotification, ToastNotificationManager, ToastNotifier},
    };

    const TAG: &str = "transport-init";
    const GROUP: &str = "token-meow-init";

    fn failure(stage: &'static str, error: windows::core::Error) -> Failure {
        Failure {
            stage,
            hresult: error.code().0,
        }
    }

    struct Apartment(bool);
    impl Drop for Apartment {
        fn drop(&mut self) {
            if self.0 {
                unsafe { RoUninitialize() };
            }
        }
    }

    pub(crate) struct WindowsTransport {
        // COM objects must be released before their apartment.
        notifier: ToastNotifier,
        _apartment: Apartment,
        app_id: HSTRING,
        permit: NotificationPermit,
    }

    impl WindowsTransport {
        pub(crate) fn connect(app_id: &str, permit: NotificationPermit) -> Result<Self, Failure> {
            let initialized = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
            if let Err(error) = &initialized {
                if error.code().0 != 0x80010106u32 as i32 {
                    return Err(failure("initialize", error.clone()));
                }
            }
            let apartment = Apartment(initialized.is_ok());
            let app_id = HSTRING::from(app_id);
            let notifier = ToastNotificationManager::CreateToastNotifierWithId(&app_id)
                .map_err(|error| failure("create_notifier", error))?;
            Ok(Self {
                notifier,
                _apartment: apartment,
                app_id,
                permit,
            })
        }
    }

    impl Transport for WindowsTransport {
        type Prepared = ToastNotification;

        fn setting(&mut self) -> Result<i32, Failure> {
            self.notifier
                .Setting()
                .map(|value| value.0)
                .map_err(|error| failure("read_setting", error))
        }

        fn prepare(&mut self) -> Result<Self::Prepared, Failure> {
            let xml =
                XmlDocument::new().map_err(|error| failure("create_registration_xml", error))?;
            xml.LoadXml(&HSTRING::from(
                "<toast><visual><binding template=\"ToastGeneric\"><text>Token 喵</text></binding></visual><audio silent=\"true\"/></toast>",
            )).map_err(|error| failure("load_registration_xml", error))?;
            let toast = ToastNotification::CreateToastNotification(&xml)
                .map_err(|error| failure("create_registration_toast", error))?;
            toast
                .SetTag(&HSTRING::from(TAG))
                .map_err(|error| failure("set_registration_tag", error))?;
            toast
                .SetGroup(&HSTRING::from(GROUP))
                .map_err(|error| failure("set_registration_group", error))?;
            toast
                .SetSuppressPopup(true)
                .map_err(|error| failure("suppress_registration_popup", error))?;
            let ticks = windows_deadline_ticks(self.permit.expires_utc()).ok_or(Failure {
                stage: "registration_deadline",
                hresult: 0x80070057u32 as i32,
            })?;
            let expiration: IReference<DateTime> = PropertyValue::CreateDateTime(DateTime {
                UniversalTime: ticks,
            })
            .and_then(|value| value.cast())
            .map_err(|error| failure("registration_expiration", error))?;
            toast
                .SetExpirationTime(&expiration)
                .map_err(|error| failure("set_registration_expiration", error))?;
            Ok(toast)
        }

        fn submit(&mut self, prepared: Self::Prepared) -> Result<bool, Failure> {
            if !self.permit.is_current() {
                return Ok(false);
            }
            self.notifier
                .Show(&prepared)
                .map(|_| true)
                .map_err(|error| failure("register_silently", error))
        }

        fn cleanup(&mut self) -> Result<(), Failure> {
            ToastNotificationManager::History()
                .and_then(|history| {
                    history.RemoveGroupedTagWithId(
                        &HSTRING::from(TAG),
                        &HSTRING::from(GROUP),
                        &self.app_id,
                    )
                })
                .map_err(|error| failure("remove_registration_toast", error))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, collections::VecDeque, rc::Rc};

    #[derive(Default)]
    struct Calls {
        reads: Cell<usize>,
        prepares: Cell<usize>,
        submits: Cell<usize>,
        cleanups: Cell<usize>,
    }
    struct Fake {
        calls: Rc<Calls>,
        settings: VecDeque<Result<i32, Failure>>,
        submit_result: Result<bool, Failure>,
        cleanup_result: Result<(), Failure>,
    }
    fn failure() -> Failure {
        Failure {
            stage: "test",
            hresult: -1,
        }
    }
    fn cold(calls: &Rc<Calls>) -> Fake {
        Fake {
            calls: calls.clone(),
            settings: [
                Err(Failure {
                    stage: "read_setting",
                    hresult: NOT_REGISTERED,
                }),
                Ok(0),
            ]
            .into(),
            submit_result: Ok(true),
            cleanup_result: Ok(()),
        }
    }
    impl Transport for Fake {
        type Prepared = ();
        fn setting(&mut self) -> Result<i32, Failure> {
            self.calls.reads.set(self.calls.reads.get() + 1);
            self.settings
                .pop_front()
                .expect("unexpected additional read")
        }
        fn prepare(&mut self) -> Result<(), Failure> {
            self.calls.prepares.set(self.calls.prepares.get() + 1);
            Ok(())
        }
        fn submit(&mut self, _: ()) -> Result<bool, Failure> {
            self.calls.submits.set(self.calls.submits.get() + 1);
            self.submit_result.clone()
        }
        fn cleanup(&mut self) -> Result<(), Failure> {
            self.calls.cleanups.set(self.calls.cleanups.get() + 1);
            self.cleanup_result.clone()
        }
    }

    #[test]
    fn cold_registration_rechecks_os_setting_and_cannot_replay() {
        let calls = Rc::new(Calls::default());
        let mut state = Registration::default();
        let Step::Finished(outcome) = state.advance(|| Ok(cold(&calls)), || true) else {
            panic!("registration not attempted")
        };
        assert_eq!(
            (outcome.initial_setting, outcome.final_setting),
            (None, Some(0))
        );
        assert!(
            outcome.bootstrap_submitted
                && outcome.error.is_none()
                && outcome.cleanup_error.is_none()
        );
        assert_eq!(
            (
                calls.reads.get(),
                calls.prepares.get(),
                calls.submits.get(),
                calls.cleanups.get()
            ),
            (2, 1, 1, 1)
        );
        assert!(matches!(
            state.advance::<Fake>(|| panic!("no reconnect"), || panic!("no additional work")),
            Step::AlreadyFinished
        ));
    }

    #[test]
    fn known_enabled_and_disabled_settings_never_send_a_registration_toast() {
        for setting in 0..=4 {
            let calls = Rc::new(Calls::default());
            let mut transport = cold(&calls);
            transport.settings = [Ok(setting)].into();
            let Step::Finished(outcome) =
                Registration::default().advance(|| Ok(transport), || true)
            else {
                panic!()
            };
            assert_eq!(outcome.final_setting, Some(setting));
            assert!(!outcome.bootstrap_submitted && outcome.error.is_none());
            assert_eq!(
                (
                    calls.prepares.get(),
                    calls.submits.get(),
                    calls.cleanups.get()
                ),
                (0, 0, 0)
            );
        }
    }

    #[test]
    fn unavailable_host_or_host_change_during_preparation_performs_no_os_write() {
        let mut state = Registration::default();
        assert!(matches!(
            state.advance::<Fake>(|| panic!("denial must not connect"), || false),
            Step::Deferred
        ));
        let calls = Rc::new(Calls::default());
        assert!(matches!(
            state.advance(|| Ok(cold(&calls)), || calls.prepares.get() == 0),
            Step::Deferred
        ));
        assert_eq!((calls.submits.get(), calls.cleanups.get()), (0, 0));
        assert!(matches!(
            state.advance(|| Ok(cold(&calls)), || true),
            Step::Finished(_)
        ));
        assert_eq!(calls.submits.get(), 1);
    }

    #[test]
    fn unrelated_com_failure_and_failed_connection_do_not_attempt_bootstrap() {
        let calls = Rc::new(Calls::default());
        let mut transport = cold(&calls);
        transport.settings = [Err(failure())].into();
        let Step::Finished(outcome) =
            Registration::default().advance::<Fake>(|| Err(failure()), || true)
        else {
            panic!()
        };
        assert!(outcome.error.is_some() && !outcome.bootstrap_submitted);
        let Step::Finished(outcome) = Registration::default().advance(|| Ok(transport), || true)
        else {
            panic!()
        };
        assert!(outcome.error.is_some());
        assert_eq!(calls.submits.get(), 0);
    }

    #[test]
    fn failed_submission_cleans_only_its_entry_and_cannot_retry() {
        let calls = Rc::new(Calls::default());
        let mut transport = cold(&calls);
        transport.submit_result = Err(failure());
        let mut state = Registration::default();
        let Step::Finished(outcome) = state.advance(|| Ok(transport), || true) else {
            panic!()
        };
        assert!(!outcome.bootstrap_submitted && outcome.error.is_some());
        assert_eq!(
            (calls.submits.get(), calls.cleanups.get(), calls.reads.get()),
            (1, 1, 1)
        );
        assert!(matches!(
            state.advance::<Fake>(|| panic!("no OS retry"), || true),
            Step::AlreadyFinished
        ));
    }

    #[test]
    fn final_permit_cancellation_before_show_can_recover_without_cleanup_or_replay() {
        let calls = Rc::new(Calls::default());
        let mut cancelled = cold(&calls);
        cancelled.submit_result = Ok(false);
        let mut state = Registration::default();
        assert!(matches!(
            state.advance(|| Ok(cancelled), || true),
            Step::Deferred
        ));
        assert_eq!((calls.reads.get(), calls.cleanups.get()), (1, 0));
        let Step::Finished(outcome) = state.advance(|| Ok(cold(&calls)), || true) else {
            panic!()
        };
        assert!(outcome.bootstrap_submitted && outcome.error.is_none());
        assert_eq!(calls.cleanups.get(), 1);
        assert!(matches!(
            state.advance::<Fake>(|| panic!("cannot issue after actual success"), || true),
            Step::AlreadyFinished
        ));
    }

    #[test]
    fn cleanup_failure_does_not_fabricate_readiness_or_resend() {
        let calls = Rc::new(Calls::default());
        let mut transport = cold(&calls);
        transport.cleanup_result = Err(failure());
        transport.settings.pop_back();
        transport.settings.push_back(Err(failure()));
        let mut state = Registration::default();
        let Step::Finished(outcome) = state.advance(|| Ok(transport), || true) else {
            panic!()
        };
        assert!(
            outcome.bootstrap_submitted
                && outcome.cleanup_error.is_some()
                && outcome.error.is_some()
        );
        assert_eq!(outcome.final_setting, None);
        assert!(matches!(
            state.advance::<Fake>(|| panic!("no retry"), || true),
            Step::AlreadyFinished
        ));
        assert_eq!(calls.submits.get(), 1);
    }

    #[cfg(all(windows, feature = "desktop", feature = "subscription-native-audit"))]
    #[test]
    #[ignore = "explicit Native cold-identity audit; one owned expiring silent transport notification"]
    fn actual_windows_cold_identity_registers_without_a_manual_kick_and_removes_its_tag() {
        use super::windows_transport::WindowsTransport;
        use crate::{
            budget_clock,
            notification_gate::{Gate, NotificationPermit},
        };
        use std::sync::Arc;
        use windows::{core::HSTRING, UI::Notifications::ToastNotificationManager};

        // This isolated transport fixture does not qualify desktop host lifecycle
        // or create a Core account, plan, ledger fact, or domain delivery permit.
        let app_id = format!("local.tokenmeow.audit.{}", uuid::Uuid::new_v4().simple());
        let gate = Arc::new(Gate::new());
        gate.publish(true, false);
        let at = budget_clock::now();
        let permit = NotificationPermit::new(
            gate.clone(),
            gate.snapshot().unwrap(),
            at.utc + chrono::Duration::seconds(15),
            at.mono_ms + 15_000,
        );
        let mut probe = WindowsTransport::connect(&app_id, permit.clone()).unwrap();
        let before = probe
            .setting()
            .expect_err("a unique identity must be genuinely cold");
        assert_eq!(before.hresult, NOT_REGISTERED);
        drop(probe);
        let mut state = Registration::default();
        let Step::Finished(outcome) = state.advance(
            || WindowsTransport::connect(&app_id, permit.clone()),
            || true,
        ) else {
            panic!("no actual registration")
        };
        assert!(outcome.bootstrap_submitted, "{outcome:?}");
        assert_eq!(outcome.final_setting, Some(0), "{outcome:?}");
        assert!(
            outcome.error.is_none() && outcome.cleanup_error.is_none(),
            "{outcome:?}"
        );
        let mut probe = WindowsTransport::connect(&app_id, permit).unwrap();
        assert_eq!(probe.setting().unwrap(), 0);
        let history = ToastNotificationManager::History()
            .unwrap()
            .GetHistoryWithId(&HSTRING::from(&app_id))
            .unwrap();
        for index in 0..history.Size().unwrap() {
            let entry = history.GetAt(index).unwrap();
            assert!(
                entry.Tag().unwrap() != "transport-init"
                    || entry.Group().unwrap() != "token-meow-init"
            );
        }
        assert!(matches!(
            state.advance::<WindowsTransport>(
                || panic!("registered state cannot issue again"),
                || true
            ),
            Step::AlreadyFinished
        ));
        println!(
            "{}",
            serde_json::json!({"kind":"actual-windows-cold-transport-registration","appId":app_id,"beforeHresult":"0x80070490","outcome":outcome,"reservedTagRemaining":false,"manualKick":false,"domainFactsCreated":0,"fullG3Qualified":false})
        );
    }
}
