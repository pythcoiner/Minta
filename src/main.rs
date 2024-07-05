mod bitcoind;
mod gui;
mod service;

use crate::gui::Flags;
use bitcoind::{BitcoinD, BitcoinMessage};
use chrono::Local;
use colored::Colorize;
use gui::Gui;
use iced::{Application, Settings, Size};
use service::ServiceFn;

#[tokio::main]
async fn main() {
    let verbose_log = true;
    fern::Dispatch::new()
        .format(move |out, message, record| {
            let color = match record.level() {
                log::Level::Error => "red",
                log::Level::Warn => "yellow",
                log::Level::Info => "green",
                log::Level::Debug => "blue",
                log::Level::Trace => "magenta",
            };

            let file = record.file();
            let line = record.line();
            let mut file_line = "".to_string();

            if let Some(f) = file {
                file_line = format!(":{}", f);
                if let Some(l) = line {
                    file_line = format!("{}:{}", file_line, l);
                }
            }
            let formatted = if verbose_log {
                format!(
                    "[{}][{}{}][{}] {}",
                    Local::now().format("%Y-%m-%d %H:%M:%S"),
                    record.target(),
                    file_line,
                    record.level(),
                    message
                )
            } else {
                format!("[{}] {}", record.level(), message)
            };
            out.finish(format_args!("{}", formatted.color(color)))
        })
        .level(log::LevelFilter::Error)
        .level_for("regtest_gui", log::LevelFilter::Info)
        // .level_for("modbus485_debugger", log::LevelFilter::Debug)
        .chain(std::io::stdout())
        .apply()
        .unwrap();

    let (gui_sender, bitcoin_receiver) = std::sync::mpsc::channel::<BitcoinMessage>();
    let (bitcoin_sender, gui_receiver) = async_channel::unbounded::<BitcoinMessage>();

    let bitcoind = BitcoinD::new(bitcoin_sender, bitcoin_receiver, gui_sender.clone());

    let mut settings = Settings::with_flags(Flags {
        sender: gui_sender,
        receiver: gui_receiver,
    });

    settings.window.size = Size {
        width: 500.0,
        height: 700.0,
    };
    settings.window.resizable = false;

    tokio::spawn(async move {
        bitcoind.start().await;
    });

    // Run the GUI
    Gui::run(settings).expect("Failed to run GUI");
}
