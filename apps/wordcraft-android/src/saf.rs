use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use jni::objects::{JObject, JObjectArray, JString, JValue};
use jni::sys::{jint, jobject};
use jni::{Env, EnvUnowned, JavaVM, jni_sig, jni_str};
use winit::platform::android::activity::AndroidApp;
use wordcraft_ui_egui::{FileDialogAnswer, FileDialogRequest, Services};

const RESULT_OK: i32 = -1;
const REQUEST_OPEN_BASE: i32 = 0x5A00;
const REQUEST_SAVE_BASE: i32 = 0x5A80;
const REQUEST_SEQUENCE_MASK: usize = 0x7F;
const MAX_DOCUMENT_BYTES: u64 = 2 * 1024 * 1024 * 1024;

static SAF_STATE: OnceLock<Arc<SafState>> = OnceLock::new();
static PANIC_TRACE_HOOK: OnceLock<()> = OnceLock::new();
static NEXT_REQUEST: AtomicU64 = AtomicU64::new(0);
static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(0);

fn diagnostic_trace(message: &str) {
    let Some(state) = SAF_STATE.get() else { return };
    let Some(directory) = state.registry_path.parent() else { return };
    let path = directory.join("saf-diagnostic.log");
    let timestamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|duration| duration.as_millis()).unwrap_or_default();
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{timestamp} [RUST] {message}");
        let _ = file.sync_all();
    }
}

struct PendingPicker {
    request_code: i32,
    answer: Sender<FileDialogAnswer>,
}

struct SafState {
    registry_path: PathBuf,
    uris: Mutex<HashMap<String, String>>,
    pending: Mutex<Option<PendingPicker>>,
}

impl SafState {
    fn load(root: PathBuf) -> Self {
        let registry_path = root.join("uri-registry.json");
        let uris = match fs::read(&registry_path) {
            Ok(contents) => match serde_json::from_slice::<HashMap<String, String>>(&contents) {
                Ok(uris) => uris,
                Err(error) => {
                    log::warn!("could not read the Android SAF URI registry: {error}");
                    HashMap::new()
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
            Err(error) => {
                log::warn!("could not open the Android SAF URI registry: {error}");
                HashMap::new()
            }
        };
        Self { registry_path, uris: Mutex::new(uris), pending: Mutex::new(None) }
    }

    fn show_picker(self: &Arc<Self>, app: &AndroidApp, request: FileDialogRequest, answer: Sender<FileDialogAnswer>) {
        let is_save = matches!(&request, FileDialogRequest::Save { .. });
        diagnostic_trace(&format!("PICKER request received save={is_save}"));
        let sequence = (NEXT_REQUEST.fetch_add(1, Ordering::Relaxed) as usize) & REQUEST_SEQUENCE_MASK;
        let request_code = (if is_save { REQUEST_SAVE_BASE } else { REQUEST_OPEN_BASE }) | sequence as i32;
        {
            let mut pending = lock(&self.pending);
            if pending.is_some() {
                diagnostic_trace("PICKER rejected because another picker is pending");
                let _ = answer.send(Err("an Android document picker is already open".into()));
                return;
            }
            *pending = Some(PendingPicker { request_code, answer });
        }

        let app_for_ui = app.clone();
        let app_for_call = app_for_ui.clone();
        let state = Arc::clone(self);
        app_for_ui.run_on_java_main_thread(Box::new(move || {
            diagnostic_trace(&format!("PICKER UI-thread launch begin request={request_code}"));
            if let Err(error) = launch_picker(&app_for_call, request_code, request) {
                diagnostic_trace(&format!("PICKER launch failed request={request_code}: {error}"));
                state.fail_picker(request_code, format!("Could not open the Android document picker: {error}"));
            } else {
                diagnostic_trace(&format!("PICKER UI-thread launch returned request={request_code}"));
            }
        }));
    }

    fn fail_picker(&self, request_code: i32, error: String) {
        diagnostic_trace(&format!("PICKER fail request={request_code} error={error}"));
        let pending = {
            let mut slot = lock(&self.pending);
            if slot.as_ref().is_some_and(|picker| picker.request_code == request_code) { slot.take() } else { None }
        };
        if let Some(picker) = pending {
            let _ = picker.answer.send(Err(error));
        }
    }

    fn complete_picker(&self, request_code: i32, result_code: i32, paths: Vec<String>, uris: Vec<String>, error: Option<String>) {
        diagnostic_trace(&format!(
            "PICKER complete begin request={request_code} result={result_code} paths={} uris={} error={}",
            paths.len(),
            uris.len(),
            error.is_some()
        ));
        let pending = {
            let mut slot = lock(&self.pending);
            if slot.as_ref().is_some_and(|picker| picker.request_code == request_code) { slot.take() } else { None }
        };
        let Some(picker) = pending else {
            log::warn!("ignoring a stale Android SAF picker result");
            return;
        };

        log::info!(
            "SAF picker result received: request={request_code}, result={result_code}, paths={}, uris={}, has_error={}",
            paths.len(),
            uris.len(),
            error.is_some()
        );
        let result = if let Some(error) = error {
            Err(error)
        } else if result_code != RESULT_OK {
            Ok(Vec::new())
        } else if paths.len() != uris.len() {
            Err("Android returned an incomplete document-picker result".into())
        } else {
            self.register_uris(&paths, &uris).map(|()| paths)
        };
        let _ = picker.answer.send(result);
        diagnostic_trace(&format!("PICKER answer sent request={request_code}"));
        log::info!("SAF picker response delivered to the UI loop: request={request_code}");
    }

    fn register_uris(&self, paths: &[String], uris: &[String]) -> Result<(), String> {
        diagnostic_trace(&format!("URI registry update begin entries={}", paths.len()));
        let mut current = lock(&self.uris);
        let mut updated = current.clone();
        for (path, uri) in paths.iter().zip(uris) {
            updated.insert(path.clone(), uri.clone());
        }
        let bytes = serde_json::to_vec(&updated).map_err(|error| format!("could not encode the SAF URI registry: {error}"))?;
        write_registry(&self.registry_path, &bytes)?;
        *current = updated;
        diagnostic_trace("URI registry update complete");
        Ok(())
    }

    fn save_to_uri(&self, app: &AndroidApp, target: &str, bytes: &[u8]) -> Result<(), String> {
        diagnostic_trace(&format!("SAVE begin target={target} bytes={}", bytes.len()));
        log::info!("SAF save started; bytes={}", bytes.len());
        if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
            return Err("The document is larger than the 2 GiB SAF transfer limit.".into());
        }
        let uri = lock(&self.uris)
            .get(target)
            .cloned()
            .ok_or_else(|| "This SAF document is no longer linked to its Android URI. Use Save As to choose it again.".to_string())?;
        diagnostic_trace(&format!("SAVE URI lookup complete uri={uri}"));
        let target_path = PathBuf::from(target);
        let parent = target_path.parent().ok_or_else(|| "The SAF working path has no parent directory.".to_string())?;
        fs::create_dir_all(parent).map_err(|error| format!("could not prepare the private SAF working directory: {error}"))?;
        diagnostic_trace(&format!("SAVE staging begin parent={}", parent.display()));
        let staged = self.stage_bytes(parent, bytes)?;
        diagnostic_trace(&format!("SAVE staging complete path={}", staged.display()));
        log::info!("SAF bytes staged; calling the Android document provider");

        let remote_result = copy_staged_file_to_uri(app, &uri, &staged);
        if let Err(error) = remote_result {
            diagnostic_trace(&format!("SAVE JNI/provider returned error={error}"));
            log::error!("SAF provider write failed: {error}");
            let _ = fs::remove_file(&staged);
            return Err(error);
        }
        log::info!("SAF provider write completed");
        diagnostic_trace("SAVE provider write returned success");
        fs::rename(&staged, &target_path).map_err(|error| {
            let _ = fs::remove_file(&staged);
            format!("the Android document was written, but the local working copy could not be updated: {error}")
        })?;
        log::info!("SAF local working copy updated");
        diagnostic_trace("SAVE local working copy updated");
        Ok(())
    }

    fn stage_bytes(&self, parent: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
        diagnostic_trace(&format!("STAGE begin bytes={}", bytes.len()));
        for _ in 0..8 {
            let sequence = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(".wordcraft-saf-{}-{sequence}.tmp", std::process::id()));
            let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(format!("could not stage the SAF document safely: {error}")),
            };
            if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
                diagnostic_trace(&format!("STAGE write/sync failed: {error}"));
                let _ = fs::remove_file(&path);
                return Err(format!("could not stage the SAF document safely: {error}"));
            }
            diagnostic_trace(&format!("STAGE write/sync complete path={}", path.display()));
            return Ok(path);
        }
        Err("could not allocate a temporary SAF save file".into())
    }
}

pub(super) fn services(app: &AndroidApp) -> Result<Services, String> {
    let internal_data = app.internal_data_path().ok_or_else(|| "Android did not provide the app's private data directory".to_string())?;
    let root = internal_data.join("wordcraft-saf");
    fs::create_dir_all(&root).map_err(|error| format!("could not create the private SAF working directory: {error}"))?;
    let state = Arc::clone(SAF_STATE.get_or_init(|| Arc::new(SafState::load(root))));
    diagnostic_trace("SAF services initialized");
    PANIC_TRACE_HOOK.get_or_init(|| {
        let previous_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            diagnostic_trace(&format!("RUST PANIC thread={:?} info={info}", std::thread::current().id()));
            previous_hook(info);
        }));
    });

    let picker_state = Arc::clone(&state);
    let picker_app = app.clone();
    let save_state = Arc::clone(&state);
    let save_app = app.clone();
    Ok(Services {
        file_dialog: Some(Box::new(move |request| {
            diagnostic_trace("UI file_dialog service invoked");
            let (answer, receiver) = channel();
            picker_state.show_picker(&picker_app, request, answer);
            receiver
        })),
        host_save_at: Some(Box::new(move |name, path, bytes, save_as| {
            diagnostic_trace(&format!("UI host_save_at invoked name={name} path={path} bytes={} save_as={save_as}", bytes.len()));
            let result = save_state.save_to_uri(&save_app, path, bytes);
            diagnostic_trace(&format!("UI host_save_at returned success={}", result.is_ok()));
            result
        })),
        ..Default::default()
    })
}

fn write_registry(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, bytes).map_err(|error| format!("could not persist Android SAF permissions: {error}"))?;
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("could not persist Android SAF permissions: {error}"));
    }
    Ok(())
}

fn launch_picker(app: &AndroidApp, request_code: i32, request: FileDialogRequest) -> Result<(), String> {
    diagnostic_trace(&format!("JNI launch_picker begin request={request_code}"));
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(|env| -> jni::errors::Result<()> {
        let activity_raw = app.activity_as_ptr() as jobject;
        let activity = unsafe { env.as_cast_raw::<JObject<'_>>(&activity_raw)? };
        match request {
            FileDialogRequest::Open { purpose } => {
                diagnostic_trace(&format!("JNI openSafPicker call begin request={request_code}"));
                let purpose = JString::new(env, purpose)?;
                env.call_method(
                    activity.as_ref(),
                    jni_str!("openSafPicker"),
                    jni_sig!("(ILjava/lang/String;Z)V"),
                    &[JValue::Int(request_code), JValue::Object(&purpose), JValue::Bool(false)],
                )?;
            }
            FileDialogRequest::OpenMany { purpose } => {
                diagnostic_trace(&format!("JNI openSafPicker multiple call begin request={request_code}"));
                let purpose = JString::new(env, purpose)?;
                env.call_method(
                    activity.as_ref(),
                    jni_str!("openSafPicker"),
                    jni_sig!("(ILjava/lang/String;Z)V"),
                    &[JValue::Int(request_code), JValue::Object(&purpose), JValue::Bool(true)],
                )?;
            }
            FileDialogRequest::Save { name, .. } => {
                diagnostic_trace(&format!("JNI createSafPicker call begin request={request_code}"));
                let name = JString::new(env, name)?;
                env.call_method(
                    activity.as_ref(),
                    jni_str!("createSafPicker"),
                    jni_sig!("(ILjava/lang/String;)V"),
                    &[JValue::Int(request_code), JValue::Object(&name)],
                )?;
            }
        }
        diagnostic_trace(&format!("JNI launch_picker Java call returned request={request_code}"));
        Ok(())
    })
    .map_err(|error| error.to_string())
}

fn copy_staged_file_to_uri(app: &AndroidApp, uri: &str, source: &Path) -> Result<(), String> {
    diagnostic_trace(&format!("JNI writeSafDocument call begin uri={uri} source={}", source.display()));
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(|env| -> jni::errors::Result<Option<String>> {
        let activity_raw = app.activity_as_ptr() as jobject;
        let activity = unsafe { env.as_cast_raw::<JObject<'_>>(&activity_raw)? };
        let uri = JString::new(env, uri)?;
        let source = JString::new(env, source.to_string_lossy())?;
        let result = env.call_method(
            activity.as_ref(),
            jni_str!("writeSafDocument"),
            jni_sig!("(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;"),
            &[JValue::Object(&uri), JValue::Object(&source)],
        )?;
        diagnostic_trace("JNI writeSafDocument Java call returned");
        let result = result.l()?;
        if result.is_null() { Ok(None) } else { Ok(Some(JString::cast_local(env, result)?.try_to_string(env)?)) }
    })
    .map_err(|error| format!("could not write to the selected Android document: {error}"))?
    .map_or(Ok(()), Err)
}

fn strings_from_array<'local>(env: &mut Env<'local>, array: &JObjectArray<'local>) -> jni::errors::Result<Vec<String>> {
    let length = array.len(env)?;
    let mut strings = Vec::with_capacity(length);
    for index in 0..length {
        let object = array.get_element(env, index)?;
        if object.is_null() {
            continue;
        }
        let string = JString::cast_local(env, object)?;
        strings.push(string.try_to_string(env)?);
    }
    Ok(strings)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_ai_storyteller_wordcraft_WordCraftActivity_nativeOnSafResult<'local>(
    mut unowned_env: EnvUnowned<'local>,
    _activity: JObject<'local>,
    request_code: jint,
    result_code: jint,
    paths: JObjectArray<'local>,
    uris: JObjectArray<'local>,
    error: JString<'local>,
) {
    diagnostic_trace(&format!("JNI callback entry request={request_code} result={result_code}"));
    log::info!("SAF JNI callback entered; request={request_code}");
    let Some(state) = SAF_STATE.get() else {
        log::error!("Android SAF returned a result before its bridge was initialized");
        return;
    };
    let result = unowned_env.with_env(|env: &mut Env<'local>| -> jni::errors::Result<()> {
        diagnostic_trace("JNI callback attached environment; decoding arrays");
        if paths.as_raw().is_null() || uris.as_raw().is_null() {
            state.fail_picker(request_code, "Android returned an empty SAF picker result.".into());
            return Ok(());
        }
        let paths = match strings_from_array(env, &paths) {
            Ok(paths) => paths,
            Err(error) => {
                state.fail_picker(request_code, format!("could not read the Android SAF file path: {error}"));
                return Ok(());
            }
        };
        let uris = match strings_from_array(env, &uris) {
            Ok(uris) => uris,
            Err(error) => {
                state.fail_picker(request_code, format!("could not read the Android SAF URI: {error}"));
                return Ok(());
            }
        };
        let error = if error.as_raw().is_null() {
            None
        } else {
            match error.try_to_string(env) {
                Ok(message) => Some(message),
                Err(error) => {
                    state.fail_picker(request_code, format!("could not read the Android SAF error: {error}"));
                    return Ok(());
                }
            }
        };
        state.complete_picker(request_code, result_code, paths, uris, error);
        diagnostic_trace(&format!("JNI callback complete_picker returned request={request_code}"));
        log::info!("SAF JNI callback processed; request={request_code}");
        Ok(())
    });
    diagnostic_trace("JNI callback resolving result");
    result.resolve::<jni::errors::LogErrorAndDefault>();
    diagnostic_trace("JNI callback returned to Java");
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
