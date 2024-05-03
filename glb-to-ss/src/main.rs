use std::{io::BufReader, net::ToSocketAddrs, path::PathBuf, sync::Arc};

use eframe::egui;
use tokio::{
    io::{self, AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tokio_rustls::{
    rustls::{self, pki_types},
    TlsConnector,
};
use tracing::{error, info, Level};
use tracing_subscriber::FmtSubscriber;

fn main() -> Result<(), eframe::Error> {
    // Trace initialization
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::TRACE)
        .finish();

    tracing::subscriber::set_global_default(subscriber)
        .expect("Could not set default trace subscriver!");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([640.0, 240.0]) // wide enough for the drag-drop overlay text
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "SS Exporter",
        options,
        Box::new(|_cc| Box::new(MyApp::new())),
    )
}

struct MyApp {
    runtime: tokio::runtime::Runtime,
    picked_path: Option<PathBuf>,
    auth_token: String,
    ip: String,
}

impl MyApp {
    pub fn new() -> Self {
        Self {
            runtime: tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("Failed to build multi-threaded async runtime!"),
            picked_path: None,
            auth_token: "".to_owned(),
            ip: "".to_owned(),
        }
    }

    pub fn send_model(&self, ui: &mut egui::Ui) -> tokio::io::Result<()> {
        let enabled =
            self.picked_path.is_some() && !self.auth_token.is_empty() && !self.ip.is_empty();
        if ui.add_enabled(enabled, egui::Button::new("Send")).clicked() {
            return self.runtime.block_on(self.send_to_server());
        }

        Ok(())
    }

    pub async fn send_to_server(&self) -> tokio::io::Result<()> {
        let model_path = self.picked_path.clone().unwrap().display().to_string();
        let auth_token = self.auth_token.clone();
        let server = self.ip.clone();
        let domain_name = "as-http.angel-sunset.app";

        info!("\n Model path: {model_path} \n Auth token: {auth_token} \n Server ip: {server}");

        let addr = (server.as_str(), 8080)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;

        let mut root_cert_pem =
            BufReader::new(include_bytes!("certs/origin_ca_rsa_root.pem").as_slice());

        let mut root_cert_store = rustls::RootCertStore::empty();

        for cert in rustls_pemfile::certs(&mut root_cert_pem) {
            root_cert_store.add(cert?).unwrap();
        }

        let config =
            rustls::ClientConfig::builder_with_protocol_versions(&[&rustls::version::TLS12])
                .with_root_certificates(root_cert_store)
                .with_no_client_auth();

        let connector = TlsConnector::from(Arc::new(config));

        let stream = TcpStream::connect(&addr).await?;

        let domain = pki_types::ServerName::try_from(domain_name)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid dnsname"))?
            .to_owned();

        let mut stream = connector.connect(domain, stream).await?;

        let request = format!("put model {} {}", auth_token, "stub");

        stream.write_all(&request.len().to_ne_bytes()).await?;
        stream.write_all(&request.as_bytes()).await?;

        let mut response_size: [u8; 8] = [0; 8];
        stream.read_exact(&mut response_size).await?;
        let response_size = usize::from_ne_bytes(response_size);

        let mut response = vec![0u8; response_size];
        stream.read_exact(&mut response).await?;

        let response = std::str::from_utf8(&response).unwrap_or("Error Parsing Response");

        match response {
            "OK" => {
                let Ok(model_bytes) = std::fs::read(model_path.clone()) else {
                    error!("File <{}> not found!", model_path);
                    return Err(io::Error::from(io::ErrorKind::NotFound));
                };

                stream.write_all(&model_bytes.len().to_ne_bytes()).await?;
                stream.write_all(&model_bytes).await?;

                info!("Model Successfully sent!");
            }
            _ => {
                error!("Request denied! Response from server: {response}")
            }
        }

        Ok(())
    }
}

impl eframe::App for MyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label("Drag-and-drop a GLB model into the window or choose one by clicking on the button below!");

            if ui.button("Open file…").clicked() {
                if let Some(path) = rfd::FileDialog::new().pick_file() {
                    if path.extension().unwrap().to_str().unwrap() == "glb" {
                        self.picked_path = Some(path);
                    } else {
                        error!("Only .glb files are supported currently!");
                    }
                }
            }

            if let Some(picked_path) = &self.picked_path {
                ui.horizontal(|ui| {
                    ui.label("Picked file:");
                    ui.monospace(picked_path.display().to_string());
                });
            }

            ui.label("Please provide the ip of the server you would like to send the model to:");
            ui.add(egui::TextEdit::singleline(&mut self.ip));

            ui.label("Please provide an auth token so the server can accept the request:");
            ui.add(egui::TextEdit::singleline(&mut self.auth_token));

            if let Err(err) = self.send_model(ui) {
                error!("{err}");
            }
        });

        preview_files_being_dropped(ctx);

        // Collect dropped files:
        ctx.input(|i| {
            if !i.raw.dropped_files.is_empty() {
                let path = i.raw.dropped_files.last().unwrap().path.clone();
                if let Some(path) = path {
                    if path.extension().unwrap().to_str().unwrap() == "glb" {
                        self.picked_path = Some(path.clone());
                    } else {
                        error!("Only .glb files are supported currently!");
                    }
                }
            }
        });
    }
}

/// Preview hovering files:
fn preview_files_being_dropped(ctx: &egui::Context) {
    use egui::*;
    use std::fmt::Write as _;

    if !ctx.input(|i| i.raw.hovered_files.is_empty()) {
        let text = ctx.input(|i| {
            let mut text = "Dropping files:\n".to_owned();
            for file in &i.raw.hovered_files {
                if let Some(path) = &file.path {
                    if path.extension().unwrap().to_str().unwrap() == "glb" {
                        write!(text, "\n{}", path.display()).ok();
                    } else {
                        text = "Only .glb files are supported currently!".to_owned();
                    }
                }
            }
            text
        });

        let painter =
            ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("file_drop_target")));

        let screen_rect = ctx.screen_rect();
        painter.rect_filled(screen_rect, 0.0, Color32::from_black_alpha(192));
        painter.text(
            screen_rect.center(),
            Align2::CENTER_CENTER,
            text,
            TextStyle::Heading.resolve(&ctx.style()),
            Color32::WHITE,
        );
    }
}
