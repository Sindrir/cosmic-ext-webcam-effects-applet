// SPDX-License-Identifier: GPL-3.0

fn main() -> cosmic::iced::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    let requested_languages = i18n_embed::DesktopLanguageRequester::requested_languages();
    cosmic_ext_webcam_effects_applet::i18n::init(&requested_languages);
    cosmic::applet::run::<cosmic_ext_webcam_effects_applet::app::AppModel>(())
}
