use eframe::egui;
use eyre::Context;

use crate::MESSAGE_READ;

use super::MESSAGE_COPY;

use std::{
    io::{BufReader, Write},
    os::unix::net::UnixStream,
    path::Path,
    time::Instant,
};

use super::Entry;

pub(crate) struct App {
    pub(crate) items: Vec<Entry>,
    pub(crate) selected_idx: usize,
    pub(crate) socket: UnixStream,
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.input(|i| {
                if i.key_pressed(egui::Key::J) || i.key_pressed(egui::Key::ArrowDown) {
                    if self.selected_idx + 1 != self.items.len() {
                        self.selected_idx += 1;
                    }
                }
                if i.key_pressed(egui::Key::K) || i.key_pressed(egui::Key::ArrowUp) {
                    self.selected_idx = self.selected_idx.saturating_sub(1);
                }

                if i.key_pressed(egui::Key::Enter)
                    && let Some(item) = self.items.get(self.selected_idx)
                {
                    let _ = self.socket.write_all(&[MESSAGE_COPY]);
                    let _ = self.socket.write_all(&item.id.to_le_bytes());
                    std::process::exit(0);
                }
            });

            ui.heading("clippyboard");

            egui::SidePanel::left("selection_panel")
                .default_width(400.0)
                .show_inside(ui, |ui| {
                    ui.heading("History");

                    ui.add_space(10.0);

                    for (idx, item) in self.items.iter().enumerate() {
                        let mut frame = egui::Frame::new().inner_margin(3.0);
                        if self.selected_idx == idx {
                            frame = frame.stroke(egui::Stroke::new(1.0, egui::Color32::PURPLE));
                        }
                        frame.show(ui, |ui| match item.mime.as_str() {
                            "text/plain" => {
                                let mut full =
                                    str::from_utf8(&item.data).unwrap_or("<invalid UTF-8>");
                                if full.len() > 1000 {
                                    full = &full[..1000];
                                }
                                ui.label(full);
                            }
                            "image/png" => {
                                ui.label("<image>");
                            }
                            _ => {
                                ui.label("<unsupported mime type>");
                            }
                        });

                        ui.separator();
                    }
                });

            egui::CentralPanel::default().show_inside(ui, |ui| {
                ui.heading("Detail");
                let Some(item) = &self.items.get(self.selected_idx) else {
                    return;
                };

                ui.add_space(10.0);

                match item.mime.as_str() {
                    "text/plain" => {
                        ui.label(str::from_utf8(&item.data).unwrap_or("<invalid UTF-8>"));
                    }
                    "image/png" => {
                        ui.image(egui::ImageSource::Bytes {
                            uri: format!("bytes://{}", item.id).into(),
                            bytes: item.data.clone().into(),
                        });
                    }
                    _ => {
                        ui.label("<unsupported mime type>");
                    }
                }
            });
        });
    }
}

pub fn main(socket_path: &Path) -> eyre::Result<()> {
    let mut socket = UnixStream::connect(&socket_path).wrap_err_with(|| {
        format!(
            "connecting to socket at {}. is the daemon running?",
            socket_path.display()
        )
    })?;
    socket
        .write_all(&[MESSAGE_READ])
        .wrap_err("writing request type")?;

    println!("INFO: Reading clipboard history from socket");
    let start = Instant::now();
    let mut items: Vec<Entry> =
        ciborium::from_reader(BufReader::new(socket)).wrap_err("reading items from socket")?;
    println!(
        "INFO: Read clipboard history from socket in {:?}",
        start.elapsed()
    );

    items.reverse();

    // heh. good design.
    let socket = UnixStream::connect(&socket_path).wrap_err_with(|| {
        format!(
            "connecting to socket at {}. is the daemon running?",
            socket_path.display()
        )
    })?;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([500.0, 500.0]),
        ..Default::default()
    };
    eframe::run_native(
        "clippyboard",
        options,
        Box::new(|cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Ok(Box::new(App {
                items,
                selected_idx: 0,
                socket,
            }))
        }),
    )
    .map_err(|err| eyre::eyre!(err.to_string()))
    .wrap_err("running GUI")?;

    Ok(())
}
