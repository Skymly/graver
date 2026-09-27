//! In-process text service. It keeps one pipe connection while activated and
//! turns service responses into composition edits. It does not register itself
//! and does not show UI.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use windows::{
    Win32::{
        Foundation::{LPARAM, WPARAM},
        UI::{
            Input::KeyboardAndMouse::{
                GetKeyState, VK_BACK, VK_CONTROL, VK_DOWN, VK_ESCAPE, VK_LEFT, VK_MENU, VK_RETURN,
                VK_RIGHT, VK_SHIFT, VK_SPACE, VK_UP,
            },
            TextServices::{
                ITfComposition, ITfCompositionSink, ITfCompositionSink_Impl, ITfContext,
                ITfContextComposition, ITfEditSession, ITfEditSession_Impl, ITfInsertAtSelection,
                ITfKeyEventSink, ITfKeyEventSink_Impl, ITfKeystrokeMgr, ITfTextInputProcessor,
                ITfTextInputProcessor_Impl, ITfTextInputProcessorEx, ITfTextInputProcessorEx_Impl,
                ITfThreadMgr, TF_ES_READWRITE, TF_ES_SYNC, TF_IAS_NOQUERY,
            },
        },
    },
    core::{BOOL, Interface, Ref, implement},
};
use windows_core::IUnknownImpl;

use crate::{
    client::Session,
    decision::{KeyDecision, KeyModifiers, PreviewKey, decide_key, preview_eaten},
    protocol::{CompositionUpdate, KeyKind, KeyRequest, PIPE_NAME},
};

struct Shared {
    pipe_name: String,
    session: Mutex<Option<Session>>,
    preedit: Mutex<String>,
    client_id: AtomicU32,
    advised: AtomicBool,
    keystroke_mgr: AtomicUsize,
    context: AtomicUsize,
    composition: AtomicUsize,
}

#[implement(
    ITfTextInputProcessor,
    ITfTextInputProcessorEx,
    ITfKeyEventSink,
    ITfCompositionSink
)]
pub(crate) struct TextService {
    shared: Arc<Shared>,
}

impl TextService {
    pub(crate) fn production() -> Self {
        Self::new(PIPE_NAME)
    }

    pub(crate) fn new(pipe_name: impl Into<String>) -> Self {
        Self {
            shared: Arc::new(Shared {
                pipe_name: pipe_name.into(),
                session: Mutex::new(None),
                preedit: Mutex::new(String::new()),
                client_id: AtomicU32::new(0),
                advised: AtomicBool::new(false),
                keystroke_mgr: AtomicUsize::new(0),
                context: AtomicUsize::new(0),
                composition: AtomicUsize::new(0),
            }),
        }
    }

    pub(crate) fn activate_session(&self) {
        let mut slot = lock(&self.shared.session);
        if slot.is_some() {
            return;
        }
        if let Ok(session) = Session::connect(&self.shared.pipe_name) {
            *slot = Some(session);
        }
    }

    pub(crate) fn end_session(&self) {
        let mut slot = lock(&self.shared.session);
        if let Some(session) = slot.as_mut() {
            let _ = session.deactivate();
        }
        *slot = None;
        *lock(&self.shared.preedit) = String::new();
    }

    pub(crate) fn session_open(&self) -> bool {
        lock(&self.shared.session).is_some()
    }

    pub(crate) fn handle_key(&self, key: KeyRequest) -> KeyDecision {
        let modifiers = KeyModifiers {
            ctrl: key.ctrl,
            alt: key.alt,
        };
        let outcome = self.send_key(key);
        let decision = decide_key(modifiers, outcome);
        match &decision {
            KeyDecision::UpdatePreedit(text) => *lock(&self.shared.preedit) = text.clone(),
            KeyDecision::CommitAndEnd(_) | KeyDecision::ReturnToHost => {}
        }
        if matches!(decision, KeyDecision::CommitAndEnd(_)) {
            *lock(&self.shared.preedit) = String::new();
        }
        decision
    }

    fn send_key(&self, key: KeyRequest) -> Result<CompositionUpdate, ()> {
        let mut slot = lock(&self.shared.session);
        let Some(session) = slot.as_mut() else {
            return Err(());
        };
        match session.key(key) {
            Ok(update) => Ok(update),
            Err(_) => {
                *slot = None;
                Err(())
            }
        }
    }
}

impl ITfTextInputProcessor_Impl for TextService_Impl {
    fn Activate(&self, ptim: Ref<ITfThreadMgr>, tid: u32) -> windows_core::Result<()> {
        self.shared.client_id.store(tid, Ordering::SeqCst);
        self.activate_session();
        if let Some(thread_mgr) = ptim.as_ref() {
            // A failed advise must not pop a dialog. Keys simply will not arrive.
            if let Ok(mgr) = thread_mgr.cast::<ITfKeystrokeMgr>() {
                let sink = self.to_interface::<ITfKeyEventSink>();
                // SAFETY: `sink` is this text service. AdviseKeyEventSink AddRefs it.
                if unsafe { mgr.AdviseKeyEventSink(tid, &sink, true) }.is_ok() {
                    store_interface(&self.shared.keystroke_mgr, Some(mgr));
                    self.shared.advised.store(true, Ordering::SeqCst);
                }
            }
        }
        Ok(())
    }

    fn Deactivate(&self) -> windows_core::Result<()> {
        let tid = self.shared.client_id.load(Ordering::SeqCst);
        if self.shared.advised.swap(false, Ordering::SeqCst)
            && let Some(mgr) = load_interface::<ITfKeystrokeMgr>(&self.shared.keystroke_mgr)
        {
            // SAFETY: `mgr` is the keystroke manager stored during Activate.
            unsafe { mgr.UnadviseKeyEventSink(tid).ok() };
        }
        store_interface::<ITfKeystrokeMgr>(&self.shared.keystroke_mgr, None);
        self.end_composition_best_effort();
        self.end_session();
        store_interface::<ITfContext>(&self.shared.context, None);
        store_interface::<ITfComposition>(&self.shared.composition, None);
        Ok(())
    }
}

impl ITfTextInputProcessorEx_Impl for TextService_Impl {
    fn ActivateEx(
        &self,
        ptim: Ref<ITfThreadMgr>,
        tid: u32,
        _dwflags: u32,
    ) -> windows_core::Result<()> {
        ITfTextInputProcessor_Impl::Activate(self, ptim, tid)
    }
}

impl ITfKeyEventSink_Impl for TextService_Impl {
    fn OnSetFocus(&self, _fforeground: BOOL) -> windows_core::Result<()> {
        Ok(())
    }

    fn OnTestKeyDown(
        &self,
        _pic: Ref<ITfContext>,
        wparam: WPARAM,
        _lparam: LPARAM,
    ) -> windows_core::Result<BOOL> {
        let key = current_key(wparam.0 as u32);
        let eaten = preview_eaten(
            self.session_open(),
            lock(&self.shared.preedit).is_empty(),
            preview_kind(&key),
            KeyModifiers {
                ctrl: key.ctrl,
                alt: key.alt,
            },
        );
        Ok(eaten.into())
    }

    fn OnTestKeyUp(
        &self,
        _pic: Ref<ITfContext>,
        _wparam: WPARAM,
        _lparam: LPARAM,
    ) -> windows_core::Result<BOOL> {
        Ok(false.into())
    }

    fn OnKeyDown(
        &self,
        pic: Ref<ITfContext>,
        wparam: WPARAM,
        _lparam: LPARAM,
    ) -> windows_core::Result<BOOL> {
        if let Some(context) = pic.as_ref() {
            store_interface(&self.shared.context, Some(context.clone()));
        }
        let decision = self.handle_key(current_key(wparam.0 as u32));
        let eaten = match decision {
            KeyDecision::ReturnToHost => false,
            other => self.apply_decision(pic.as_ref(), other).is_ok(),
        };
        Ok(eaten.into())
    }

    fn OnKeyUp(
        &self,
        _pic: Ref<ITfContext>,
        _wparam: WPARAM,
        _lparam: LPARAM,
    ) -> windows_core::Result<BOOL> {
        Ok(false.into())
    }

    fn OnPreservedKey(
        &self,
        _pic: Ref<ITfContext>,
        _rguid: *const windows_core::GUID,
    ) -> windows_core::Result<BOOL> {
        Ok(false.into())
    }
}

impl ITfCompositionSink_Impl for TextService_Impl {
    fn OnCompositionTerminated(
        &self,
        _ecwrite: u32,
        _pcomposition: Ref<ITfComposition>,
    ) -> windows_core::Result<()> {
        store_interface::<ITfComposition>(&self.shared.composition, None);
        *lock(&self.shared.preedit) = String::new();
        Ok(())
    }
}

impl TextService_Impl {
    fn apply_decision(
        &self,
        context: Option<&ITfContext>,
        decision: KeyDecision,
    ) -> windows_core::Result<()> {
        let Some(context) = context else {
            return Err(windows::core::Error::from(
                windows::Win32::Foundation::E_POINTER,
            ));
        };
        let edit: ITfEditSession = ApplyEdit {
            context: context.clone(),
            shared: Arc::clone(&self.shared),
            sink: self.to_interface::<ITfCompositionSink>(),
            decision,
        }
        .into();
        let tid = self.shared.client_id.load(Ordering::SeqCst);
        // SAFETY: `edit` stays alive for the synchronous edit session. A failure
        // is returned to the key sink, which then gives the key back. No dialog.
        let hr = unsafe { context.RequestEditSession(tid, &edit, TF_ES_SYNC | TF_ES_READWRITE)? };
        hr.ok()
    }

    fn end_composition_best_effort(&self) {
        let Some(context) = load_interface::<ITfContext>(&self.shared.context) else {
            return;
        };
        let edit: ITfEditSession = ApplyEdit {
            context: context.clone(),
            shared: Arc::clone(&self.shared),
            sink: self.to_interface::<ITfCompositionSink>(),
            decision: KeyDecision::UpdatePreedit(String::new()),
        }
        .into();
        let tid = self.shared.client_id.load(Ordering::SeqCst);
        // SAFETY: best-effort cleanup during Deactivate. Errors are ignored.
        unsafe {
            let _ = context.RequestEditSession(tid, &edit, TF_ES_SYNC | TF_ES_READWRITE);
        }
    }
}

#[implement(ITfEditSession)]
struct ApplyEdit {
    context: ITfContext,
    shared: Arc<Shared>,
    sink: ITfCompositionSink,
    decision: KeyDecision,
}

impl ITfEditSession_Impl for ApplyEdit_Impl {
    fn DoEditSession(&self, ec: u32) -> windows_core::Result<()> {
        match &self.decision {
            KeyDecision::ReturnToHost => Ok(()),
            KeyDecision::UpdatePreedit(text) if text.is_empty() => {
                clear_composition(ec, &self.shared)
            }
            KeyDecision::UpdatePreedit(text) => {
                set_preedit(ec, &self.context, &self.shared, &self.sink, text)
            }
            KeyDecision::CommitAndEnd(text) => commit_text(ec, &self.context, &self.shared, text),
        }
    }
}

fn set_preedit(
    ec: u32,
    context: &ITfContext,
    shared: &Shared,
    sink: &ITfCompositionSink,
    text: &str,
) -> windows_core::Result<()> {
    if let Some(composition) = load_interface::<ITfComposition>(&shared.composition) {
        let range = unsafe { composition.GetRange() }?;
        // SAFETY: `ec` is the edit cookie for this session, and `range` belongs to it.
        unsafe { range.SetText(ec, 0, &utf16(text)) }?;
        return Ok(());
    }
    let insert = context.cast::<ITfInsertAtSelection>()?;
    // SAFETY: insert the preedit at the selection and start a composition over it.
    let range = unsafe { insert.InsertTextAtSelection(ec, TF_IAS_NOQUERY, &utf16(text)) }?;
    let site = context.cast::<ITfContextComposition>()?;
    let composition = unsafe { site.StartComposition(ec, &range, sink) }?;
    store_interface(&shared.composition, Some(composition));
    Ok(())
}

fn commit_text(
    ec: u32,
    context: &ITfContext,
    shared: &Shared,
    text: &str,
) -> windows_core::Result<()> {
    if let Some(composition) = take_interface::<ITfComposition>(&shared.composition) {
        let range = unsafe { composition.GetRange() }?;
        unsafe { range.SetText(ec, 0, &utf16(text)) }?;
        unsafe { composition.EndComposition(ec) }?;
        return Ok(());
    }
    let insert = context.cast::<ITfInsertAtSelection>()?;
    unsafe { insert.InsertTextAtSelection(ec, TF_IAS_NOQUERY, &utf16(text)) }?;
    Ok(())
}

fn clear_composition(ec: u32, shared: &Shared) -> windows_core::Result<()> {
    if let Some(composition) = take_interface::<ITfComposition>(&shared.composition) {
        let range = unsafe { composition.GetRange() }?;
        unsafe { range.SetText(ec, 0, &[]) }?;
        unsafe { composition.EndComposition(ec) }?;
    }
    Ok(())
}

pub(crate) fn map_virtual_key(vk: u32, shift: bool, ctrl: bool, alt: bool) -> KeyRequest {
    let modifiers = (shift, ctrl, alt);
    let kind = match vk {
        value if value == u32::from(VK_BACK.0) => KeyKind::Backspace,
        value if value == u32::from(VK_ESCAPE.0) => KeyKind::Escape,
        value if value == u32::from(VK_SPACE.0) => KeyKind::Space,
        value if value == u32::from(VK_RETURN.0) => KeyKind::Enter,
        value if value == u32::from(VK_LEFT.0) => KeyKind::Left,
        value if value == u32::from(VK_RIGHT.0) => KeyKind::Right,
        value if value == u32::from(VK_UP.0) => KeyKind::Up,
        value if value == u32::from(VK_DOWN.0) => KeyKind::Down,
        value if (0x30..=0x39).contains(&value) => KeyKind::Digit((value - 0x30) as u8),
        value if (0x41..=0x5A).contains(&value) => {
            let ascii = value as u8;
            let ch = if shift {
                char::from(ascii)
            } else {
                char::from(ascii).to_ascii_lowercase()
            };
            KeyKind::Char(ch)
        }
        _ => KeyKind::Other,
    };
    KeyRequest {
        kind,
        shift: modifiers.0,
        ctrl: modifiers.1,
        alt: modifiers.2,
    }
}

fn current_key(vk: u32) -> KeyRequest {
    map_virtual_key(
        vk,
        key_down(VK_SHIFT.0),
        key_down(VK_CONTROL.0),
        key_down(VK_MENU.0),
    )
}

fn key_down(vk: u16) -> bool {
    // SAFETY: GetKeyState only reads the calling thread's keyboard state.
    let state = unsafe { GetKeyState(i32::from(vk)) };
    state < 0
}

fn preview_kind(key: &KeyRequest) -> PreviewKey {
    match key.kind {
        KeyKind::Char(_) => PreviewKey::Char,
        KeyKind::Backspace => PreviewKey::Backspace,
        KeyKind::Escape => PreviewKey::Escape,
        KeyKind::Space => PreviewKey::Space,
        KeyKind::Enter => PreviewKey::Enter,
        _ => PreviewKey::Other,
    }
}

fn utf16(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

fn store_interface<T: Interface>(slot: &AtomicUsize, value: Option<T>) {
    let new_raw = value
        .map(|iface| {
            let raw = iface.as_raw() as usize;
            // SAFETY: the slot takes ownership of this reference. Release happens in a later store.
            core::mem::forget(iface);
            raw
        })
        .unwrap_or(0);
    let old = slot.swap(new_raw, Ordering::SeqCst);
    release_raw::<T>(old);
}

fn load_interface<T: Interface>(slot: &AtomicUsize) -> Option<T> {
    let raw = slot.load(Ordering::SeqCst);
    if raw == 0 {
        return None;
    }
    // SAFETY: `raw` is an AddRef'd interface stored by `store_interface`. Clone it and
    // forget the temporary owner so the slot keeps its reference.
    unsafe {
        let owned = T::from_raw(raw as *mut core::ffi::c_void);
        let clone = owned.clone();
        core::mem::forget(owned);
        Some(clone)
    }
}

fn take_interface<T: Interface>(slot: &AtomicUsize) -> Option<T> {
    let raw = slot.swap(0, Ordering::SeqCst);
    if raw == 0 {
        return None;
    }
    // SAFETY: this consumes the reference previously stored in the slot.
    Some(unsafe { T::from_raw(raw as *mut core::ffi::c_void) })
}

fn release_raw<T: Interface>(raw: usize) {
    if raw != 0 {
        // SAFETY: `raw` was produced by `store_interface` and is no longer in the slot.
        drop(unsafe { T::from_raw(raw as *mut core::ffi::c_void) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::KeyDecision;
    use std::sync::atomic::AtomicU32;
    use std::thread;
    use std::time::{Duration, Instant};
    use windows::Win32::{
        Foundation::{ERROR_PIPE_CONNECTED, HANDLE},
        Storage::FileSystem::PIPE_ACCESS_DUPLEX,
        System::Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE,
            PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
        },
    };
    use windows::core::PCWSTR;

    #[test]
    fn activation_holds_one_pipe_until_deactivate() {
        let name = unique_pipe_name("Hold");
        assert_ne!(name, PIPE_NAME);
        let connects = Arc::new(AtomicU32::new(0));
        let ready = AtomicBool::new(false);
        let connected = Arc::clone(&connects);
        thread::scope(|scope| {
            let server = scope.spawn(|| stub_hold(&name, &ready, &connected));
            wait_ready(&server, &ready);
            let service = TextService::new(&name);
            service.activate_session();
            assert!(service.session_open());
            let start = Instant::now();
            while connects.load(Ordering::SeqCst) == 0 {
                if start.elapsed() > Duration::from_secs(5) {
                    panic!("service did not connect");
                }
                thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(connects.load(Ordering::SeqCst), 1);
            assert!(service.session_open());
            service.end_session();
            assert!(!service.session_open());
            server.join().unwrap();
        });
    }

    #[test]
    fn missing_service_does_not_consume_or_panic() {
        let service = TextService::new(unique_pipe_name("Down"));
        service.activate_session();
        assert!(!service.session_open());
        let decision = service.handle_key(KeyRequest {
            kind: KeyKind::Char('a'),
            shift: false,
            ctrl: false,
            alt: false,
        });
        assert_eq!(decision, KeyDecision::ReturnToHost);
    }

    #[test]
    fn key_is_sent_and_interpreted_without_a_popup() {
        let name = unique_pipe_name("Key");
        let ready = AtomicBool::new(false);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&seen);
        thread::scope(|scope| {
            let server = scope.spawn(|| stub_reply(&name, &ready, &recorded));
            wait_ready(&server, &ready);
            let service = TextService::new(&name);
            service.activate_session();
            let decision = service.handle_key(KeyRequest {
                kind: KeyKind::Char('a'),
                shift: false,
                ctrl: false,
                alt: false,
            });
            assert_eq!(decision, KeyDecision::UpdatePreedit("a".into()));
            let ctrl = service.handle_key(KeyRequest {
                kind: KeyKind::Char('c'),
                shift: false,
                ctrl: true,
                alt: false,
            });
            assert_eq!(ctrl, KeyDecision::ReturnToHost);
            service.end_session();
            server.join().unwrap();
        });
        let bytes = seen.lock().unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(text.contains("\"op\":\"key\""));
        assert!(text.contains("\"ch\":\"a\""));
    }

    #[test]
    fn absent_service_returns_the_key_and_present_service_interprets_commit_and_preedit() {
        let absent = unique_pipe_name("Absent");
        assert_ne!(absent, PIPE_NAME);
        let down = TextService::new(&absent);
        down.activate_session();
        assert!(!down.session_open());
        assert_eq!(
            down.handle_key(KeyRequest {
                kind: KeyKind::Char('a'),
                shift: false,
                ctrl: false,
                alt: false,
            }),
            KeyDecision::ReturnToHost
        );

        let name = unique_pipe_name("Interpret");
        assert_ne!(name, PIPE_NAME);
        let ready = AtomicBool::new(false);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&seen);
        let replies = [
            crate::protocol::encode_frame(
                r#"{"op":"update","v":1,"id":1,"preedit":"你好","candidates":[],"commit":null,"consumed":true}"#
                    .as_bytes(),
            )
            .unwrap(),
            crate::protocol::encode_frame(
                r#"{"op":"update","v":1,"id":2,"preedit":"","candidates":[{"text":"你好"}],"commit":"你好","consumed":true}"#
                    .as_bytes(),
            )
            .unwrap(),
            crate::protocol::encode_frame(
                r#"{"op":"update","v":1,"id":3,"preedit":"","candidates":[],"commit":null,"consumed":false}"#
                    .as_bytes(),
            )
            .unwrap(),
        ];
        thread::scope(|scope| {
            let server = scope.spawn(|| stub_script(&name, &ready, &recorded, &replies));
            wait_ready(&server, &ready);
            let service = TextService::new(&name);
            service.activate_session();
            assert!(service.session_open());
            assert_eq!(
                service.handle_key(KeyRequest {
                    kind: KeyKind::Char('你'),
                    shift: false,
                    ctrl: false,
                    alt: false,
                }),
                KeyDecision::UpdatePreedit("你好".into())
            );
            assert_eq!(
                service.handle_key(KeyRequest {
                    kind: KeyKind::Space,
                    shift: false,
                    ctrl: false,
                    alt: false,
                }),
                KeyDecision::CommitAndEnd("你好".into())
            );
            service.end_session();
            assert!(!service.session_open());
            server.join().unwrap();
        });
        let text = String::from_utf8(seen.lock().unwrap().clone()).unwrap();
        assert!(text.contains("\"ch\":\"你\""));
        assert!(text.contains("\"kind\":\"space\""));
    }

    #[test]
    fn virtual_keys_map_without_touching_the_keyboard() {
        let plain = map_virtual_key(u32::from(b'A'), false, false, false);
        assert_eq!(plain.kind, KeyKind::Char('a'));
        let shifted = map_virtual_key(u32::from(b'A'), true, false, false);
        assert_eq!(shifted.kind, KeyKind::Char('A'));
        let space = map_virtual_key(u32::from(VK_SPACE.0), false, true, false);
        assert_eq!(space.kind, KeyKind::Space);
        assert!(space.ctrl);
        assert!(!preview_eaten(
            true,
            false,
            preview_kind(&space),
            KeyModifiers {
                ctrl: space.ctrl,
                alt: space.alt,
            }
        ));
    }

    fn unique_pipe_name(label: &str) -> String {
        static NEXT: AtomicU32 = AtomicU32::new(1);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        format!(r"\\.\pipe\Graver.Tip.{label}.{}.{n}", std::process::id())
    }

    fn wait_ready(server: &thread::ScopedJoinHandle<()>, ready: &AtomicBool) {
        let start = Instant::now();
        while !ready.load(Ordering::SeqCst) {
            if server.is_finished() {
                panic!("stub exited before listening");
            }
            if start.elapsed() > Duration::from_secs(5) {
                panic!("stub did not listen");
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn stub_hold(name: &str, ready: &AtomicBool, connects: &AtomicU32) {
        let pipe = create_stub(name);
        ready.store(true, Ordering::SeqCst);
        connect_stub(&pipe);
        connects.fetch_add(1, Ordering::SeqCst);
        let mut buf = [0u8; 64];
        let _ = blocking_read(&pipe, &mut buf);
    }

    fn stub_reply(name: &str, ready: &AtomicBool, seen: &Mutex<Vec<u8>>) {
        let pipe = create_stub(name);
        ready.store(true, Ordering::SeqCst);
        connect_stub(&pipe);
        let mut buf = [0u8; 512];
        let n = blocking_read(&pipe, &mut buf).unwrap_or(0);
        seen.lock().unwrap().extend_from_slice(&buf[..n]);
        let reply = crate::protocol::encode_frame(
            br#"{"op":"update","v":1,"id":1,"preedit":"a","candidates":[],"commit":null,"consumed":true}"#,
        )
        .unwrap();
        blocking_write(&pipe, &reply);
        let _ = blocking_read(&pipe, &mut buf);
    }

    fn stub_script(name: &str, ready: &AtomicBool, seen: &Mutex<Vec<u8>>, replies: &[Vec<u8>]) {
        let pipe = create_stub(name);
        ready.store(true, Ordering::SeqCst);
        connect_stub(&pipe);
        let mut buf = [0u8; 1024];
        for reply in replies {
            let n = match blocking_read(&pipe, &mut buf) {
                Ok(n) if n > 0 => n,
                _ => return,
            };
            seen.lock().unwrap().extend_from_slice(&buf[..n]);
            blocking_write(&pipe, reply);
        }
    }

    fn create_stub(name: &str) -> std::os::windows::io::OwnedHandle {
        use std::os::windows::io::{FromRawHandle, OwnedHandle};
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let handle = unsafe {
            CreateNamedPipeW(
                PCWSTR(wide.as_ptr()),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                PIPE_UNLIMITED_INSTANCES,
                64 * 1024,
                64 * 1024,
                0,
                None,
            )
        };
        assert!(!handle.is_invalid());
        unsafe { OwnedHandle::from_raw_handle(handle.0) }
    }

    fn connect_stub(pipe: &std::os::windows::io::OwnedHandle) {
        use std::os::windows::io::AsRawHandle;
        match unsafe { ConnectNamedPipe(HANDLE(pipe.as_raw_handle()), None) } {
            Ok(()) => {}
            Err(err)
                if err.code() == windows::core::HRESULT::from_win32(ERROR_PIPE_CONNECTED.0) => {}
            Err(err) => panic!("stub connect failed: {err}"),
        }
    }

    fn blocking_read(
        pipe: &std::os::windows::io::OwnedHandle,
        buf: &mut [u8],
    ) -> windows_core::Result<usize> {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Storage::FileSystem::ReadFile;
        let mut read = 0u32;
        unsafe {
            ReadFile(
                HANDLE(pipe.as_raw_handle()),
                Some(buf),
                Some(&mut read),
                None,
            )?;
        }
        Ok(read as usize)
    }

    fn blocking_write(pipe: &std::os::windows::io::OwnedHandle, data: &[u8]) {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Storage::FileSystem::WriteFile;
        let mut written = 0u32;
        unsafe {
            WriteFile(
                HANDLE(pipe.as_raw_handle()),
                Some(data),
                Some(&mut written),
                None,
            )
            .expect("stub write");
        }
    }
}
