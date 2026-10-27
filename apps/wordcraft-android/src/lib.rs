#![cfg(target_os = "android")]

use eframe::{App, Frame, NativeOptions, egui};
use winit::platform::android::activity::AndroidApp;
use wordcraft_engine::Session;
use wordcraft_ui_egui::{Services, WordApp};

mod saf;

struct AndroidWordCraft(WordApp);

impl App for AndroidWordCraft {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut Frame) {
        self.0.logic(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut Frame) {
        self.0.ui(ui);
    }
}

impl Drop for AndroidWordCraft {
    fn drop(&mut self) {
        self.0.session.finish_recovery();
    }
}

#[unsafe(no_mangle)]
fn android_main(android_app: AndroidApp) {
    android_logger::init_once(android_logger::Config::default().with_max_level(log::LevelFilter::Info));

    let recovery_root = android_app.internal_data_path().map(|root| root.join("wordcraft-autosave-cache"));
    let services = match saf::services(&android_app) {
        Ok(services) => services,
        Err(error) => {
            log::error!("WordCraft Android SAF initialization failed: {error}");
            Services::default()
        }
    };
    let options =
        NativeOptions { android_app: Some(android_app), viewport: egui::ViewportBuilder::default().with_title("WordCraft"), ..Default::default() };

    let result = eframe::run_native(
        "WordCraft",
        options,
        Box::new(move |creation_context| {
            let region = wordcraft_ui_egui::i18n::system_region();
            let mut session = Session::new(wordcraft_engine::blank_document(&region));
            session.system_region = region;
            if let Some(root) = recovery_root {
                let wake_context = creation_context.egui_ctx.clone();
                if let Err(error) = session.enable_autosave_cache(root, std::sync::Arc::new(move || wake_context.request_repaint())) {
                    log::error!("WordCraft Android private AutoSave cache unavailable: {error}");
                }
            } else {
                log::error!("WordCraft Android private AutoSave cache unavailable: no app data directory");
            }
            let mut app = WordApp::new(session, services);
            app.start_screen();
            Ok(Box::new(AndroidWordCraft(app)))
        }),
    );

    if let Err(error) = result {
        log::error!("WordCraft Android startup failed: {error}");
    }
}
